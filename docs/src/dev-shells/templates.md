# Project Templates (`beryl init`)

`beryl init` scaffolds a ready-to-use `beryl.lua` (and optionally a `.envrc`) for common language stacks.

## Listing Templates

```bash
beryl init --list
```

## Built-In Templates

| Template | Aliases | Included Packages |
| :--- | :--- | :--- |
| `minimal` | `default`, `basic` | `git`, `ripgrep` |
| `rust` | `rs` | `rustc`, `cargo`, `rust-analyzer`, `clippy`, `rustfmt`, `pkg-config`, `gcc`, `openssl` |
| `c` | — | `gcc`, `clang`, `gnumake`, `cmake`, `pkg-config`, `gdb`, `clang-tools` |
| `cpp` | `c++`, `cxx` | `gcc`, `clang`, `cmake`, `ninja`, `gnumake`, `pkg-config`, `gdb`, `clang-tools` |
| `python` | `py` | `python3`, `uv`, `ruff`, `pyright` |
| `node` | `js`, `ts`, `typescript` | `nodejs`, `pnpm`, `typescript`, `typescript-language-server` |
| `web` | `frontend` | `nodejs`, `pnpm`, `typescript`, `typescript-language-server`, `tailwindcss`, `esbuild` |
| `java` | `jvm` | `jdk`, `maven`, `gradle`, `jdt-language-server` (`JAVA_HOME` wired) |
| `go` | `golang` | `go`, `gopls`, `golangci-lint` |

## Usage

```bash
# Create beryl.lua for a Rust project
beryl init rust

# Create beryl.lua + .envrc for direnv
beryl init cpp --direnv

# Write to a custom path or overwrite an existing file
beryl init python -o custom.lua --force
```
