# What's new in pgbg

Every change you can see or use, newest first.

Each entry says what changed, then where, then how to use it.

## Unreleased

### New

- **`pgbg connections`** lists the connections of your `config.toml`: name, tier, URL, and how pgbg reaches each one.
  Add `--json` for a script.
  A plain password, in a `password` key or in a URL, is refused: name a keychain item, an environment variable or a command instead.
