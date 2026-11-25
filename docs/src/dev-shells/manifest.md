# Manifest Format (`beryl.lua` & `beryl.shell`)

A `beryl.lua` file is a project-level manifest (analogous to a Nix `flake.nix`) that defines named development shells and optional project packages.

## Example `beryl.lua`

```lua
local p = beryl.nixpkgs

return {
  description = "Beryl development environment",

  shells = {
    default = beryl.shell({
      packages = {
        p("rustc"), p("cargo"), p("rust-analyzer"), p("clippy"), p("rustfmt"),
        p("gcc"), p("clang"), p("cmake"), p("gnumake"), p("pkg-config"), p("gdb"),
      },
      libs = {
        p("lua5_4"), p("sqlite"), p("openssl"), p("xz"), p("zstd"),
      },
      env = {
        LIBCLANG_PATH = p("libclang.lib") .. "/lib",
        RUST_BACKTRACE = "1",
      },
      hook = [[
        echo "Entered Beryl dev shell ($(rustc --version))"
      ]],
    }),
  },
}
```

## `beryl.shell(spec)` Fields

| Field | Type | Description |
| :--- | :--- | :--- |
| `packages` | `StoreRef[]` | Tool packages whose `bin/` is prepended to `PATH` (and `lib/pkgconfig` / `share/pkgconfig` to `PKG_CONFIG_PATH`). |
| `libs` | `StoreRef[]` | Library packages whose `include/`, `lib/`, and `pkgconfig/` are wired into `CPATH`, `LIBRARY_PATH`, `LDFLAGS`, and `PKG_CONFIG_PATH`. |
| `env` | `table` | Custom environment variables (`string`, `number`, `boolean`, or `StoreRef`). Applied after standard variables so you can override defaults. |
| `hook` | `string` | Shell snippet executed on entry (sourced via `--rcfile` in interactive `bash`, or prepended to `-c <command>`). |

Beryl also sets `IN_BERYL_SHELL=1` inside every `beryl develop` environment.

## Running Commands in a Dev Shell

```bash
# Enter the `default` shell interactively
beryl develop

# Enter a named shell (e.g. `shells.ci`)
beryl develop ci

# Run a one-shot command inside the shell
beryl develop -c "cargo test --workspace"
```
