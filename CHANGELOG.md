# What's new in pgbg

Every change you can see or use, newest first.

Each entry says what changed, then where, then how to use it.

## Unreleased

### New

- **`EXPLAIN ANALYZE` of a read** on a read-only connection, in the TUI and `pgbg run`: the plan as the statement ran, with its times and rows.
  Write `EXPLAIN ANALYZE SELECT ...` or `EXPLAIN (ANALYZE, BUFFERS) SELECT ...`; the statement runs in the read-only transaction, so `EXPLAIN ANALYZE` of a write is still refused.
- **The parameter bar** in the TUI: when the statement under the cursor holds a `{{name}}`, a line above the editor shows a field for each, an empty one with what it runs with: its `-- param:` default, `NULL` when `optional`, or `required`.
  Press `⇥` from the editor to reach it, `⇥` and `⇧⇥` to move between fields, `⏎` to run, `esc` to cancel a running statement or go back to the editor; the values stay on the sheet until you quit, and are never written to disk.
- **Completion** in the TUI's editor: as you type a name, the rest of the best table, column or keyword shows faded after the cursor, and `→` takes it.
  It knows what the cursor wants from the statement under it: a table after `FROM` or `JOIN`, a column of `orders` after `o.` when the statement reads `FROM orders o`, else a column of the tables it names first.
  Tables in the search path come bare, the others with their schema, and a name that needs quotes goes in quoted; nothing shows inside a string or a comment.
  `ctrl-space` lists the names that fit at the cursor, close matches too: `↑↓` to pick, `⏎` or `⇥` to take, `esc` to close.
  When no name fits, the list stays open saying `no match`, so `⏎` never runs a half-typed statement.
- **`pgbg run --query` and `--file`**: run a saved query by its path, as `pgbg run shop --query orders/daily`, the project's `.pgbg/queries/` before your own, with the same checks as the TUI; or a statement from a file, `--file orders.sql`, or from stdin with `--file -`.
  The statement typed as an argument still works.
- **Readable cells** in the TUI's grid: `f` puts the column under the cursor in its next format, `F` puts every column readable or raw again.
  A timestamp reads in your local time or as `3 h ago`, a number as `1 234 567` or a size like `1.5 KiB`, a UUID by its first eight digits, bytes by their size.
  `-- format: created_at=relative size=bytes` in a saved query's header opens those columns so, and `[display] format = "readable"` in `config.toml` opens every result readable.
  A `timestamp` without time zone stays raw under `F` and `readable`, since pgbg cannot know its zone; `f` or `-- format:` read it relative as UTC.
  A copy, `--json` and `pgbg run` still give the values as the database gave them.
- **Parameters** in `pgbg run`: write `{{name}}` in a statement, outside strings and comments, and give its value with `--param name=value`, or with `--params-file <file>`, one `name=value` a line, `-` for stdin, so a value need not sit in the shell's history.
  A value is always bound, never put in the statement's text, and the server reads it as the type it gives the parameter: `WHERE day > {{since}}::date`.
  Lines heading the statement declare them: `-- param: since date = 2024-01-01` gives a type and a default, `-- param: owner text optional` binds NULL when left empty; a type the server does not know stops the run.
  One file may serve several queries: a name in it the statement lacks is left alone, while a `--param` for one is refused as a typo.
  A name with no value, or a statement mixing its own `$1` with `{{name}}` stops the run before anything is sent; the history keeps `{{name}}`, never the value.
- **`ctrl-t` finds a table** in the TUI, from anywhere: type a few letters of `schema.table`, then `⏎` shows its columns and indexes, `ctrl-r` runs its first 100 rows, `⇥` puts its name in the editor.
  It reads the schema the first time, like the sidebar's schema tab.
- **The schema** in the TUI's sidebar: on the sidebar, `]` or `[` switches from the saved queries to the schema of the connection on screen, its schemas and their tables; `⏎` folds a schema, `/` keeps the tables whose name holds what you type, `r` reads it again.
  On a table, `p` runs `SELECT * FROM` it `LIMIT 100`, `i` puts its name in the editor at the cursor, quoted when it needs it, and `⏎` shows its columns, their types, defaults and indexes in the result pane.
  The sidebar shows on a terminal of 80 columns or more, saved queries or not; the schema is read the moment its tab shows, on a second, short login along the same tunnel, so a running statement never waits on it; the first 20 000 tables by name are kept, and the status line says when there are more.
  `schema = "off"` on a connection never reads it, for a pooler or a `CONNECTION LIMIT` with no login to spare.
- **Big results page in** in the TUI: a result comes 500 rows at a time, and the next page is fetched as the grid's cursor nears the last row, with no cap.
  The title says `first 1 500 rows · more…` while rows wait on the server, and `all 1 732 rows` once they all came; a result left 5 minutes without a page asked stops there, so its open transaction does not hold up the server, and a page the server fails says why in the status line.
  A connection that allows writes, and `pgbg run`, still keep the first 10 000 rows.
- **Row detail** in the TUI: on the grid, press `⏎` or `space` to see the row under the cursor, one column after another with its type and its whole value, long ones wrapped, starting at the cursor's column.
  JSON shows indented and coloured; `j`/`k` scroll, `esc` goes back to the grid.
- **Copy** from the grid: `h`/`l` move the grid's cursor from column to column, the grid scrolling only when it leaves the view; `y` copies the cell under it, `Y` the row as a CSV line, and `c` then `c`, `j` or `m` the whole result as CSV, JSON or a Markdown table.
  Values are copied whole and as the database gave them, NUMERIC with every digit; the status line says what was copied.
  It goes to your terminal's clipboard (OSC 52, so it works over ssh and in tmux with `set-clipboard on`) and to `pbcopy` on a Mac.
- **`pgbg doctor <connection>`** checks a connection, one line per check: `✓` fine, `!` worth a look, `✗` broken, each with how to fix it.
  It reads the config and the mode of `config.toml`, says whether TLS is on and why, starts and stops the tunnel, logs in, and shows the server's version and the statement timeout in effect, with where it comes from: the connection, its tier or the server.
  It says what the role can write: superuser, `CREATEROLE`, `CREATEDB`, `BYPASSRLS`, `pg_write_all_data`, new schemas, and the schemas, tables and sequences it may change; a read-only connection whose role can write gets `!`, since only a read-only role truly stops a write.
  It checks the role you log in as and every role it may `SET ROLE` to, even one it does not inherit, and names which role grants what.
  `--json` prints the same checks; pgbg exits 1 when one fails, so a script can run it.
  A `ctrl-c`, a `kill` or a closed terminal stops its tunnel too.

- **History** of every statement `pgbg run` sends: `pgbg history <connection>` lists the newest first, with when, how long and what came of it; `--limit n` and `--json` too.
  It lives in `pgbg/history/` in your data folder, readable by you only; it never keeps a row, the server's words or a statement that holds a password, as a word or in a URL such as `postgresql://nina:...@db.example.com`.
  The TUI adds every statement it runs too: press `ctrl-r`, type a few letters, and `⏎` puts the statement under your editor's text, ready to run.
- **The connection picker** in the TUI: press `ctrl-o`, type a few letters of a connection's name, and `⏎` to switch to it.
  Each connection shows its tier's colour, where it goes, whether it takes writes, and whether its session is open.
  Sessions stay open until you quit, so switching back is instant, and each connection keeps its own editor text, result and saved query.
  In the picker, `ctrl-d` removes the highlighted connection from your `config.toml` once you press `y`, with the keychain item no other connection names; `ctrl-x` closes its session and its tunnel, and keeps its editor text until you pick it again.
  `ctrl-a` adds a connection and `ctrl-e` edits the highlighted one in a form: tier, url, where the password comes from, the password itself for a keychain item (typed hidden), tunnel and the command it runs before, writes, TLS and the statement timeout; `⇥` moves, `←`/`→` choose, `⏎` saves, and a refused line says why under it.
  A paste goes into the line with the cursor, and a line longer than the form scrolls with the cursor.
- **Charts** in the TUI: on a result, press `v` to see it as a chart, and `v` again for the grid; the title says which one shows.
  pgbg puts the first text, date or time column across and the numeric columns up: bars for text or a few rows, a line over time otherwise, with gaps in time left as gaps.
  A result it cannot chart says why, such as no numeric column.
- **Saved queries** in the TUI: put `.sql` files under `.pgbg/queries/`, in folders if you like, at the top of your project.
  Press `ctrl-p`, type a few letters of a query's path, and `⏎` opens it in the editor; `↑`/`↓` pick another match, `esc` closes the finder.
  pgbg finds the `.pgbg/queries/` nearest the folder you start it in, as git finds `.git`.
  Queries of your own, for every project, go under `pgbg/queries/` in your config folder; the finder marks them `user`.
  The search stops at your home folder or at the top of your repository, where `.git` is; a queries folder that is a link, or that another user owns, is skipped.
  Only a plain file of 1 MiB at most opens: a pipe, a link or a dump says why it does not.
  The sidebar on the left lists them as a tree, `●` on the one in the editor: `tab` reaches it, `j`/`k` move, `⏎` opens a query or folds a folder; a terminal under 80 columns leaves it out.
  A header of `--` lines names the query and draws its result: `-- name: Orders per day`, `-- chart: bar x=day y=orders`; a result that chart does not fit shows on the grid.
  `-- connection: prod` only says which connection the query was written for: the editor's title shows it, in red for a production one, and the statement still runs on the connection on screen.
  A header line pgbg cannot read shows as a warning in the editor's title; the query opens and runs all the same.
  `ctrl-s` saves the editor's text over the query it stands for; new text asks for a path, as `orders/daily`, and goes under `.pgbg/queries/`, or your own folder when the project has none.
  The editor's title marks unsaved text with `*`, and opening another query over it asks first.
  `ctrl-e` in the editor opens the saved query in your `$VISUAL` or `$EDITOR`, and pgbg reads it again when the editor exits.
- **Tunnels** for `pgbg run` and the TUI: a connection with `tunnel = { command = [...] }` starts that command on a free local port, put in place of `{port}`, and connects through it.
  TLS still checks the certificate against the host in `url`, so `sslmode=require` works through a tunnel; a tunnel without it needs `tls = "off"` on purpose, even to `localhost`.
  `before = [...]` runs once first, and is given up after 2 minutes.
  A tunnel that dies says how and quotes its last words; it stops with pgbg, along with any `ssh` or `gcloud` it started, even when pgbg crashes, is killed or loses its terminal.
  pgbg warns when a tunnel's port still answers once it stopped, as when an ssh `ControlMaster` holds the forward: add `-o ControlPath=none` to the command.
  The TUI's status line shows `⇄ tunnel :<port>`; when the tunnel dies, its session closes, and `⏎` starts both again.
- **A statement timeout on every session**, set by the connection's tier: none on local, a minute on staging, 30 seconds on production.
  The server cancels a statement that runs longer; set `statement_timeout = "2min"` on a connection to choose its own, `"0s"` for none.
  A stricter timeout the server already sets, for your role or your database, is kept; one you set on the connection always wins.
- **`pgbg connection add|edit|remove`** writes `config.toml` for you, one key at a time, keeping your comments and order.
  `add <name> --tier <tier> --url <url>` takes `--tunnel "<command>"`, one of `--password-env`, `--password-command "<command>"` or `--keychain`, and `--writes allowed`; a command splits as a shell would, so quote a word with spaces: `--tunnel "ssh -N -o 'ProxyCommand ssh -W %h:%p jump' -L 127.0.0.1:{port}:db.example.com:5432 bastion"`; `edit` changes only the flags you give.
  An error names the flag it is about, and `edit` can fix a config that does not load.
  `edit --keychain` deletes the item it replaces, unless another connection names it.
  `--statement-timeout 2min` sets the connection's own statement timeout, `0s` none; `--tls off` talks to a remote host in clear on purpose.
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
  So does a mark that reverses text, in the grid, a chart, an error or the status line; a paste drops them.
  Press `tab` to give it the keys, then `j`/`k`, `PgUp`/`PgDn` and `g`/`G` to scroll its rows, `h`/`l` its columns; `tab` again goes back to the editor.
- **`pgbg schema <connection>`** prints every table your role can read, with its columns, their types and defaults, and its indexes.
  Add a few letters to see only the tables whose name holds them, `pgbg schema prod orders`, and `--json` for a script.
  It reads the first 20 000 tables by name that hold the letters you gave, and says so on stderr when there are more.
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

- **A string reads the same to pgbg's guard and to the server**: every read-only transaction sets `standard_conforming_strings = on`, and a connection that allows writes sets it for its session, so a server set otherwise cannot hide part of a statement inside what pgbg took for a string.
- **A character outside ASCII, a no-break space included, is part of a word**, as PostgreSQL reads it, where pgbg took some for blanks.
- **`⇧⇥`** now moves back a field in the connection form on terminals that report it with `shift` held, as most do.
- **A tunnel whose command died is never taken as up**, even when another process answers on its port: pgbg says how it died instead.
- **A second `ctrl-c`, `kill` or hang-up stops the TUI at once** while its sessions stop, as it does under `pgbg run`; it waited up to 3 seconds.
- **Copying a big result no longer freezes the TUI**: its text is built off the screen's loop, and a copy over 100 KB skips the terminal's clipboard, which the status line says (`through pbcopy only`).
- **A copy as JSON keeps every column under its own name**: a name two columns share takes a suffix no other column has, so `count, count, count_2` gives `count`, `count_3` and `count_2`.
- **A lone carriage return in a Markdown copy** becomes `<br>` too, so each row stays one line.
- **Row counts read the same everywhere**: `first 1 500 rows · more…` in the title, `copied first 1 500 rows as CSV` after a copy, thousands apart in `pgbg run` too.
- **A write whose TUI session closes between two round trips never commits**: closing it, quitting, or a config change asks the server to cancel the statement, then drops it, so pgbg sends nothing more on that session; a cancel the server got while idle between two round trips let the rest, `COMMIT` included, go through.
  Every stop works this way, in the TUI and under `pgbg run`, `pgbg doctor` and `pgbg schema`: pgbg waits 1 second for the server to take the cancel and 3 seconds in all.
- **A statement running when its connection is edited fails with its old session** in the TUI; the sheet stayed on `running…` for good.
- **`Esc` on a statement queued behind a page being fetched keeps it from the server**; the cancel was lost and the statement ran.
- **A save whose keychain step fails keeps the connection the config took**: the connection form closes, the picker lists it, and the status line says the password did not reach the keychain (or the old item was not deleted); `pgbg password set` tries again.
- **The connection form scrolls** on a short terminal, so the line with the cursor, such as `timeout`, stays in view.
- **A pasted password keeps its tabs and other control characters**; only its line end is dropped.
- **The row's detail opens on a screen wider than 820 columns**, and no longer rebuilds its lines on every frame.
- **pgbg no longer changes the session settings of a server link it shares through PgBouncer**: a read-only connection sets its statement timeout and its idle timeout inside each transaction, so other clients of a transaction pool keep theirs and pgbg's next transaction keeps its own.
  A result left waiting for its next page closes before a stricter idle timeout on the server would end the whole session.
- **A `real` value shows as the server prints it**: `0.1`, not `0.10000000149011612`.
- **A copy says when some cells could not be decoded**: they are copied as `<unsupported>`, and the status line says how many.
- **A `ctrl-c`, a `kill` or a closed terminal cancels the statement on the server** under `pgbg run`, `pgbg doctor` and `pgbg schema`, before the link and the tunnel go: a write on a connection that allows writes no longer commits after pgbg exits, even where the server cannot tell pgbg went (before PostgreSQL 14, off Linux, behind PgBouncer).
  Closing a session in the TUI sends the cancel again before it cuts the link, when the first one did not reach the server.
  A `before` or password command runs in a process group of its own, so what it started goes with it when pgbg stops waiting on it.
- **Editing a connection whose session you closed leaves it closed** in the TUI; it logged in again on its own.
  Picking it opens a session with the connection as it is now.
- **A result whose session closed while it paged stops at the rows it has**: its title no longer says `more…`, and a page that comes late is dropped.
- **In the chart view, the grid keys do nothing**: `j`, `k`, `PgDn` and `y` moved and copied the hidden grid.
- **A failed save in the connection form keeps a password typed while it saved**; the one sent came back over it.
- **Closing a session stops its running statement**: `ctrl-x` in the picker, a connection removed or edited, or quitting cancels it on the server, and a write on a connection that allows writes never commits after it.
  pgbg waits 3 seconds at most, then cuts the link, so the server stops the statement within a second; a statement still queued is never sent.
- **The connection form saves a new password for the keychain item the connection already names**; it kept the old one.
  Adding a connection under a name already taken is refused beside its line, and never touches the keychain item it names.
- **A failed change to the config says why in the status line** until the next key, unless the form that asked is open; it no longer replaces the result on screen.
  A second `⏎` while the form saves sends nothing.
- **`ctrl-x` in the connection picker keeps it open** and says which session it closed.
- **A connection picked again soon after `ctrl-x` or a config change shows its new session's state**: the words of the old one, still stopping, are dropped.
- **Config changes from the TUI answer in the order you made them**, one at a time.
- **`ctrl-x` keeps the sheet it closes**: the editor text stays, the picker says `closed`, and picking the connection opens a new session.
- **A save the config refuses keeps the password typed** in the connection form, so a second `⏎` sends it again.
- **The history stops waiting** on a disk whose files keep changing under the lock, such as some network mounts, and says so in the log.
- **The finder and the connection picker always show their keys** at the bottom, however many matches or a question under the list; the picker follows a connection added or removed while it is open.
- **A URL whose `options` or `application_name` hold `%20`** reaches the server as written, as in `options=-c%20role%3Dnina`; it failed with `unrecognized configuration parameter "+role"`.
- **The editor colours SQL by meaning**: `NULL` as a constant, `1e3` as a number, and table names plain.
- **The connection picker keeps the `WRITES` badge** on a long name or a narrow terminal, cutting the name instead, and says `running…` for a connection whose statement still runs.
- **Picking a connection whose session failed** in the picker opens it again.
- **A saved query opens in the editor that asked for it**, even when you switched connection while it was read; its error shows there too.
- **The sidebar keeps its cursor on the same query** when the list of queries changes, and draws a name's control characters escaped.
- **A queries folder swapped for a link** after pgbg started is no longer listed or read.
- **The TUI footer keeps `^c quit` and `F1 keys`** however narrow the terminal: the other hints go first, and the status takes at most half the row.
  The status reads as one chain, `⇄ :49213 · connected`, or gives the one reason the session is down after a `✗`; a narrow screen no longer crashes it.
- **The frame names the tier** beside the connection, `● shop · production`, and an error is drawn in its own colour, never the production red.
- **A statement the TUI shows as failed is never sent**: one sent while its session fails to open fails with it, so `⏎` again never runs a write twice.
- **A `kill` or a closed terminal stops the TUI as `ctrl-c` does**: the running statement is cancelled, one still waiting is never sent, and pgbg warns when a tunnel's port still answers.
- **`ctrl-c` quits from the connection picker** too.
- **`pgbg password set` gives the terminal back** when a `kill` stops it while you type.
- **Pasting into the TUI editor** inserts the text as it is: a line end in a pasted statement no longer runs the part before it, and a tab no longer moves the keys to the result.
  Typing in a long editor no longer waits on its colours, which follow a moment after you stop.
- **`esc` cancels only the statement you see running** in the TUI: a cancel that arrives late never stops the next statement, and `esc` while the session is still opening keeps the statement from the server.
- **`ctrl-c` cancels the running statement before pgbg quits**, so the server does not run it to its end.
- **A lost session shows at once** in the TUI's status line, even when nothing runs; the next `⏎` opens a new one.
- **The error mark stays off when you edit while a statement runs**, instead of marking the wrong character.
- **A huge value no longer slows the results grid**: a cell formats only the characters it can show, so a 10 MB JSON value draws as fast as a short one.
- **A lost session opens again** in the TUI: after the server restarts or the link drops, the next statement you run opens a new session instead of failing for good.
  The statement that met the dead link fails; press `⏎` to run it again.
- **Opening a session gives up after 10 seconds** instead of waiting on a server that never answers, even one that takes the link and then stalls, as a tunnel does.
  Set `connect_timeout` in the connection's `url` to wait longer.
- **A big result cannot fill the memory**: the TUI and `pgbg run` keep the first 10 000 rows, and say `first 10 000 rows` when there were more.
  Add a `LIMIT` or a `WHERE` to see the rest.
- **A cut result frees the session at once on a connection that allows writes**: the server stops after the first 10 000 rows instead of sending the whole table first.
  A statement that returns rows runs in its own transaction, committed once its rows are in; inside a transaction you began with `BEGIN`, it stays yours to end.
- **A statement stops when pgbg quits**: the server checks every second that pgbg is still there, on PostgreSQL 14 and later.
- **A keychain password with an accent, a tab or any other character outside plain ASCII logs in**: `pgbg` read it back as hex before.
  Nothing to do: the next session reads it right.
- **A password too long for the keychain is refused before it is saved**, instead of replacing the item with a cut one.
  A keychain error now gives the exit status of `security` only, never what it printed.
- **Values and errors cannot talk to your terminal**: `pgbg run` prints a newline, a tab or an escape code inside a value as `\n`, `\t` or `\u{1b}`, and so do a server error that quotes your data and a control character in a config key.
  The Unicode marks that reverse the order of text are escaped too; use `--json` to get a value's exact text.
- **`ctrl-u` or `alt` with a letter at the hidden password prompt adds nothing** to the password, instead of the letter.
- **A config path that links to a missing file is refused** by `pgbg connection` and `pgbg password set`, instead of the link being replaced by a new file.
  The rewritten file reaches the disk before it replaces the old one.
- **No more clear text through `hostaddr`**: a `url` with `hostaddr` is refused, since it sent a connection to `localhost` on to another machine without TLS.
  Name the host in the `url` instead; pgbg keeps `hostaddr` for its tunnels.
- **A password never shows in a config error**: a `password` written as a number, a boolean or a list is refused like a plain one, and a broken line is reported by its number only.
  Fix the line the error names in your `config.toml`.
- **Dates and timestamps of `infinity` show as `infinity`** in `pgbg run` and the TUI, instead of failing the whole statement; a value out of range shows as `<unsupported>` in its own cell.
- **Floats that are not numbers print as PostgreSQL spells them**: `NaN`, `Infinity` and `-Infinity`, in the table and in `--json`, where they were `null`.
- **A column of a domain type shows its values**, decoded like the type the domain is built on, and a `void` result such as `pg_sleep` shows `NULL`.
- **Wide characters keep `pgbg run` columns aligned**: a cell with `日本` or an emoji takes the room it takes on screen.
- **A `--` comment ends at a carriage return**, as PostgreSQL ends it, so text after it is checked like the rest of the statement.
