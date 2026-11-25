# Installation & Building

## Prerequisites

Beryl is written in Rust (edition 2024) and embeds Lua 5.4 via `mlua`.

If you have Nix with flakes enabled, enter the included development shell from the repository root:

```bash
nix develop
```

The `flake.nix` dev shell provides `rustc`, `cargo`, `gcc`, `clang`, `cmake`, `gnumake`, `pkg-config`, `lua5_4`, `sqlite`, `openssl`, `xz`, and `zstd`.

## Building from Source

```bash
cargo build --release
```

The compiled binary will be at `target/release/beryl`. You can install it to `~/.cargo/bin`:

```bash
cargo install --path crates/beryl-cli
```

## Running the Test Suite

```bash
cargo test --workspace
```

> **Note:** Tests that exercise `DaemonStore` and `nix-daemon` builds (`daemon_store_tests` and `daemon_build_tests`) automatically detect `/nix/var/nix/daemon-socket/socket` and skip cleanly if no `nix-daemon` is running on the host.
