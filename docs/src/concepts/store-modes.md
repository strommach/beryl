# LocalStore vs. DaemonStore

All store operations in Beryl are abstracted behind the `Store` trait (`crates/beryl-store/src/store.rs`), supporting two interchangeable backends.

## Standalone `LocalStore` (Default)

`LocalStore` manages a self-contained Nix-compatible store rooted at `--store <dir>` (or `BERYL_STORE_DIR`, defaulting to `/nix`):

```text
<root>/
├── store/<hash>-<name>          # Read-only store objects
└── var/nix/
    ├── db/db.sqlite             # ValidPaths, Refs, DerivationOutputs
    ├── profiles/                # Atomic profile generations
    ├── gcroots/                 # Additional GC root symlinks
    └── tmp/                     # Staging directory for atomic renames
```

Use `--store /tmp/my-store` during development or on systems without Nix installed. No root privileges or `nix-daemon` are required.

## `DaemonStore` (`--daemon`)

When `--daemon` (or `BERYL_USE_DAEMON=1`) is passed, Beryl connects to `/nix/var/nix/daemon-socket/socket` (or `--daemon-socket` / `BERYL_DAEMON_SOCKET`) and speaks the `nix-daemon` worker protocol (v1.35):

- **Queries:** `IsValidPath`, `QueryPathInfo`, transitive `query_closure`.
- **Substitution & Uploads:** `EnsurePath`, framed `AddToStoreNar`.
- **Builds:** `AddTextToStore` (registers `.drv` ATerm files) and `BuildPaths` (`<drv>!out` executes in `nix-daemon`'s real sandbox).
- **GC Roots:** `AddIndirectRoot` registers user-space profile symlinks so `nix-collect-garbage` never deletes Beryl-installed closures.

`DaemonStore` lets unprivileged users share `/nix/store` safely alongside an existing multi-user Nix installation.
