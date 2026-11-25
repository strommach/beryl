# Three-Tier Nixpkgs Resolution

Binary caches like `cache.nixos.org` are indexed by store-path hash, not by attribute name. To resolve `beryl.nixpkgs("openssl_3")` or `nixpkgs#ripgrep` without requiring Nix to be installed, `beryl-index` uses a three-tier resolution chain.

## Tier 1: `beryl.lock` (Zero Network, Zero Nix)

Beryl checks `beryl.lock` (configurable via `--lockfile`, default `./beryl.lock`) for `(system, attr)`:

```toml
version = 1

[nixpkgs]
channel = "nixos-unstable"

[packages."aarch64-linux"."ripgrep"]
store_path = "/nix/store/...-ripgrep-14.1.1"
source = "hydra"
```

If pinned, Beryl returns the store path immediately. Rebuilds never need an index or network lookup.

## Tier 2: Upstream Hydra Index + Cache Verification (Zero Nix)

When an attribute is unpinned (or during `beryl update`), and `nixpkgs.rev` is not pinned to a custom git commit:
1. Beryl queries Hydra's JSON job endpoint (`https://hydra.nixos.org/job/nixpkgs/trunk/<attr>.<system>/latest`).
2. It confirms `<hash>.narinfo` exists and passes Ed25519 signature verification on the configured binary cache.
3. It persists the resolved path into `beryl.lock` with `source = "hydra"`.

## Tier 3: `nix eval` Fallback

If the attribute is absent from the Hydra index—or if `nixpkgs.rev` in `beryl.lock` pins a custom `nixpkgs` git commit—Beryl checks for `nix` in `$PATH`, runs:

```bash
nix eval --raw <flake>#legacyPackages.<system>.<attr>.outPath
```

and persists the resulting store path into `beryl.lock` with `source = "nix-eval"`.

## Managing Pins

- `beryl lock <attrs...>`: Resolve and pin attributes into `beryl.lock`.
- `beryl update [attrs...]`: Bypass `beryl.lock` to refresh pinned attributes (or all pinned attributes for the target system if none are specified).
