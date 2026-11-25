# Quick Start

This guide walks through installing a prebuilt package from `cache.nixos.org`, creating a project dev shell, and building a custom Lua package.

## 1. Install a Prebuilt `nixpkgs` Package

Using a standalone local store (no root or `nix-daemon` required):

```bash
beryl --store /tmp/my-nix install nixpkgs#ripgrep
```

Beryl will:
1. Resolve `ripgrep` for your host system (e.g. `aarch64-linux` or `x86_64-linux`) and pin it in `beryl.lock`.
2. Download and Ed25519-verify the `.narinfo` and NAR closure from `cache.nixos.org`.
3. Link the package into `/tmp/my-nix/var/nix/profiles/default`.

If you already have a multi-user `nix-daemon` running on your machine, pass `--daemon` instead:

```bash
beryl --daemon install nixpkgs#ripgrep
```

## 2. Create a Project Dev Shell

Scaffold a `beryl.lua` for your language (e.g., Rust, C++, Python, Node, Go, Java, Web):

```bash
beryl init rust --direnv
beryl develop
```

Inside the dev shell, `PATH`, `CPATH`, `LIBRARY_PATH`, `PKG_CONFIG_PATH`, `LDFLAGS`, and custom `env` variables are wired automatically.

## 3. Build a Custom `pkg.lua`

Create `pkg.lua`:

```lua
return beryl.derivation({
  name = "hello-beryl",
  version = "0.1.0",
  build = [[
    mkdir -p "$out/bin"
    printf '#!/bin/sh\necho "Hello from Beryl!"\n' > "$out/bin/hello-beryl"
    chmod +x "$out/bin/hello-beryl"
  ]],
})
```

Build and run it:

```bash
beryl --store /tmp/my-nix install ./pkg.lua
/tmp/my-nix/var/nix/profiles/default/bin/hello-beryl
```
