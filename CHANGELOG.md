# What's new in pgbg

Every change you can see or use, newest first.

Each entry says what changed, then where, then how to use it.

## Unreleased

### New

- **Saved queries** in the TUI: put `.sql` files under `.pgbg/queries/`, in folders if you like, at the top of your project.
  Press `ctrl-p`, type a few letters of a query's path, and `⏎` opens it in the editor; `↑`/`↓` pick another match, `esc` closes the finder.
  pgbg finds the `.pgbg/queries/` nearest the folder you start it in, as git finds `.git`.
  Queries of your own, for every project, go under `pgbg/queries/` in your config folder; the finder marks them `user`.
  The search stops at your home folder or at the top of your repository, where `.git` is; a queries folder that is a link, or that another user owns, is skipped.
  Only a plain file of 1 MiB at most opens: a pipe, a link or a dump says why it does not.
- **Tunnels** for `pgbg run` and the TUI: a connection with `tunnel = { command = [...] }` starts that command on a free local port, put in place of `{port}`, and connects through it.
  TLS still checks the certificate against the host in `url`, so `sslmode=require` works through a tunnel; `before = [...]` runs once first.
  A tunnel that dies says how and quotes its last words; it stops with pgbg, along with any `ssh` or `gcloud` it started, even when pgbg crashes.
  The TUI's status line shows `⇄ tunnel :<port>`; when the tunnel dies, its session closes, and `⏎` starts both again.
- **A statement timeout on every session**, set by the connection's tier: none on local, a minute on staging, 30 seconds on production.
  The server cancels a statement that runs longer; set `statement_timeout = "2min"` on a connection to choose its own, `"0s"` for none.
- **`pgbg connection add|edit|remove`** writes `config.toml` for you, one key at a time, keeping your comments and order.
  `add <name> --tier <tier> --url <url>` takes `--tunnel "<command>"`, one of `--password-env`, `--password-command "<command>"` or `--keychain`, and `--writes allowed`; a command splits as a shell would, so quote a word with spaces: `--tunnel "ssh -N -o 'ProxyCommand ssh -W %h:%p jump' -L 127.0.0.1:{port}:db.example.com:5432 bastion"`; `edit` changes only the flags you give.
  An error names the flag it is about, and `edit` can fix a config that does not load.
  `edit --keychain` deletes the item it replaces, unless another connection names it.
  `remove` deletes the connection's keychain item too; add `--keep-password` to leave it, or to remove a connection from a config that does not load.
- **`pgbg password set <connection>`** saves a connection's password in the macOS keychain.
  Type it at the hidden prompt, or pipe it in: `pass shop | pgbg password set prod`; no flag takes a password, so `ps` never shows one.
  A connection with no password yet gets `password = { keychain = "pgbg/<name>" }` in your `config.toml`, the rest of the file as you wrote it.
- **The TUI**: `pgbg` opens it on the first connection of your `config.toml`, `pgbg tui <connection>` on the one you name.
  Its frame takes the colour of the connection's tier: green for local, amber for staging, red for production.
  Type a statement in the editor and press `⏎` to run it; `⌥⏎` breaks the line, `ctrl-c` quits.
  With several statements in the editor, `⏎` runs only the one under the cursor, split on `;` outside strings and comments.
  `esc` cancels a running statement on the server.
  When the server refuses a statement, the editor marks the character its error points at, until you edit the text.
  The editor colours SQL as you type: keywords, strings, numbers and comments each in their own colour.
- **The keys of the TUI** on one screen: press `F1`, or `?` in the results grid, to list the keys of the pane you are in; press `?` again for every key.
  The footer shows the keys of the pane that has them.
- **The results grid** in the TUI: numbers line up on the right, `▶` marks the current row, and a control character in a value shows escaped.
  Press `tab` to give it the keys, then `j`/`k`, `PgUp`/`PgDn` and `g`/`G` to scroll its rows, `h`/`l` its columns; `tab` again goes back to the editor.
- **`pgbg run <connection> "<sql>"`** runs one statement and prints its result as a table, with the row count.
  Add `--json` for a script: `{"columns": [...], "rows": [[...]], "is_cut": false}`; `is_cut` is `true` when only the first 10 000 rows came back.
  A connection is read only unless it says `writes = "allowed"`: a statement that is not a read is refused before it is sent, and every read runs in a read-only transaction that is rolled back.
  The password comes from `password = { keychain = "..." }`, `{ env = "..." }` or `{ command = [...] }`.
- **`pgbg connections`** lists the connections of your `config.toml`: name, tier, URL, and how pgbg reaches each one.
  Add `--json` for a script.
  A plain password, in a `password` key or in a URL, is refused: name a keychain item, an environment variable or a command instead.
- **TLS on every remote connection**: a connection whose host is not this machine needs `sslmode=require` (or `verify-ca`, `verify-full`) in its `url`, and pgbg checks the server's certificate.
  Set `tls = "off"` on the connection to talk to it in clear on purpose; a local host talks in clear unless its `url` asks for TLS.

### Fixed

- **`esc` cancels only the statement you see running** in the TUI: a cancel that arrives late never stops the next statement, and `esc` while the session is still opening keeps the statement from the server.
- **`ctrl-c` cancels the running statement before pgbg quits**, so the server does not run it to its end.
- **A lost session shows at once** in the TUI's status line, even when nothing runs; the next `⏎` opens a new one.
- **The error mark stays off when you edit while a statement runs**, instead of marking the wrong character.
- **A huge value no longer slows the results grid**: a cell formats only the characters it can show, so a 10 MB JSON value draws as fast as a short one.
- **A lost session opens again** in the TUI: after the server restarts or the link drops, the next statement you run opens a new session instead of failing for good.
  The statement that met the dead link fails; press `⏎` to run it again.
- **Opening a session gives up after 10 seconds** instead of waiting on a server that never answers, even one that takes the link and then stalls, as a tunnel does.
  Set `connect_timeout` in the connection's `url` to wait longer.
- **A big result cannot fill the memory**: the TUI and `pgbg run` keep the first 10 000 rows, and say `first 10000 rows` when there were more.
  Add a `LIMIT` or a `WHERE` to see the rest.
- **A cut result frees the session at once on a connection that allows writes**: the server stops after the first 10 000 rows instead of sending the whole table first.
  A statement that returns rows runs in its own transaction, committed once its rows are in; inside a transaction you began with `BEGIN`, it stays yours to end.
- **A statement stops when pgbg quits**: the server checks every second that pgbg is still there, on PostgreSQL 14 and later.
- **A keychain password with an accent, a tab or any other character outside plain ASCII logs in**: `pgbg` read it back as hex before.
  Nothing to do: the next session reads it right.
- **A password too long for the keychain is refused before it is saved**, instead of replacing the item with a cut one.
  A keychain error now gives the exit status of `security` only, never what it printed.
- **Errors cannot talk to your terminal**: an escape code a server error quotes from your data prints as `\u{1b}`, and so does a control character in a config key.
  Values and errors also escape the Unicode marks that reverse the order of text.
- **`ctrl-u` or `alt` with a letter at the hidden password prompt adds nothing** to the password, instead of the letter.
- **A config path that links to a missing file is refused** by `pgbg connection` and `pgbg password set`, instead of the link being replaced by a new file.
  The rewritten file reaches the disk before it replaces the old one.

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
