# Architecture & Workspace Crates

Beryl is organized as a 6-crate Cargo workspace so each layer can be tested independently against real Nix outputs:

```text
beryl/
├── Cargo.toml                  # Workspace root + shared dependencies
└── crates/
    ├── beryl-nix/              # Nix primitives via `nix-rs-compat`
    ├── beryl-store/            # Store trait, LocalStore, DaemonStore, BinaryCacheClient, profiles, GC
    ├── beryl-index/            # beryl.lock + three-tier nixpkgs resolver
    ├── beryl-eval/             # Lua 5.4 (mlua) evaluator for pkg.lua and beryl.lua
    ├── beryl-build/            # Sandboxed builder, builtin:fetchurl, daemon builder, ref scanner
    └── beryl-cli/              # clap CLI (`beryl`), `develop` runner, and `init` templates
```

## Crate Responsibilities

- **`beryl-nix`** (`crates/beryl-nix`): Wraps `nix-rs-compat` for store path validation, Nix base32 encoding, streaming NAR `unpack_nar` / `dump_nar` / `hash_nar_path`, `.narinfo` parsing + Ed25519 verification, and ATerm `.drv` construction.
- **`beryl-store`** (`crates/beryl-store`): Defines the async `Store` trait implemented by `LocalStore` (SQLite `ValidPaths`, `Refs`, `DerivationOutputs` + read-only store filesystem) and `DaemonStore` (`nix-daemon` worker protocol v1.35 client). Also provides `BinaryCacheClient`, `ProfileManager`, `DaemonProfileManager`, and mark-and-sweep `collect_garbage`.
- **`beryl-index`** (`crates/beryl-index`): Manages `beryl.lock` (TOML) and the three-tier `NixpkgsResolver` (`beryl.lock` → Hydra JSON + binary cache `.narinfo` check → `nix eval --raw` fallback).
- **`beryl-eval`** (`crates/beryl-eval`): Embeds Lua 5.4 via `mlua`, exposes `beryl.nixpkgs`, `beryl.fetch_url`, `beryl.derivation`, and `beryl.shell`, and compiles `pkg.lua` / `beryl.lua` into verified `.drv` DAGs and `ProjectManifest`s.
- **`beryl-build`** (`crates/beryl-build`): Executes `builtin:fetchurl` fixed-output derivations, runs local builds with `env_clear()` and best-effort Linux `CLONE_NEWUSER | CLONE_NEWNET` namespaces, scans outputs with Aho-Corasick for runtime `Refs`, and supports `realise_eval_via_daemon` for `nix-daemon` builds.
- **`beryl-cli`** (`crates/beryl-cli`): Exposes the `beryl` binary with `--store` and `--daemon` modes, `beryl develop`, and `beryl init`.
