# Profiles, Rollbacks & Garbage Collection

Beryl separates the immutable store from user-facing environments using **symlink-farm profiles** and **mark-and-sweep garbage collection**.

## Atomic Profiles

Installing a package (`beryl install <target>`) creates a new numbered generation (`<profile>-<N>-link`) and atomically flips the active profile symlink (`<profile>`) via `rename(2)` over a temporary symlink.

- **LocalStore mode:** Profiles live at `<store>/var/nix/profiles/<profile>` (`default` by default, configurable with `--profile <name>`). Each generation is a content-addressed `beryl-env` store object containing a `beryl-manifest.json` and merged symlinks (`bin/`, `lib/`, `include/`, `share/`).
- **Daemon mode (`--daemon`):** Profiles live in user-writable `$XDG_STATE_HOME/beryl/profiles/` (falling back to `~/.local/state/beryl/profiles/`). Each installed store path gets a symlink in `.gcroots/` registered with `nix-daemon` via `AddIndirectRoot`.

## Rollbacks

```bash
beryl rollback
# or with nix-daemon:
beryl --daemon rollback
```

`rollback` locates the highest existing generation strictly below the active generation and atomically flips the active profile symlink back to it.

## Garbage Collection (`LocalStore`)

```bash
beryl gc
```

`collect_garbage` performs mark-and-sweep collection on `LocalStore`:
1. **Mark:** Collects all symlinks under `<store>/var/nix/profiles/` and `<store>/var/nix/gcroots/` and computes their transitive runtime closures via `Refs`.
2. **Sweep:** Deletes any store path in `ValidPaths` not in the live set from both SQLite (`ValidPaths`, `Refs`, `DerivationOutputs`) and `<store>/store/`.
