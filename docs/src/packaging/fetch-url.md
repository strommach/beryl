# Fetching Sources (`beryl.fetch_url`)

`beryl.fetch_url` creates a Nix **Fixed-Output Derivation (FOD)** using `builder = "builtin:fetchurl"`.

## Usage

```lua
local src = beryl.fetch_url({
  url = "https://example.com/archive.tar.xz",
  sha256 = "05a89d6b3b283f22b4d78b915f22a...",
  name = "custom-name.tar.xz", -- optional; defaults to URL filename
})
```

## Supported `sha256` Formats

`beryl-eval` accepts SHA-256 hashes in any standard format:
- **Hex:** 64 hex characters (with or without `sha256:` prefix).
- **Nix Base32:** 52 Nix-base32 characters (with or without `sha256:` prefix).
- **SRI:** `sha256-<base64>`.

## Execution & Verification

- In **LocalStore mode**, `beryl-build` (`fetchurl.rs`) downloads the URL over HTTPS, computes the SHA-256 digest, fails with `FixedOutputHashMismatch` if it differs, and registers the file as a content-addressed `fixed:sha256:...` store object.
- In **`--daemon` mode**, `nix-daemon` executes `builtin:fetchurl` natively using its built-in fetcher.
