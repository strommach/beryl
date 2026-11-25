# `direnv` Integration

`beryl develop --export` prints POSIX-compatible `export KEY='VALUE'` statements (plus any `hook` script) to stdout instead of spawning a subshell.

## Manual Shell Evaluation

```bash
eval "$(beryl develop --export)"
# or with nix-daemon:
eval "$(beryl --daemon develop --export)"
```

## Automatic Activation with `direnv`

Generate a `.envrc` automatically when initializing a project:

```bash
beryl init rust --direnv
direnv allow
```

Or create `.envrc` manually:

```sh
watch_file beryl.lua beryl.lock
eval "$(beryl develop --export)"
```

Whenever `beryl.lua` or `beryl.lock` changes, `direnv` re-runs `beryl develop --export` and updates your current shell environment without nesting subshells.
