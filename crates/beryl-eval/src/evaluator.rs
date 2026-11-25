//! Lua 5.4 evaluator compiling `pkg.lua` definitions into deterministic Nix ATerm `.drv` files.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Cursor;
use std::path::Path;
use std::sync::{Arc, Mutex};

use beryl_nix::derivation::{
    Derivation, DerivationBuilder, DrvHashModulo, OutputHash, OutputHashMode, OutputName,
    OutputsBuilder, lookup_fn,
};
use beryl_nix::nar_fs::{dump_nar, hash_nar_path};
use beryl_nix::nixbase32;
use beryl_nix::nixhash::NixHash;
use beryl_nix::store_path::StorePath;
use beryl_store::{LocalStore, Store, ValidPathInfo};
use mlua::{Lua, Table, Value};

use crate::error::{EvalError, Result};
use crate::project::{DevShellRef, DevShellSpec, ProjectManifest};
use crate::store_ref::StoreRef;

/// Callback trait for resolving `beryl.nixpkgs("attr")` during Lua evaluation.
pub trait NixpkgsLookup: Send + Sync {
    fn resolve_nixpkgs(&self, system: &str, attr: &str) -> Result<StorePath>;
}

/// Result of evaluating a `pkg.lua` file.
#[derive(Debug, Clone)]
pub struct EvalResult {
    /// Root package `StoreRef` returned by `pkg.lua`.
    pub root: StoreRef,
    /// Complete DAG of derivations (including `fetch_url` FODs and nested derivations)
    /// keyed by their `.drv` `StorePath`.
    pub derivations: BTreeMap<StorePath, Derivation>,
    /// Prebuilt `nixpkgs` store paths referenced as `inputSrcs`.
    pub input_sources: BTreeSet<StorePath>,
}

impl EvalResult {
    /// Write all evaluated `.drv` ATerm files into `store` as valid store objects.
    pub async fn write_drvs_to_store(&self, store: &LocalStore) -> Result<Vec<StorePath>> {
        let mut written = Vec::with_capacity(self.derivations.len());

        // Write in deterministic order; `.drv` files are plain text files in `/nix/store/*.drv`.
        // We compute each `.drv` file's NAR representation and register it in `LocalStore`.
        // Note: input_derivations may reference other `.drv`s, so we register in topological order.
        let ordered = topo_sort_drvs(&self.derivations);
        for drv_path in ordered {
            let drv = &self.derivations[&drv_path];
            if store.has_path(&drv_path).await? {
                written.push(drv_path);
                continue;
            }

            let aterm_bytes = drv.to_aterm_bytes();
            let tmp = tempfile::tempdir_in(store.tmp_dir())?;
            let drv_file = tmp.path().join("drv");
            fs::write(&drv_file, &aterm_bytes)?;

            let hashed = hash_nar_path(&drv_file)?;
            let mut nar = Vec::with_capacity(hashed.size as usize);
            dump_nar(&drv_file, &mut nar)?;

            // References of a `.drv` file are its input_derivations + input_sources that are present in store.
            let mut references = BTreeSet::new();
            for in_drv in drv.input_derivations().keys() {
                if store.has_path(in_drv).await? {
                    references.insert(in_drv.clone());
                }
            }
            for in_src in drv.input_sources() {
                if store.has_path(in_src).await? {
                    references.insert(in_src.clone());
                }
            }

            let info = ValidPathInfo {
                path: drv_path.clone(),
                nar_hash: hashed.sha256,
                nar_size: hashed.size,
                references,
                deriver: None,
                sigs: vec![],
                ca: None,
                registration_time: 0,
            };

            store.add_nar(info, Cursor::new(nar)).await?;
            written.push(drv_path);
        }

        Ok(written)
    }
}

#[derive(Default)]
struct EvalState {
    derivations: BTreeMap<StorePath, Derivation>,
    hdms: BTreeMap<StorePath, DrvHashModulo>,
    input_sources: BTreeSet<StorePath>,
}

/// Lua 5.4 evaluator for Beryl `pkg.lua` files.
pub struct Evaluator {
    system: String,
    lookup: Arc<dyn NixpkgsLookup>,
}

impl Evaluator {
    pub fn new(system: impl Into<String>, lookup: Arc<dyn NixpkgsLookup>) -> Self {
        Self {
            system: system.into(),
            lookup,
        }
    }

    /// Evaluate a `pkg.lua` file from disk.
    pub fn eval_file(&self, path: &Path) -> Result<EvalResult> {
        let source = fs::read_to_string(path)?;
        let chunk_name = path.display().to_string();
        self.eval_str(&source, &chunk_name)
    }

    /// Evaluate a Lua package definition string.
    pub fn eval_str(&self, source: &str, chunk_name: &str) -> Result<EvalResult> {
        let (lua, state) = self.create_lua_env()?;

        let value: Value = lua.load(source).set_name(chunk_name).eval()?;
        let root = match value {
            Value::UserData(ud) => ud.borrow::<StoreRef>()?.clone(),
            _ => return Err(EvalError::InvalidReturnValue),
        };

        let st = state.lock().expect("eval state mutex poisoned");
        Ok(EvalResult {
            root,
            derivations: st.derivations.clone(),
            input_sources: st.input_sources.clone(),
        })
    }

    /// Evaluate a `beryl.lua` project manifest file from disk.
    pub fn eval_project_file(&self, path: &Path) -> Result<ProjectManifest> {
        let source = fs::read_to_string(path)?;
        let chunk_name = path.display().to_string();
        self.eval_project_str(&source, &chunk_name)
    }

    /// Evaluate a `beryl.lua` project manifest string.
    pub fn eval_project_str(&self, source: &str, chunk_name: &str) -> Result<ProjectManifest> {
        let (lua, state) = self.create_lua_env()?;

        let value: Value = lua.load(source).set_name(chunk_name).eval()?;
        let tbl = match value {
            Value::Table(t) => t,
            _ => return Err(EvalError::InvalidManifest),
        };

        let description: Option<String> = tbl.get("description")?;

        let mut shells = BTreeMap::new();
        if let Some(shells_tbl) = tbl.get::<Option<Table>>("shells")? {
            for pair in shells_tbl.pairs::<String, Value>() {
                let (name, val) = pair?;
                let spec = match val {
                    Value::UserData(ud) => ud.borrow::<DevShellRef>()?.0.clone(),
                    Value::Table(t) => parse_shell_table(&t)?,
                    other => {
                        return Err(EvalError::Lua(mlua::Error::RuntimeError(format!(
                            "shells.{name} must be a beryl.shell(...) or table, got {}",
                            other.type_name()
                        ))));
                    }
                };
                shells.insert(name, spec);
            }
        }

        let mut packages = BTreeMap::new();
        if let Some(pkgs_tbl) = tbl.get::<Option<Table>>("packages")? {
            for pair in pkgs_tbl.pairs::<String, Value>() {
                let (name, val) = pair?;
                match val {
                    Value::UserData(ud) => {
                        packages.insert(name, ud.borrow::<StoreRef>()?.clone());
                    }
                    other => {
                        return Err(EvalError::Lua(mlua::Error::RuntimeError(format!(
                            "packages.{name} must be a StoreRef/derivation, got {}",
                            other.type_name()
                        ))));
                    }
                }
            }
        }

        let st = state.lock().expect("eval state mutex poisoned");
        Ok(ProjectManifest {
            description,
            shells,
            packages,
            derivations: st.derivations.clone(),
            input_sources: st.input_sources.clone(),
        })
    }

    fn create_lua_env(&self) -> Result<(Lua, Arc<Mutex<EvalState>>)> {
        let lua = Lua::new();
        let state = Arc::new(Mutex::new(EvalState::default()));

        let beryl_tbl = lua.create_table()?;
        beryl_tbl.set("system", self.system.as_str())?;

        // 1. beryl.nixpkgs(attr)
        {
            let system = self.system.clone();
            let lookup = Arc::clone(&self.lookup);
            let state = Arc::clone(&state);
            let f = lua.create_function(move |_, attr: String| {
                let sp = lookup
                    .resolve_nixpkgs(&system, &attr)
                    .map_err(|e| mlua::Error::RuntimeError(e.to_string()))?;
                state
                    .lock()
                    .expect("eval state mutex poisoned")
                    .input_sources
                    .insert(sp.clone());
                Ok(StoreRef::prebuilt(sp, attr))
            })?;
            beryl_tbl.set("nixpkgs", f)?;
        }

        // 2. beryl.fetch_url({ url, sha256, name? })
        {
            let system = self.system.clone();
            let state = Arc::clone(&state);
            let f = lua.create_function(move |_, spec: Table| {
                let url: String = spec.get("url")?;
                let sha256_str: String = spec.get("sha256")?;
                let name: String = match spec.get::<Option<String>>("name")? {
                    Some(n) => sanitize_store_name(&n),
                    None => filename_from_url(&url),
                };

                let digest = parse_sha256(&sha256_str)
                    .map_err(|e| mlua::Error::RuntimeError(e.to_string()))?;
                let nix_hash = NixHash::Sha256(digest);
                let output_hash = OutputHash {
                    hash: nix_hash.clone(),
                    mode: OutputHashMode::Flat,
                };

                let mut env = BTreeMap::new();
                env.insert("builder".into(), "builtin:fetchurl".into());
                env.insert("name".into(), name.as_str().into());
                env.insert("out".into(), "".into());
                env.insert("outputHash".into(), hex::encode(digest).into());
                env.insert("outputHashAlgo".into(), "sha256".into());
                env.insert("outputHashMode".into(), "flat".into());
                env.insert("system".into(), system.as_str().into());
                env.insert("url".into(), url.as_str().into());

                let builder = DerivationBuilder {
                    arguments: vec![],
                    command: "builtin:fetchurl".into(),
                    environment: env,
                    input_derivations: BTreeMap::new(),
                    input_sources: BTreeSet::new(),
                    outputs: OutputsBuilder::Fixed(output_hash),
                    system: system.as_str().into(),
                };

                let drv = builder
                    .build(&name, lookup_fn(|_| None))
                    .map_err(|e| mlua::Error::RuntimeError(e.to_string()))?;

                let drv_path = drv.drv_path().clone();
                let out_path = drv
                    .outputs()
                    .get(&OutputName::out())
                    .expect("FOD always has `out` output")
                    .clone();
                let hdm = drv
                    .hash_derivation_modulo(lookup_fn(|_| None))
                    .map_err(|e| mlua::Error::RuntimeError(e.to_string()))?;

                {
                    let mut st = state.lock().expect("eval state mutex poisoned");
                    st.hdms.insert(drv_path.clone(), hdm);
                    st.derivations.insert(drv_path.clone(), drv);
                }

                Ok(StoreRef::from_derivation(out_path, drv_path, name))
            })?;
            beryl_tbl.set("fetch_url", f)?;
        }

        // 3. beryl.derivation({ name, version?, src?, inputs?, env?, builder?, args?, build?, system? })
        {
            let default_system = self.system.clone();
            let state = Arc::clone(&state);
            let f = lua.create_function(move |_, spec: Table| {
                let base_name: String = spec.get("name")?;
                let version: Option<String> = spec.get("version")?;
                let full_name = match version {
                    Some(v) if !v.is_empty() => format!("{base_name}-{v}"),
                    _ => base_name.clone(),
                };
                let full_name = sanitize_store_name(&full_name);

                let system: String = spec
                    .get::<Option<String>>("system")?
                    .unwrap_or_else(|| default_system.clone());
                let command: String = spec
                    .get::<Option<String>>("builder")?
                    .unwrap_or_else(|| "/bin/sh".to_string());
                let build_script: Option<String> = spec.get("build")?;
                let explicit_args: Option<Vec<String>> = spec.get("args")?;

                let arguments: Vec<bstr::BString> = match (explicit_args, &build_script) {
                    (Some(args), _) => args.into_iter().map(Into::into).collect(),
                    (None, Some(script)) => vec!["-e".into(), "-c".into(), script.as_str().into()],
                    (None, None) => vec![],
                };

                let mut env: BTreeMap<bstr::BString, bstr::BString> = BTreeMap::new();
                env.insert("builder".into(), command.as_str().into());
                env.insert("name".into(), full_name.as_str().into());
                env.insert("out".into(), "".into());
                env.insert("system".into(), system.as_str().into());

                let mut input_derivations: BTreeMap<StorePath, BTreeSet<OutputName>> =
                    BTreeMap::new();
                let mut input_sources: BTreeSet<StorePath> = BTreeSet::new();
                let mut input_paths_for_env: Vec<String> = Vec::new();

                // Optional `src` (StoreRef or string)
                if let Some(src_val) = spec.get::<Option<Value>>("src")? {
                    match src_val {
                        Value::UserData(ud) => {
                            let r = ud.borrow::<StoreRef>()?;
                            register_input(&r, &mut input_derivations, &mut input_sources);
                            env.insert("src".into(), r.out_path.to_absolute_path().into());
                        }
                        Value::String(s) => {
                            env.insert("src".into(), s.to_str()?.to_string().into());
                        }
                        other => {
                            return Err(mlua::Error::RuntimeError(format!(
                                "`src` must be a StoreRef or string, got {}",
                                other.type_name()
                            )));
                        }
                    }
                }

                // Optional `inputs` table (array or keyed map of StoreRefs)
                if let Some(inputs_tbl) = spec.get::<Option<Table>>("inputs")? {
                    for pair in inputs_tbl.pairs::<Value, Value>() {
                        let (k, v) = pair?;
                        let ud = match v {
                            Value::UserData(ud) => ud,
                            other => {
                                return Err(mlua::Error::RuntimeError(format!(
                                    "`inputs` entries must be StoreRef values, got {}",
                                    other.type_name()
                                )));
                            }
                        };
                        let r = ud.borrow::<StoreRef>()?;
                        register_input(&r, &mut input_derivations, &mut input_sources);
                        let abs = r.out_path.to_absolute_path();
                        input_paths_for_env.push(abs.clone());

                        // Export input as an environment variable for build scripts
                        match k {
                            Value::String(s) => {
                                env.insert(s.to_str()?.to_string().into(), abs.into());
                            }
                            Value::Integer(_) => {
                                if let Some(ref attr) = r.env_name {
                                    let var = sanitize_env_var(attr);
                                    if !var.is_empty() {
                                        env.insert(var.into(), abs.clone().into());
                                    }
                                    // Also expose base name before `_` (e.g. `openssl_3` -> `openssl`)
                                    if let Some((base, _)) = attr.split_once('_') {
                                        let base_var = sanitize_env_var(base);
                                        if !base_var.is_empty()
                                            && !env.contains_key(base_var.as_bytes())
                                        {
                                            env.insert(base_var.into(), abs.into());
                                        }
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }

                if !input_paths_for_env.is_empty() {
                    env.insert("BERYL_INPUTS".into(), input_paths_for_env.join(" ").into());
                }

                // Optional extra `env` table
                if let Some(extra_env) = spec.get::<Option<Table>>("env")? {
                    for pair in extra_env.pairs::<String, Value>() {
                        let (k, v) = pair?;
                        let val_str = match v {
                            Value::String(s) => s.to_str()?.to_string(),
                            Value::Integer(i) => i.to_string(),
                            Value::Number(n) => n.to_string(),
                            Value::Boolean(b) => if b { "1" } else { "" }.to_string(),
                            Value::UserData(ud) => {
                                ud.borrow::<StoreRef>()?.out_path.to_absolute_path()
                            }
                            other => {
                                return Err(mlua::Error::RuntimeError(format!(
                                    "unsupported env value type for '{k}': {}",
                                    other.type_name()
                                )));
                            }
                        };
                        env.insert(k.into(), val_str.into());
                    }
                }

                let builder = DerivationBuilder {
                    arguments,
                    command: command.into(),
                    environment: env,
                    input_derivations,
                    input_sources,
                    outputs: OutputsBuilder::default(),
                    system: system.into(),
                };

                let mut st = state.lock().expect("eval state mutex poisoned");
                let hdm_lookup = lookup_fn(|drv_path| st.hdms.get(&drv_path.to_owned()).cloned());
                let drv = builder
                    .build(&full_name, hdm_lookup)
                    .map_err(|e| mlua::Error::RuntimeError(e.to_string()))?;

                let drv_path = drv.drv_path().clone();
                let out_path = drv
                    .outputs()
                    .get(&OutputName::out())
                    .expect("derivation has `out` output")
                    .clone();
                let hdm_lookup2 = lookup_fn(|p| st.hdms.get(&p.to_owned()).cloned());
                let hdm = drv
                    .hash_derivation_modulo(hdm_lookup2)
                    .map_err(|e| mlua::Error::RuntimeError(e.to_string()))?;

                st.hdms.insert(drv_path.clone(), hdm);
                st.derivations.insert(drv_path.clone(), drv);

                Ok(StoreRef::from_derivation(out_path, drv_path, full_name))
            })?;
            beryl_tbl.set("derivation", f)?;
        }

        // 4. beryl.shell({ packages?, libs?, env?, hook? })
        {
            let f = lua.create_function(|_, spec: Table| {
                let shell = parse_shell_table(&spec)?;
                Ok(DevShellRef(shell))
            })?;
            beryl_tbl.set("shell", f)?;
        }

        lua.globals().set("beryl", beryl_tbl)?;
        Ok((lua, state))
    }
}

fn parse_shell_table(spec: &Table) -> mlua::Result<DevShellSpec> {
    let mut packages = Vec::new();
    if let Some(pkgs_tbl) = spec.get::<Option<Table>>("packages")? {
        for val in pkgs_tbl.sequence_values::<Value>() {
            let val = val?;
            match val {
                Value::UserData(ud) => packages.push(ud.borrow::<StoreRef>()?.clone()),
                other => {
                    return Err(mlua::Error::RuntimeError(format!(
                        "`packages` entries must be StoreRef values, got {}",
                        other.type_name()
                    )));
                }
            }
        }
    }

    let mut libs = Vec::new();
    if let Some(libs_tbl) = spec.get::<Option<Table>>("libs")? {
        for val in libs_tbl.sequence_values::<Value>() {
            let val = val?;
            match val {
                Value::UserData(ud) => libs.push(ud.borrow::<StoreRef>()?.clone()),
                other => {
                    return Err(mlua::Error::RuntimeError(format!(
                        "`libs` entries must be StoreRef values, got {}",
                        other.type_name()
                    )));
                }
            }
        }
    }

    let mut env = BTreeMap::new();
    if let Some(env_tbl) = spec.get::<Option<Table>>("env")? {
        for pair in env_tbl.pairs::<String, Value>() {
            let (k, v) = pair?;
            let val_str = match v {
                Value::String(s) => s.to_str()?.to_string(),
                Value::Integer(i) => i.to_string(),
                Value::Number(n) => n.to_string(),
                Value::Boolean(b) => if b { "1" } else { "" }.to_string(),
                Value::UserData(ud) => ud.borrow::<StoreRef>()?.out_path.to_absolute_path(),
                other => {
                    return Err(mlua::Error::RuntimeError(format!(
                        "unsupported env value type for '{k}': {}",
                        other.type_name()
                    )));
                }
            };
            env.insert(k, val_str);
        }
    }

    let hook: Option<String> = spec.get("hook")?;
    Ok(DevShellSpec {
        packages,
        libs,
        env,
        hook,
    })
}

fn register_input(
    r: &StoreRef,
    input_derivations: &mut BTreeMap<StorePath, BTreeSet<OutputName>>,
    input_sources: &mut BTreeSet<StorePath>,
) {
    if let Some(ref drv_path) = r.drv_path {
        input_derivations
            .entry(drv_path.clone())
            .or_default()
            .insert(OutputName::out());
    } else {
        input_sources.insert(r.out_path.clone());
    }
}

fn parse_sha256(raw: &str) -> Result<[u8; 32]> {
    let trimmed = raw.trim();
    // 1. Hex (64 chars, optionally prefixed with `sha256:`)
    let without_prefix = trimmed.strip_prefix("sha256:").unwrap_or(trimmed);
    if without_prefix.len() == 64
        && let Ok(bytes) = hex::decode(without_prefix)
        && let Ok(arr) = <[u8; 32]>::try_from(bytes)
    {
        return Ok(arr);
    }
    // 2. Nix base32 (52 chars, optionally prefixed with `sha256:`)
    if without_prefix.len() == 52
        && let Ok(bytes) = nixbase32::decode(without_prefix)
        && let Ok(arr) = <[u8; 32]>::try_from(bytes)
    {
        return Ok(arr);
    }
    // 3. SRI format (`sha256-<base64>`)
    if let Ok(NixHash::Sha256(arr)) = NixHash::from_sri(trimmed) {
        return Ok(arr);
    }

    Err(EvalError::InvalidSha256(raw.to_string()))
}

fn filename_from_url(url: &str) -> String {
    let without_query = url.split('?').next().unwrap_or(url);
    let last = without_query
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("source");
    sanitize_store_name(last)
}

fn sanitize_store_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.' | '_' | '?' | '=') {
                c
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = cleaned.trim_start_matches('.');
    if trimmed.is_empty() {
        "unnamed".to_string()
    } else {
        trimmed.to_string()
    }
}

fn sanitize_env_var(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn topo_sort_drvs(drvs: &BTreeMap<StorePath, Derivation>) -> Vec<StorePath> {
    let mut visited = BTreeSet::new();
    let mut order = Vec::with_capacity(drvs.len());

    fn dfs(
        path: &StorePath,
        drvs: &BTreeMap<StorePath, Derivation>,
        visited: &mut BTreeSet<StorePath>,
        order: &mut Vec<StorePath>,
    ) {
        if !visited.insert(path.clone()) {
            return;
        }
        if let Some(drv) = drvs.get(path) {
            for dep in drv.input_derivations().keys() {
                if drvs.contains_key(dep) {
                    dfs(dep, drvs, visited, order);
                }
            }
        }
        order.push(path.clone());
    }

    for path in drvs.keys() {
        dfs(path, drvs, &mut visited, &mut order);
    }

    order
}
