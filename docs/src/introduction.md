# Introduction

**Beryl** is a Lua-driven, `/nix/store`-compatible package manager and development-environment tool written in Rust.

It implements the purely functional deployment model from Eelco Dolstra’s *The Purely Functional Software Deployment Model* (2006) while maintaining **binary and store compatibility with the Nix ecosystem**:

- **Zero Nix expression language required:** Write package definitions (`pkg.lua`) and project dev-shell manifests (`beryl.lua`) in clean **Lua 5.4**, **Bash**, and **Make**.
- **100% `/nix/store` compatibility:** Store paths use Nix base32 SHA-256 hashes (`/nix/store/<32-char-hash>-<name>`), standard NAR archives, Ed25519 `.narinfo` signatures, and standard ATerm `.drv` derivations.
- **Direct `nixpkgs` binary cache access:** Pull prebuilt binaries and closures directly from `https://cache.nixos.org` (or custom caches) without needing Nix installed.
- **Dual store backends:** Run standalone with an unprivileged SQLite `LocalStore` (`--store <dir>`), or talk directly to an existing multi-user `nix-daemon` (`--daemon`).
- **Flake-like dev shells:** Scaffold multi-language environments with `beryl init` and enter isolated dev shells with `beryl develop` (including `--export` for `direnv`).
