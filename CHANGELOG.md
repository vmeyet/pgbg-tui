# What's new in pgbg

Every change you can see or use, newest first.

Each entry says what changed, then where, then how to use it.

## Unreleased

### New

- **The TUI**: `pgbg` opens it on the first connection of your `config.toml`, `pgbg tui <connection>` on the one you name.
  Its frame takes the colour of the connection's tier: green for local, amber for staging, red for production.
  Type a statement in the editor and press `⏎` to run it; `⌥⏎` breaks the line, `ctrl-c` quits.
  With several statements in the editor, `⏎` runs only the one under the cursor, split on `;` outside strings and comments.
  `esc` cancels a running statement on the server.
  When the server refuses a statement, the editor marks the character its error points at, until you edit the text.
- **The results grid** in the TUI: numbers line up on the right, `▶` marks the current row, and a control character in a value shows escaped.
  Press `tab` to give it the keys, then `j`/`k`, `PgUp`/`PgDn` and `g`/`G` to scroll its rows, `h`/`l` its columns; `tab` again goes back to the editor.
- **`pgbg run <connection> "<sql>"`** runs one statement and prints its result as a table, with the row count.
  Add `--json` for a script: `{"columns": [...], "rows": [[...]]}`.
  A connection is read only unless it says `writes = "allowed"`: a statement that is not a read is refused before it is sent, and every read runs in a read-only transaction that is rolled back.
  The password comes from `password = { env = "..." }` or `{ command = [...] }`; `keychain` comes later.
- **`pgbg connections`** lists the connections of your `config.toml`: name, tier, URL, and how pgbg reaches each one.
  Add `--json` for a script.
  A plain password, in a `password` key or in a URL, is refused: name a keychain item, an environment variable or a command instead.
- **TLS on every remote connection**: a connection whose host is not this machine needs `sslmode=require` (or `verify-ca`, `verify-full`) in its `url`, and pgbg checks the server's certificate.
  Set `tls = "off"` on the connection to talk to it in clear on purpose; a local host talks in clear unless its `url` asks for TLS.

### Fixed

- **No more clear text through `hostaddr`**: a `url` with `hostaddr` is refused, since it sent a connection to `localhost` on to another machine without TLS.
  Name the host in the `url` instead; pgbg keeps `hostaddr` for its tunnels.
- **A password never shows in a config error**: a `password` written as a number, a boolean or a list is refused like a plain one, and a broken line is reported by its number only.
  Fix the line the error names in your `config.toml`.
- **Values cannot talk to your terminal**: `pgbg run` prints a newline, a tab or an escape code inside a value as `\n`, `\t` or `\u{1b}`.
  Use `--json` to get the exact text.
- **A `--` comment ends at a carriage return**, as PostgreSQL ends it, so text after it is checked like the rest of the statement.
