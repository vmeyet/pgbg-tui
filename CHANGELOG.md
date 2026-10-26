# What's new in pgbg

Every change you can see or use, newest first.

Each entry says what changed, then where, then how to use it.

## Unreleased

### New

- **`pgbg password set <connection>`** saves a connection's password in the macOS keychain.
  Type it at the hidden prompt, or pipe it in: `pass shop | pgbg password set prod`; no flag takes a password, so `ps` never shows one.
  A connection with no password yet gets `password = { keychain = "pgbg/<name>" }` in your `config.toml`, the rest of the file as you wrote it.
- **The TUI**: `pgbg` opens it on the first connection of your `config.toml`, `pgbg tui <connection>` on the one you name.
  Its frame takes the colour of the connection's tier: green for local, amber for staging, red for production.
  Type a statement in the editor and press `⏎` to run it; `⌥⏎` breaks the line, `ctrl-c` quits.
  With several statements in the editor, `⏎` runs only the one under the cursor, split on `;` outside strings and comments.
  `esc` cancels a running statement on the server.
  When the server refuses a statement, the editor marks the character its error points at, until you edit the text.
- **The keys of the TUI** on one screen: press `F1`, or `?` in the results grid, to list the keys of the pane you are in; press `?` again for every key.
  The footer shows the keys of the pane that has them.
- **The results grid** in the TUI: numbers line up on the right, `▶` marks the current row, and a control character in a value shows escaped.
  Press `tab` to give it the keys, then `j`/`k`, `PgUp`/`PgDn` and `g`/`G` to scroll its rows, `h`/`l` its columns; `tab` again goes back to the editor.
- **`pgbg run <connection> "<sql>"`** runs one statement and prints its result as a table, with the row count.
  Add `--json` for a script: `{"columns": [...], "rows": [[...]]}`.
  A connection is read only unless it says `writes = "allowed"`: a statement that is not a read is refused before it is sent, and every read runs in a read-only transaction that is rolled back.
  The password comes from `password = { keychain = "..." }`, `{ env = "..." }` or `{ command = [...] }`.
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
- **Dates and timestamps of `infinity` show as `infinity`** in `pgbg run` and the TUI, instead of failing the whole statement; a value out of range shows as `<unsupported>` in its own cell.
- **Floats that are not numbers print as PostgreSQL spells them**: `NaN`, `Infinity` and `-Infinity`, in the table and in `--json`, where they were `null`.
- **A column of a domain type shows its values**, decoded like the type the domain is built on, and a `void` result such as `pg_sleep` shows `NULL`.
- **Wide characters keep `pgbg run` columns aligned**: a cell with `日本` or an emoji takes the room it takes on screen.
- **A `--` comment ends at a carriage return**, as PostgreSQL ends it, so text after it is checked like the rest of the statement.
