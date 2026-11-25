# Store Compatibility & Hashing

Beryl is **byte-for-byte compatible** with the Nix store format via `nix-rs-compat` (wrapped by `beryl-nix`).

## Store Paths

Every artifact lives at:

```text
/nix/store/<32-char-nix-base32-hash>-<name>
```

Nix base32 uses the 32-character alphabet `0123456789abcdfghijklmnpqrsvwxyz` (omitting `e`, `o`, `u`, `t`). Because Beryl computes identical hashes and paths to C++ Nix, prebuilt ELF/Mach-O binaries retain valid `RPATH`s, dynamic linker paths, and script shebangs with zero binary patching.

## NAR (Nix Archive)

Beryl dumps and unpacks canonical NAR streams (`beryl_nix::nar_fs`):
- Directory entries are sorted lexicographically by raw byte order.
- Regular file executable bits (`0o755` vs `0o644`) and symlink targets are preserved.
- Path traversal entries (`.`, `..`, `/`, NUL) are rejected during unpacking.
- Single-pass SHA-256 hashing (`hash_nar_path`) matches `nix hash path --type sha256 --base32` byte-for-byte.

## Binary Cache Verification (`.narinfo`)

When fetching from `https://cache.nixos.org` (or custom caches via `--cache` and `--trusted-key`), Beryl:
1. Downloads `<hash>.narinfo`.
2. Verifies its Ed25519 signature against trusted public keys (built-in `cache.nixos.org-1:6NCHdD59X431o0gWypbMrAURkbJ16ZPMQFGspcDShjY=` plus any `--trusted-key` flags).
3. Streams and decompresses the NAR (`xz`, `zstd`, or `none`).
4. Verifies the unpacked NAR's SHA-256 digest and byte size before publishing it into the store.

## ATerm Derivations (`.drv`)

Lua definitions compile to standard Nix ATerm derivations (`Derive([...], [...], [...], ...)`), including recursive `hashDerivationModulo` calculation over input derivations and fixed-output hashes. Emitted `.drv` files are valid inputs to both Beryl's builder and C++ `nix-daemon`.
