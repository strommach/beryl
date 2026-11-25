# Sandboxing & Reference Scanning

## Build Isolation (`LocalStore` Mode)

When building a derivation locally (`crates/beryl-build/src/builder.rs`):
1. Missing `input_sources` (e.g. `nixpkgs` dependencies) are fetched from the binary cache first.
2. Derivations are built in topological order.
3. Each build runs inside an isolated temporary directory (`<store>/var/nix/tmp/...`) with `env_clear()`, setting only derivation `env` variables plus:
   - `out` (staging output path)
   - `HOME`, `TMPDIR`, `NIX_BUILD_TOP`
   - `BERYL_BUILD_CORES`
   - `PATH`, `CPATH`, `LIBRARY_PATH`, `LDFLAGS`, `PKG_CONFIG_PATH` derived from `inputs`.
4. **Linux namespaces:** On Linux (`SandboxMode::Auto`), the builder invokes `unshare(CLONE_NEWUSER | CLONE_NEWNET)` in `pre_exec` to isolate user and network namespaces when permitted by the host kernel. Pass `--no-sandbox` to `beryl build` to disable namespace `unshare`.

> When `--daemon` is passed, `nix-daemon` executes the build inside Nix's full chroot/mount-namespace sandbox.

## Post-Build Reference Scanning

After a build succeeds (`crates/beryl-build/src/scanner.rs`), Beryl scans every file and symlink target in `$out` using an **Aho-Corasick** automaton over the 32-character Nix base32 digests of the transitive input closure (plus `$out` itself).

- Reads files in 64 KiB chunks with a 31-byte overlap window so hashes spanning chunk boundaries are always detected.
- Records exact runtime `Refs` in SQLite so `beryl gc` and `query_closure` preserve every runtime dependency.
