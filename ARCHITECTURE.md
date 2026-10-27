# Architecture

`pgbg` is one binary: a TUI (`pgbg`) and scriptable commands (`pgbg run`, `pgbg connections`, ...).
`src/lib.rs` holds everything and exports only what `src/main.rs` calls.
This page says how the code is laid out; until the roadmap is done it is the target, and [`docs/roadmap.md`](docs/roadmap.md) says which parts exist.
When a boundary or a flow changes, change this page in the same MR.

## Module map

| Module | What it holds |
|---|---|
| `main.rs`, `cli.rs` | The clap command line and the dispatch to `commands/` |
| `commands/` | One module per subcommand, each with a `run`; plain output through `render.rs`, JSON with `--json` |
| `commands/doctor/` | `pgbg doctor`: a list of named checks in `mod.rs`, each verdict (`✓`, `!`, `✗`) and its fix a pure function in `verdicts.rs`, the lines and the JSON in `report.rs` |
| `config/` | `config.toml`: connections, and theme and keys (planned); load, validate, refuse secrets; rewrite key by key with `toml_edit` from a `ConnectionChange` of keys each parsed on its own (`Tier`, `Target`, `TunnelSpec`, `SecretSource`, `WritePolicy`, `Tls`, `StatementTimeout`) that serializes as the keys it sets, checked as a load would, through a temp file only its owner reads; `config/keychain.rs` says which keychain items a config names and which one a connection owns alone |
| `connection/` | `Connection`: name, tier, target, tunnel, secret source, write policy, TLS, statement timeout; parsed once from config |
| `secret/` | `Secret` (prints `<redacted>`) and its sources (a `KeychainItem` is one word `security -i` cannot split): keychain through `/usr/bin/security`, environment variable, command; `SecretSource::read` reads one when a session opens; `Secret::from_stdin` reads one typed hidden or piped in, and the TUI's connection form types one into a `Secret` it never shows |
| `tunnel/` | `Tunnel`: a child process forwarding a free `127.0.0.1` port to the database; start, wait for the port, stop; its `Route` is what a session opens along |
| `db/` | The only module that sees `tokio_postgres`: connect over rustls, open a session along its route, through the connection's tunnel when it has one (`open_once` for a command, which owns the tunnel and stops it), run a statement, stream rows, cancel |
| `db/model/` | The neutral model: `ResultSet` (its columns, and its rows a `Page` at a time, `push`ed on; its `Extent` says whether it holds them all, was cut, or has more waiting in its portal), `Column`, `ColumnKind`, `Value`, `Schema` (its tables, their columns and indexes, filtered by name, and `Schema::complete` for the editor), `Table`, `ServerError`, `Route` (direct, or a tunnel's `LocalPort`), and `CancelToken`, whose driver part only `db/` reads |
| `db/schema.rs` | `Session::read_schema`: every schema, table, column and index the role can read, from `pg_class`, `pg_attribute` and `pg_index` in one read-only transaction, a partition left to its parent; the TUI reads it on a second session along the same route, opened with the same password read and closed once read, so its statements never wait on it (planned) |
| `db/server.rs` | What the server says of a session (`ServerFacts`: user, version, the `statement_timeout` in effect and the server's own, from `pg_settings`) and what its role can write (`RoleRights`: for the login role, each role it may `SET ROLE` to and `PUBLIC`, the flags, `pg_write_all_data`, database `CREATE`, and the schemas, tables and sequences each may write, read from the ACLs so each grant names its role), read with `Session::read_only` like the schema |
| `db/lex.rs` (planned) | The tokenizer: words, strings, comments, dollar quotes and `{{name}}`, each with its place; the guard, completion and parameters read the same tokens |
| `db/guard.rs` | Classifies a statement before it is sent: read, write, or more than one statement; `admit` turns a statement into an `Admitted`, the only kind a session runs; `statement_at` finds the statement under the editor's cursor with the same tokenizer |
| `db/decode.rs` | Wire values to `Value`, by column type, a domain by its base type; a value it cannot decode is `Unsupported`, never an error |
| `query/` | Saved queries: the `.pgbg/queries/` tree, the header (`-- name:`, `-- connection:`, `-- chart:`, and `-- param:`, `-- format:` (planned)), load, and save |
| `query/params.rs` (planned) | Parameters: the `{{name}}` of a statement turned into `$n` and the `Params` each run binds; pure |
| `complete/` (planned) | Completion, pure: the `Index` built once from a `Schema` (names lowercased and sorted, so a prefix is a binary search), the `Context` at the cursor from the tokens of the statement under it (a table after `FROM` or `JOIN`, a column of `orders` after `o.` when `FROM orders o`, a keyword at a statement's start), and the ranked candidates |
| `format/` (planned) | Formats, pure: for each `ColumnKind`, the ways a cell can read (raw first) and the text of one cell in one of them, the clock and the time zone passed in |
| `history.rs` | Every statement run, per connection: one JSON line each in `<data folder>/pgbg/history/<connection>.jsonl` (folder `0700`, file `0600`), rolled over to `.1` past 4 MB under an exclusive `flock`, so two pgbg never lose each other's lines; the text, when, how long and what came of it, never a row, the server's words or a statement holding `PASSWORD` or a URL with a password; `History::distinct` lists each statement once for `ctrl-r`, which `Search` narrows as the finder does |
| `chart/` | `ChartSpec` (kind, x column, y columns) and `Series` built from a `ResultSet`, both pure; one file per chart kind |
| `syntax/` | SQL colours from tree-sitter, loaded on first use; JSON colours for the row's detail, by hand in `syntax/json.rs` |
| `act.rs`, `keymap.rs` (planned) | Every action in one table (name, default keys, help, pane, and palette verb (planned)); the user's `[keys]` on top (planned) |
| `tui/mod.rs` | The event loop: terminal events, answers and the recolour timer in, frames out |
| `tui/app/` | `App`, the pure state machine: keys in, `Action`s out, `Answer`s applied to the sheet of the connection that gave them, a saved query read to the sheet that asked for it, a config answer to the form that sent its `ConfigChangeId` (else to the status line); `protocol.rs` holds what the app and the backend say to each other, `sheet.rs` one connection's state, `search.rs` the fzf-narrowed list the finder and the picker share; one `Overlay` at most (help, finder, picker, connection form, row detail, the completion list (planned)) takes the keys; the sidebar shows the queries or the schema (planned), and the parameter bar sits above the editor (planned) |
| `tui/backend/` | `Backend::dispatch` hands each `Action` over without waiting, to one part per concern: sessions (one per connection opened, a task owns each one, runs its statements in turn, takes cancels by `RunId` while a statement runs, and says when the session is lost), queries, the config (changed through `config/` on a blocking thread, then read back; a typed password goes to the keychain only once the config took the change), and history (each run added on a blocking thread once answered, best effort and logged; read for `ctrl-r` the same way) |
| `tui/ui.rs`, `tui/*_view.rs` | Drawing: layout in `ui.rs`, one view module per pane or modal |
| `tui/grid_view/` | The results grid: only the visible rows and columns are drawn, each cell through its column's format (planned) |
| `tui/chart_view/` | Draws a `Series` with ratatui's `BarChart` (bars across, so text labels stay readable) and `Chart` (braille line), one file per kind; a legend in vtui's user colours |
| `tui/theme.rs` | The tier colours, the failure colour (never the production red) and the SQL token colours on top of vtui's palette |
| `vtui` (git dependency) | Shared with the sibling TUIs: the terminal in raw mode (`Screen`), the palette (`Theme`), the input box (`Field`), the `ctrl-k` palette, the `?` overlay, fzf-style matching |
| `log.rs` | The log file; never a secret, never a row |
| `signal.rs` | The signals that stop pgbg (`SIGINT`, `SIGTERM`, `SIGHUP`) and the exit code each gives |

## Data flow

Running a statement:

1. The editor hands `App` the statement under the cursor; `App` returns `Action::Run { connection, run, statement }`, and every answer about it carries the same connection and `RunId`, so a late one never lands on another connection or the next run.
2. `db::guard` classifies it; a write on a read-only connection comes back as `Incoming::Refused` in an `Answer` and never reaches the server.
3. The session opens `BEGIN READ ONLY` and binds the statement to a portal (`Transaction::bind`).
   A portal takes any statement the guard admits; `DECLARE CURSOR` takes only a `SELECT` or `VALUES`, not `SHOW` or `EXPLAIN`.
4. The first page (`query_portal(&portal, 500)`) comes back as `Incoming::Page`; the grid paints it, and more pages are fetched as the grid nears the end.
   Until paging lands, a result keeps its first 10 000 rows and says it is cut.
5. A server error comes back as a `ServerError` with the server's position; the editor marks that character until the text changes.
6. `Esc` on a running statement returns `Action::Cancel { connection, run }`; the session task sends the server a cancel request over a link of its own, with the session's TLS, only while that run is on the server, and before the next one starts.
   The statement fails with the server's words, and the transaction is always rolled back at the end; a run cancelled before it started is never sent.
7. Closing a session (`ctrl-c` to quit, `ctrl-x` in the picker, a connection removed or changed) cancels its running statement first, and one not yet sent never is; pgbg waits 2 seconds at most for the session task, then aborts it.
   A `Session` dropped in the middle of a statement cuts its link instead of letting it finish, so the server stops the statement and rolls it back (`client_connection_check_interval`).

Switching connection:

1. `ctrl-o` opens the connection picker; picking one not yet open starts its sheet and returns `Action::Open(connection)`, picking one whose session failed returns it again so its task opens it again, picking an open one shows its sheet and returns nothing.
2. When the connection has a tunnel, the session task starts it with `Tunnel::start`: it picks a free port, runs the command, and waits until the port answers.
3. `Session::open` takes the tunnel's `Route`: it connects to `hostaddr=127.0.0.1` and the tunnel's port, and keeps the URL's host for the TLS certificate check; it reads the schema in the background for completion and the sidebar (planned).
4. The session stays open until pgbg quits or `ctrl-x` in the picker closes it, so switching back is instant; its tunnel stops with it.
   When the config changes, the sheet of a connection gone closes and the sheet of one changed opens a new session on it (`Action::Reopen`).
   Each sheet keeps its editor text and its result while another one is shown, and the answers of its session land on it alone.
5. A session whose link is gone, or whose rollback failed, is opened again before the next statement; it is never retried on its own.
   A statement sent while its session fails to open fails with it, and is never sent on a later open; once pgbg quits, one still queued is never sent.
6. Opening sets `statement_timeout`: the connection's own, else the stricter of its tier's (none on local, 60 s on staging, 30 s on production) and the one the server already applies, read from `pg_settings`.
   It is a session setting, so behind PgBouncer in transaction pooling it does not follow pgbg's next transaction: there, give the role its own `statement_timeout`.
7. Opening gives up after 10 seconds, TLS and login included, unless the `url` sets `connect_timeout`; and each session asks the server to stop its statement once pgbg is gone (`client_connection_check_interval`).

Completing a name (planned):

1. Once a session opens, its task opens a second session along the same route, reads the schema, closes it and answers `Incoming::Schema`; a failed read is a warning, never a lost session.
   The backend builds the `complete::Index` on a blocking thread and the sheet keeps it with the `Schema` behind an `Arc`, shared and never changed; `r` in the schema tab reads it again, and so does a session opened again.
2. Each key in the editor asks `complete` for the context at the cursor and the candidates, in the app, before the frame: under a millisecond, so it never waits on the database.
3. Candidates rank by: a prefix before a fuzzy match, a column of a table the statement names before any other, then shorter first; at most 50 are kept.
4. The best one shows dimmed after the cursor (vtui's `complete::ghost`); `→` at the end of the word takes it, quoted when its name needs it; `ctrl-space` opens the list.
5. A schema past 20 000 tables keeps the first ones in name order and says it is cut; completion and the sidebar work on what was kept.

Running a saved query with parameters (planned):

1. `query::params` reads the `{{name}}` of the statement under the cursor from `db::lex`'s tokens, never inside a string, a comment or a quoted name, and the header's `-- param: since date = 2024-01-01`, `-- param: owner text optional`.
2. The parameter bar above the editor shows one field per name, filled with the last values given for this query on this connection, else the default; `⏎` in a field runs.
3. The statement is sent with each name as `$1`, `$2` (the same name the same number), so the guard classifies what the server gets; a name left empty, with no default and not `optional`, stops the run before anything is sent, and an `optional` one is bound as NULL.
4. The session prepares the statement with the header's types, looked up by the server (`to_regtype`) from a bound name, else with the types the server infers, and binds each value in text: the server parses it with that type's own input, and a value it cannot read fails with its words.
5. The history keeps the statement with its `{{name}}`, never the values; the values go in `<data folder>/pgbg/params/<connection>.json` (`0600`), keyed by query path.

Formatting a cell (planned):

1. A result opens with every column in the format the header's `-- format:` names, else the config's `[display] format` (`raw` or `readable`).
2. `f` on a column steps through its kind's formats, `F` sets every column raw or readable; the sheet keeps the choice by column index with the result, and the grid widths are measured again on the first page.
3. The loop passes the clock and the local time zone to the drawing, so `format/` stays pure; a relative time redraws each minute while one is shown.
4. Formats are for reading only: `pgbg run`, `--json` and a copy (planned) give raw values.

Charting a result:

1. `v` on a result asks `chart::infer` for its `ChartColumns`, by index so two columns of one name (`count`) stay apart: the first text, date or time column as x, else the first numeric column when it holds ascending integers (`year`), every other numeric column as y; bars when x is text or there are at most 12 rows, a line otherwise.
2. A saved query can set it in its header instead: `-- chart: bar x=day y=orders,refunds`, parsed by `ChartSpec`'s `FromStr`; a header without `x=` or `y=` leaves them to the result, as `infer` would pick them.
   A header name matches a column whatever its case, as PostgreSQL folds an unquoted name, unless one column has it exactly; a name several columns answer to is refused.
3. `chart::Series` builds the points: a line over dates, times or integers is placed on their scale and sorted, so a gap shows, and a row with no place on it (NULL, infinity) is left out; bars and a line over text keep the result's order, one point per row.
4. A y value of NULL, NaN or infinity leaves a gap, and NUMERIC is read as a float; each kind caps its points to what a terminal shows: bars keep the first 64, a line samples 1024 evenly, and the series says when rows were left out.
5. The sheet keeps the series built on `v` with its result, and a new result starts on the grid, or on the chart of the editor's `-- chart:` header when it fits the result; `chart_view` draws it, or says why the result has no chart.

## Boundaries

- Only `src/db/` sees a `tokio_postgres` type; everything else uses `db/model/`.
- Side effects live at the edges: `main.rs`, `signal.rs`, `commands/`, `tui/mod.rs`, `tui/backend/`, `tunnel/`, `secret/`, `config/` when it rewrites the file, `query/` when it lists and reads the queries folders, `history.rs` when it appends and reads its files.
- `tui/app/` is pure: it never reads the clock nor does I/O; its tests drive it with keys and `Answer` values.
- `chart/` is pure: a `ResultSet` in, a `Series` out; drawing lives in `tui/chart_view/`.
- A program that takes the terminal (`$EDITOR` on a saved query) runs from the loop through `Screen::lend`, with the key stream dropped and pastes given back until it exits; the query is read again after.
- Secrets are read only in `secret/` and sent only by `Session::open` and the keychain write.

## Read only

Four layers, outermost first; only the last one is a guarantee.

1. **Policy**: each connection chooses; it is read only unless it says `writes = "allowed"`, whatever its tier.
2. **Guard**: `db::guard` lets through `SELECT`, `WITH` without a data-changing part, `VALUES`, `TABLE`, `EXPLAIN` without `ANALYZE`, and `SHOW`, without a row lock (`FOR UPDATE`, `FOR SHARE`) or `SELECT ... INTO`; anything else is refused locally.
3. **Transaction**: one statement per round trip (the extended protocol refuses more), inside `BEGIN READ ONLY`, always rolled back; the server refuses writes even when the guard misses one.
4. **Role**: a read-only database role. A read-only transaction still lets a function with side effects run (`pg_terminate_backend`, `dblink`); only the role stops those.

A parameter is always bound, never put in the statement's text (planned): its value cannot change what the guard admitted.

On a connection that allows writes, a statement runs as typed, except that one returning rows goes through a portal in its own committed transaction, unless the user's own `BEGIN` left one open (a `SAVEPOINT` probe tells).

`pgbg doctor <connection>` checks the role and says what it can write.

## Secrets

- A connection names where its password comes from: `password = { keychain = "pgbg/prod" }`, `{ env = "PROD_PGPASSWORD" }` or `{ command = ["gcloud", "secrets", "versions", "access", ...] }`.
- The password is read when the session opens, kept in a `Secret`, and dropped once connected.
- The config loader refuses a plain `password`, and a URL with a password in it.
- Keychain items live under the service `pgbg`, one account per item; `security -g` reads one on a pipe, in hex when it is not printable ASCII, and `security -i` writes one so the password never shows in argv; an error quotes `security`'s exit status only.
- `pgbg password set <connection>` takes the password typed hidden or piped in, never as a flag.
- `pgbg connection remove` deletes the connection's keychain item before its table, unless `--keep-password` or another connection names the same item.
- A statement in the history may hold personal data in its literals (`WHERE email = '...'`): that is why its file is `0600` and never leaves this machine.

## Tunnels

- A tunnel is a command with a `{port}` placeholder: `ssh -N -o ControlPath=none -o ExitOnForwardFailure=yes -L 127.0.0.1:{port}:localhost:5432 bastion`, `gcloud compute start-iap-tunnel ... --local-host-port=localhost:{port}`, `cloud-sql-proxy --port {port} ...`.
- An optional `before` command runs first, once, within 2 minutes, and is safe to run twice (starting a stopped bastion).
- pgbg picks the free port, so two pgbg can run side by side; the child runs in its own process group, and stopping or dropping the `Tunnel` kills the whole group, so an `ssh` or `gcloud` it started dies too.
- Only what stays in that group dies with it: an ssh `ControlMaster` or `ssh -f` keeps the forward elsewhere, so `stop` fails when the port still answers, and a command that ends cleanly before its port answers is said to have handed the forward off.
- pgbg cannot make the command bind `127.0.0.1` only, nor know who answers on the port: the examples bind it explicitly, and a tunnelled connection needs TLS or `tls = "off"` on purpose, never the clear a local host gets.
- A tunnel is ready once its port takes a TCP connection, within 30 seconds; one that dies first says how it exited and its last lines of stderr, never its arguments.
- `SIGINT`, `SIGTERM` and `SIGHUP` stop pgbg through `signal.rs`: `pgbg run`, `pgbg doctor` and `pgbg schema` drop their work, so every `Drop` runs and the tunnel goes; the TUI quits as `ctrl-c` does, cancelling every running statement, and each session task stops its tunnel, warning when its port still answers; pgbg exits with `128 + n`. Only a `SIGKILL` leaves a tunnel behind.
- A session through a tunnel connects to `hostaddr=127.0.0.1` on the tunnel's port, and TLS still checks the certificate against the URL's host; that is why the config refuses a `hostaddr` of its own.
- The session task owns the tunnel with the session: the status line shows `⇄ tunnel :<port>`, `starting…` or how it died; a tunnel that dies closes its session and says why, and the next statement starts both again.
- A session reopened while its tunnel still runs keeps that tunnel.
- Release builds abort on a panic, so no `Drop` runs: `tunnel/groups.rs` keeps every live process group in a registry, and a panic hook kills them all.
- The hook kills them only when the panic ends the process: always in a release build, and on the main thread only when the build unwinds, since a tokio task, a blocking thread or a test catches its own panic and goes on.
- A tunnel seen ended has its group killed at once, before its id can be reused, and a group the hook killed is never signalled again.

## Saved queries

- One `.sql` file per query under `.pgbg/queries/`, found by walking up from the working directory like `.git`, then under the user config folder.
- The walk stops at the home folder or at the first folder holding `.git`, so a folder above the project never gives its queries.
- A root that is a link, under a `.pgbg` that is a link, or that another user owns is skipped with a warning: a cloned repository cannot point pgbg at the whole disk.
- Each list and each read checks the root again, and that it still leads to the real path it had when found: a folder swapped for a link since then is skipped.
- Folders nest; the sidebar shows the tree and the fuzzy finder searches every file.
- Listing walks the folders on a blocking thread, keeps only plain files, and never follows a link, so nothing outside the root is listed.
- Opening reads the file through `tokio::fs`, again only when it is a plain file whose real path stays inside its root, and only up to 1 MiB; a bigger one is too big for the editor.
- A query path is drawn with its control characters and text-reversing marks escaped.
- An optional header of `--` lines sets the name, the default connection, the chart, and the parameters and formats (planned); a file without a header is still a query.
- The header is read again whenever the editor's text changes; a line it cannot read is a warning in the editor's title, never a failure.
- `-- connection:` only suggests: the editor's title names it, in red when it is a production connection, and a statement always runs on the session on screen.
- The file is the source of truth: pgbg reads it when it is opened and writes it back when it is saved, nothing else.
- A save writes a temporary file beside the query and renames it over; it makes the missing folders, refuses a link or a root that changed, and a new query never replaces another file, so saving twice changes nothing.
- A new query goes under the project's root, else under the user's; opening another query over unsaved text asks first.

## Performance

- Rows stream through a portal, a page at a time; nothing waits on the whole result.
- The grid draws only the visible window; column widths come from the first page and the header, capped.
- Values are decoded once, at the boundary, into `Value`; drawing formats only what is on screen, and the row's detail at most 64 Ki characters of a value.
- Syntax colours live in `tui/app/editor.rs` with the text they colour: an edit or an opened query marks them stale, and the loop rebuilds them once the text stayed still for 40 ms, so a key never waits on tree-sitter.
- The editor view draws only the lines in view, each in one pass over its spans.
- The TUI asks the terminal for bracketed paste: a paste comes as one `Event::Paste`, inserted whole, so its line ends never run a statement; it drops control characters and text-reversing marks.
- Text from the database or the server (grid cells, the row's detail, chart labels and names, a failed run, the status line) is drawn through `render::printable`, its control characters and text-reversing marks escaped.

## Glossary

One word per concept, in code, docs and UI.

| Word | Means |
|---|---|
| connection | A named database target in the config: where, how to reach it, how to log in |
| tier | `local`, `staging` or `production`; sets the colour and the timeout |
| session | An open link to one connection, with its tunnel if it has one; its `SessionState` (opening, open, failed) is what the status line says |
| sheet | One connection in the TUI: its editor text and the saved query it stands for, its last run, its session's state |
| act | A thing a key does: a row of `act.rs`; the app turns acts into actions |
| action | What the app asks the backend to do: open a session, run or cancel a statement, list or read queries |
| answer | What comes back from the backend: a session's `Incoming`, tagged with its connection, or the saved queries' |
| run | One sending of a statement, named by its `RunId`: running, cancelling, done or failed |
| tunnel | The child process that forwards a local port to a database out of reach |
| statement | One SQL statement, the unit pgbg runs |
| query | A saved `.sql` file under `.pgbg/queries/` |
| result | The columns and rows a statement returned, page by page |
| page | One batch of rows fetched from the portal |
| chart | A bar or line drawing of a result |
| history | Every statement run on a connection |
| schema | The schemas, tables, columns and indexes a session's role can read |
| completion | A name the editor offers at the cursor, from the schema or the keywords |
| parameter | A `{{name}}` in a statement, filled in the parameter bar and bound when it runs |
| format | How a column's cells read: raw, or a readable form of its kind |
