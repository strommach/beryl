# CLI Reference

## Global Flags

| Flag | Env Var | Default | Description |
| :--- | :--- | :--- | :--- |
| `--store <PATH>` | `BERYL_STORE_DIR` | `/nix` | Root directory for `LocalStore` (`store/` and `var/nix/`). |
| `--daemon` | `BERYL_USE_DAEMON` | `false` | Connect to `nix-daemon` instead of direct `LocalStore` access. |
| `--daemon-socket <PATH>` | `BERYL_DAEMON_SOCKET` | `/nix/var/nix/daemon-socket/socket` | Unix socket path for `nix-daemon`. |
| `--cache <URL>` | `BERYL_CACHE_URL` | `https://cache.nixos.org` | Binary cache base URL. |
| `--trusted-key <KEY>` | — | `cache.nixos.org-1:...` | Additional trusted Ed25519 public keys (`name:base64`). Repeatable. |
| `--profile <NAME>` | — | `default` | Profile name under `var/nix/profiles/`. |
| `--lockfile <PATH>` | — | `beryl.lock` | Path to the lockfile. |
| `--system <TRIPLE>` | — | Host triple | Target Nix system (e.g. `aarch64-linux`, `x86_64-linux`, `aarch64-darwin`). |

## Subcommands

### `beryl init [TEMPLATE]`
Scaffold a `beryl.lua` manifest from a built-in template.
- `--list`: List all available templates and aliases.
- `-o, --output <PATH>`: Output file path (default `beryl.lua`).
- `--force`: Overwrite an existing manifest.
- `--direnv`: Also generate a `.envrc` file.

### `beryl develop [SHELL]`
Enter a project dev shell defined in `beryl.lua` (defaults to `default`).
- `-f, --file <PATH>`: Manifest path (default `beryl.lua`).
- `--export`: Print POSIX `export ...` lines instead of spawning a shell.
- `-c, --command <CMD>`: Run a single command inside the dev shell.

### `beryl build <PATH>`
Evaluate a `pkg.lua`, fetch missing `nixpkgs` inputs, and build into the store.
- `--no-sandbox`: Disable Linux namespace `unshare` (local store mode).

### `beryl install <TARGET>`
Build or fetch `<TARGET>` (`nixpkgs#<attr>`, `/nix/store/...`, or `./pkg.lua`) and link it into the active profile generation.

### `beryl shell <PACKAGES...>`
Spawn a subshell with requested packages (`ripgrep`, `nixpkgs#ripgrep`, or `/nix/store/...`) on `PATH`/`CPATH`/`LIBRARY_PATH`/`LDFLAGS`/`PKG_CONFIG_PATH`.
- `-c, --command <CMD>`: Run a single command inside the shell.

### `beryl eval <PATH>`
Evaluate a `pkg.lua` into Nix ATerm `.drv` files.
- `--write`: Write evaluated `.drv` files into the local store.
- `--aterm`: Print the root derivation's raw ATerm representation.

### `beryl fetch <STORE_PATH>`
Fetch a store path and its full runtime closure from the binary cache.

### `beryl info <STORE_PATH>`
Query `NarSize`, `NarHash`, `Deriver`, `CA`, and `References` for a store path.

### `beryl lock <ATTRS...>`
Resolve and pin `nixpkgs` attributes into `beryl.lock`.
- `--channel <CHANNEL>`: Set `nixpkgs.channel` (e.g. `nixos-unstable`, `nixos-25.05`).
- `--rev <COMMIT>`: Pin a custom `nixpkgs` git commit.

### `beryl update [ATTRS...]`
Refresh pinned `nixpkgs` attributes in `beryl.lock`.
- `--channel <CHANNEL>`: Switch `nixpkgs.channel` and re-resolve pinned packages.
- `--rev <COMMIT>`: Pin a custom `nixpkgs` git commit and re-resolve pinned packages.

### `beryl rollback`
Atomically revert the active profile to the previous generation.

### `beryl gc`
Sweep unreferenced paths from the local store.
