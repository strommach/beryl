# Writing Derivations (`pkg.lua`)

A `pkg.lua` file evaluates to a `StoreRef` returned by `beryl.derivation({ ... })`.

## Example

```lua
local openssl = beryl.nixpkgs("openssl_3")
local zlib    = beryl.nixpkgs("zlib")
local gcc     = beryl.nixpkgs("gcc")

return beryl.derivation({
  name = "my-custom-curl",
  version = "8.7.1",

  src = beryl.fetch_url({
    url = "https://curl.se/download/curl-8.7.1.tar.xz",
    sha256 = "05a89d6b3b283f22b4d78b915f22a...",
  }),

  inputs = { openssl, zlib, gcc },

  build = [[
    tar -xf "$src" --strip-components=1
    ./configure --prefix="$out" --with-openssl="$openssl" --with-zlib="$zlib"
    make -j"$BERYL_BUILD_CORES"
    make install
  ]],
})
```

## `beryl.derivation(spec)` Fields

| Field | Type | Required | Description |
| :--- | :--- | :--- | :--- |
| `name` | `string` | Yes | Package name (combined with `version` as `<name>-<version>`). |
| `version` | `string` | No | Optional version suffix. |
| `src` | `StoreRef \| string` | No | Source path exported as `$src`. |
| `inputs` | `table` | No | Array or keyed table of `StoreRef`s (`beryl.nixpkgs(...)` or nested `beryl.derivation(...)`). |
| `build` | `string` | No | Shell script executed via `builder` with `-e -c <script>`. |
| `builder` | `string` | No | Executable path (defaults to `/bin/sh`). |
| `args` | `table` | No | Explicit argument array (overrides `build`). |
| `env` | `table` | No | Extra environment variables (`string`, `number`, `boolean`, or `StoreRef`). |
| `system` | `string` | No | Override target Nix system triple. |

## `StoreRef` Behavior in Lua

Values returned by `beryl.nixpkgs`, `beryl.fetch_url`, and `beryl.derivation` are `StoreRef` userdata:
- `tostring(ref)` or `"foo" .. ref` evaluates to the `/nix/store/<hash>-<name>` path.
- `ref.path` returns the output store path string.
- `ref.drv_path` returns the `.drv` path (or `nil` for prebuilt `nixpkgs` refs).
- `ref.name` returns the store path name.
- Array entries in `inputs` automatically export sanitized environment variables (e.g., `openssl_3` exports both `openssl_3` and `openssl`), plus `BERYL_INPUTS` containing all input paths.
