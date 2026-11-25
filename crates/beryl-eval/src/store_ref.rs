//! Lua `UserData` representing a Nix store path (prebuilt `nixpkgs` path, FOD, or derivation output).

use beryl_nix::store_path::StorePath;
use mlua::{MetaMethod, UserData, UserDataFields, UserDataMethods};

/// A reference to a Nix store path exposed to Lua scripts.
///
/// Converting a `StoreRef` to a string in Lua (via `tostring(x)` or `..`) yields
/// its logical `/nix/store/<hash>-<name>` path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreRef {
    /// Logical output store path (`/nix/store/<hash>-<name>`).
    pub out_path: StorePath,
    /// Associated `.drv` store path if this ref was produced by `beryl.fetch_url` or `beryl.derivation`.
    pub drv_path: Option<StorePath>,
    /// Friendly identifier (e.g. `"openssl_3"`, `"zlib"`, or derivation name) used for env vars.
    pub env_name: Option<String>,
}

impl StoreRef {
    pub fn prebuilt(out_path: StorePath, attr_name: impl Into<String>) -> Self {
        Self {
            out_path,
            drv_path: None,
            env_name: Some(attr_name.into()),
        }
    }

    pub fn from_derivation(
        out_path: StorePath,
        drv_path: StorePath,
        name: impl Into<String>,
    ) -> Self {
        Self {
            out_path,
            drv_path: Some(drv_path),
            env_name: Some(name.into()),
        }
    }
}

impl UserData for StoreRef {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("path", |_, this| Ok(this.out_path.to_absolute_path()));
        fields.add_field_method_get("drv_path", |_, this| {
            Ok(this.drv_path.as_ref().map(|p| p.to_absolute_path()))
        });
        fields.add_field_method_get("name", |_, this| Ok(this.out_path.name().to_string()));
    }

    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_meta_method(MetaMethod::ToString, |_, this, ()| {
            Ok(this.out_path.to_absolute_path())
        });
        methods.add_meta_function(
            MetaMethod::Concat,
            |_, (lhs, rhs): (mlua::Value, mlua::Value)| {
                let to_str = |v: mlua::Value| -> mlua::Result<String> {
                    match v {
                        mlua::Value::String(s) => Ok(s.to_str()?.to_string()),
                        mlua::Value::UserData(ud) => {
                            let r = ud.borrow::<StoreRef>()?;
                            Ok(r.out_path.to_absolute_path())
                        }
                        other => Err(mlua::Error::RuntimeError(format!(
                            "cannot concatenate {} with StoreRef",
                            other.type_name()
                        ))),
                    }
                };
                Ok(format!("{}{}", to_str(lhs)?, to_str(rhs)?))
            },
        );
    }
}
