# Beryl
**A Lua-driven, `/nix/store`-compatible package manager and dev-shell tool.**

Beryl implements the purely functional deployment model from Eelco Dolstra’s [The Purely Functional Software Deployment Model](https://edolstra.github.io/pubs/phd-thesis.pdf) (2006) while maintaining **100% binary and store compatibility with the Nix ecosystem**. You write clean, imperative-friendly build definitions and dev-shell manifests in **Lua 5.4**, **Bash**, and **Makefiles**—and seamlessly pull prebuilt dependencies from `cache.nixos.org` without writing a single line of the Nix expression language.

Full documentation lives in [`docs/`](docs/src/SUMMARY.md) (build with `mdbook build docs`).

## Core Architecture

### 1. Native `/nix/store` Compatibility
Beryl uses `/nix/store` conventions directly rather than inventing a parallel directory hierarchy. Every artifact is stored at:
```text
/nix/store/<32-char-nix-base32-hash>-<name>-<version>/
```

Because store paths match upstream Nix conventions byte-for-byte, compiled binaries (ELF on Linux, Mach-O on macOS) retain valid `RPATH`s, dynamic linker paths, and script shebangs—allowing Beryl to consume upstream `nixpkgs` binaries with zero binary patching.

### 2. Dual Store Modes (`LocalStore` & `DaemonStore`)
* **Standalone `LocalStore` (`--store <dir>`):** Self-contained SQLite (`ValidPaths`, `Refs`, `DerivationOutputs`) + read-only store filesystem. Runs completely unprivileged with zero Nix installation required.
* **`DaemonStore` (`--daemon`):** Speaks the `nix-daemon` worker protocol over `/nix/var/nix/daemon-socket/socket` (`EnsurePath`, `AddTextToStore`, `BuildPaths`, `AddIndirectRoot`, `QueryPathInfo`) for unprivileged coexistence with an existing multi-user Nix installation.

### 3. Atomic Profiles & Garbage Collection
* **Atomic Profiles:** User environments are symlink farms pointing into the store. Installing, upgrading, or rolling back a generation is a single atomic symlink flip.
* **Mark-and-Sweep GC:** `beryl gc` sweeps unreferenced paths in `LocalStore`; `--daemon` mode registers indirect GC roots with `nix-daemon` so `nix-collect-garbage` preserves Beryl profiles.

### 4. Lua Evaluation → Standard Derivations (`.drv`)
Beryl replaces lazy Nix expressions with Lua tables and hooks, compiling them down to standard Nix ATerm derivations (`.drv`):

* **Deterministic Input Hashing:** Computes Nix-compatible base32 SHA-256 hashes over all inputs, environment variables, builder paths, and transitive dependency closures (`hashDerivationModulo`).
* **Sandboxed Execution:** Local builds run in isolated directories with stripped environments and best-effort Linux `CLONE_NEWUSER | CLONE_NEWNET` namespaces (or Nix's full sandbox when using `--daemon`).
* **Reference Scanning:** Post-build Aho-Corasick scans automatically detect 32-character store hashes inside output binaries, scripts, and symlinks to register exact runtime closures.

### 5. Zero-Nix-Language Ecosystem Bridge
Beryl leverages `nixpkgs` without requiring you to write Nix code via a three-tier resolution chain:
1. **`beryl.lock`:** Instant offline resolution for pinned `(system, attr)` pairs.
2. **Upstream Hydra Index + Cache Verification:** Queries Hydra's JSON endpoint and verifies Ed25519 `.narinfo` signatures on `cache.nixos.org` (zero Nix required).
3. **`nix eval` Fallback:** Falls back to `nix eval --raw` when `nix` is in `$PATH` for unindexed attributes or custom pinned `nixpkgs` commits.

## Package Definition (`pkg.lua`)
Pull dependencies straight from `nixpkgs` binaries while writing your own build logic in plain Lua, Bash, or Make:

```lua
-- Pull prebuilt binaries directly from cache.nixos.org (no compilation needed)
local openssl = beryl.nixpkgs("openssl_3")
local zlib    = beryl.nixpkgs("zlib")
local gcc     = beryl.nixpkgs("gcc")

-- Define a custom package in Lua
return beryl.derivation({
  name = "my-custom-curl",
  version = "8.7.1",

  src = beryl.fetch_url({
    url = "https://curl.se/download/curl-8.7.1.tar.xz",
    sha256 = "05a89d6b3b283f22b4d78b915f22a...",
  }),

  inputs = { openssl, zlib, gcc },

  -- Standard Bash or Makefile invocation
  build = [[
    tar -xf "$src" --strip-components=1
    ./configure --prefix="$out" --with-openssl="$openssl" --with-zlib="$zlib"
    make -j"$BERYL_BUILD_CORES"
    make install
  ]],
})
```

## Project Dev Shells (`beryl.lua`)
Define flake-like development environments in `beryl.lua` or scaffold one with `beryl init`:

```lua
local p = beryl.nixpkgs

return {
  description = "Rust project",
  shells = {
    default = beryl.shell({
      packages = { p("rustc"), p("cargo"), p("rust-analyzer"), p("clippy"), p("rustfmt") },
      libs = { p("openssl") },
      env = { RUST_BACKTRACE = "1" },
      hook = [[ echo "Rust dev shell ready ($(rustc --version))" ]],
    }),
  },
}
```

## CLI Workflow
```bash
# Project dev shells & scaffolds
beryl init rust --direnv          # Scaffold beryl.lua (+ .envrc) from templates (rust, c, cpp, python, node, web, java, go, minimal)
beryl develop                     # Enter the default dev shell from beryl.lua
beryl develop --export            # Print POSIX `export ...` lines for direnv / eval

# Building & installing packages
beryl build ./pkg.lua             # Evaluate Lua -> .drv, fetch substitutes, build into store
beryl install ./pkg.lua           # Build/fetch and link into active user profile
beryl install nixpkgs#ripgrep     # Fetch prebuilt binary directly from cache.nixos.org
beryl --daemon install ./pkg.lua  # Build and install via running nix-daemon

# Environment, lockfile & store management
beryl shell openssl zlib          # Spawn a subshell with store paths on PATH/CPATH/LIBRARY_PATH/LDFLAGS
beryl lock ripgrep fd             # Pin nixpkgs attributes into beryl.lock
beryl update                      # Refresh pinned attributes in beryl.lock
beryl eval ./pkg.lua --aterm      # Inspect generated ATerm .drv
beryl fetch /nix/store/...        # Fetch a store path closure from binary cache
beryl info /nix/store/...         # Inspect NarSize, NarHash, Deriver, and References
beryl rollback                    # Atomically revert profile to previous generation
beryl gc                          # Sweep unreferenced paths from LocalStore
```

## Roadmap
* [x] **NAR & `.narinfo` Client:** HTTP fetcher, Ed25519 signature verifier, and NAR stream unpacker.
* [x] **Nix Base32 & Hashing Engine:** Byte-for-byte compatible store path calculation (`sha256` + Nix base32 encoding).
* [x] **Lua Evaluator:** Embed Lua 5.4 (`mlua`) to compile `pkg.lua` and `beryl.lua` definitions into ATerm `.drv` files and dev shells.
* [x] **Store & Daemon Bridge:** Register valid paths in `var/nix/db/db.sqlite` (`LocalStore`) and communicate via `nix-daemon` worker protocol (`DaemonStore`).
* [x] **Sandboxed Builder & Reference Scanner:** Isolated build execution, `builtin:fetchurl` FODs, and Aho-Corasick runtime closure discovery.
* [x] **Project Dev Shells & Templates:** `beryl.lua`, `beryl develop` (`--export` for `direnv`), and `beryl init` language scaffolds.
* [ ] **macOS Sandbox & APFS Store Support:** Seatbelt/isolated build execution on `aarch64-darwin`.

## Stack
Rust workspace (`beryl-nix`, `beryl-store`, `beryl-index`, `beryl-eval`, `beryl-build`, `beryl-cli`):
- `nix-rs-compat` (Nix Store, NAR, `.narinfo`, ATerm `.drv`, Base32)
- `mlua` (Lua 5.4 Evaluator)
- `reqwest` & `tokio` (Cache HTTP & Async Runtime)
- `rusqlite` (Local Store DB)
- `nix` & `clap` (Linux Namespaces & CLI)
- `zstd` & `xz2` (NAR Decompression)
- `aho-corasick` (Post-build Reference Scanner)
