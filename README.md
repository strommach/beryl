# Beryl
**A Lua-driven, `/nix/store`-compatible package manager.**

Beryl implements the purely functional deployment model from Eelco Dolstra’s [The Purely Functional Software Deployment Model](https://edolstra.github.io/pubs/phd-thesis.pdf) (2006) while maintaining **100% binary and store compatibility with the Nix ecosystem**. You write clean, imperative-friendly build definitions in **Lua**, **Bash**, and **Makefiles**—and seamlessly pull prebuilt dependencies from `cache.nixos.org` without writing a single line of the Nix expression language.

## Core Architecture

### 1. Native `/nix/store` Compatibility
Beryl uses `/nix/store` directly rather than inventing a parallel directory hierarchy. Every artifact is stored at:
/nix/store/<32-char-nix-base32-hash>-<name>-<version>/


Because store paths match upstream Nix conventions byte-for-byte, compiled binaries (ELF on Linux, Mach-O on macOS) retain valid `RPATH`s, dynamic linker paths, and script shebangs—allowing Beryl to consume upstream `nixpkgs` binaries with zero binary patching.

### 2. Separation of Store & Profiles
* **Immutable Store (`/nix/store`):** Read-only, content/input-addressed artifacts. Multiple versions and build variants coexist without collision.
* **Atomic Profiles (`/nix/var/nix/profiles/...`):** User environments are symlink farms pointing into `/nix/store`. Installing, upgrading, or rolling back a generation is a single atomic symlink flip.

### 3. Lua Evaluation → Standard Derivations (`.drv`)
Beryl replaces lazy Nix expressions with Lua tables and hooks, compiling them down to standard Nix ATerm derivations (`.drv`):

* **Deterministic Input Hashing:** Computes Nix-compatible base32 SHA-256 hashes over all inputs, environment variables, builder paths, and transitive dependency closures.
* **Sandboxed Execution:** Builds run in isolated environments (Linux namespaces on Linux; Seatbelt/isolated directories on macOS) with stripped `PATH`, read-only store mounts, and network access blocked after source verification.
* **Reference Scanning:** Post-build scans automatically detect 32-character store hashes inside output binaries and scripts to register exact runtime closures.

### 4. Zero-Nix-Language Ecosystem Bridge
Beryl leverages the 100,000+ packages in `nixpkgs` without requiring you to write Nix code:

* **Direct Binary Cache Substitution:** Fetches `.narinfo` metadata, verifies Ed25519 cache signatures, and unpacks compressed **NAR (Nix Archive)** streams directly from `cache.nixos.org` or Cachix.
* **Opaque `nixpkgs` Dependencies:** Reference any upstream package inside Lua (`beryl.nixpkgs("openssl")`). Beryl resolves its store path and fetches the prebuilt binary so your Lua/Makefile builds can link against it immediately.
* **Daemon & SQLite Coexistence:** Designed to interoperate with `/nix/var/nix/db/db.sqlite` and `nix-daemon` so Beryl-built packages and Nix-installed packages share garbage collection roots cleanly.

## Package Definition (`pkg.lua`)
Pull dependencies straight from `nixpkgs` binaries while writing your own build logic in plain Lua, Bash, or Make:

-- Pull prebuilt binaries directly from cache.nixos.org (no compilation needed)
local openssl = beryl.nixpkgs("openssl_3")
local zlib    = beryl.nixpkgs("zlib")
local gcc     = beryl.nixpkgs("gcc")

-- Define a custom package in Lua
return beryl.derivation({
  name = "my-custom-curl",
  version = "8.7.1",

  src = beryl.fetch_url({
    url = "[https://curl.se/download/curl-8.7.1.tar.xz](https://curl.se/download/curl-8.7.1.tar.xz)",
    sha256 = "05a89d6b3b283f22b4d78b915f22a...",
  }),

  inputs = { openssl, zlib, gcc },

  -- Standard Bash or Makefile invocation
  build = [[
    tar -xf $src --strip-components=1 ./configure --prefix=$out --with-openssl=${openssl} --with-zlib=${zlib}
    make -j$BERYL_BUILD_CORES
    make install
  ]],
})

## CLI Workflow
Familiar, predictable commands modeled after modern package managers:
beryl build ./pkg.lua          # Evaluate Lua -> .drv, fetch substitutes, build into /nix/store
beryl install ./pkg.lua        # Build/fetch and link into active user profile
beryl install nixpkgs#ripgrep  # Fetch prebuilt binary directly from cache.nixos.org
beryl shell openssl zlib       # Spawn an isolated subshell with store paths on PATH/CPATH/LDFLAGS
beryl rollback                 # Atomically revert profile to previous generation
beryl gc                       # Sweep unreferenced paths from /nix/store

## Platform Support & Development Setup
* **Linux (`x86_64-linux`, `aarch64-linux`):** Full namespace sandboxing (`mount`, `pid`, `net`, `user`) and direct `/nix/store` management.
* **macOS (`aarch64-darwin`):** Runs natively using a dedicated APFS volume mounted at `/nix` via `/etc/synthetic.conf`, pulling `aarch64-darwin` binaries from `cache.nixos.org`.

## Roadmap
* [ ] **NAR & `.narinfo` Client:** HTTP fetcher, Ed25519 signature verifier, and NAR stream unpacker.
* [ ] **Nix Base32 & Hashing Engine:** Byte-for-byte compatible store path calculation (`sha256` + Nix base32 encoding).
* [ ] **Lua Evaluator:** Embed Lua 5.4 / LuaJIT to compile `pkg.lua` definitions into ATerm `.drv` files.
* [ ] **Store & Daemon Bridge:** Register valid paths in `/nix/var/nix/db/db.sqlite` or communicate via `nix-daemon` worker protocol.
* [ ] **Sandboxed Builder & Reference Scanner:** Isolated build execution and post-build runtime closure discovery.

## Stack
Rust:
- mlua (Lua Evaluator)
- reqwest & tokio (Cache HTTP & Stream)
- rusqlite (Store DB)
- nix & clap (Sandboxing & CLI
- zstd & xz2 (Decompression)
- nix-rs-compat (Nix Store & Cache)
