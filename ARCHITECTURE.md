# Architecture

`pgbg` is one binary: a TUI (`pgbg`) and scriptable commands (`pgbg run`, `pgbg connections`, ...).
`src/lib.rs` holds everything and exports only what `src/main.rs` calls.
The code is the contract; this page describes it, and changes in the MR that changes a boundary or a flow.

## Module map

| Module | What it holds |
|---|---|
| `main.rs`, `cli.rs` | The clap command line and the dispatch to `commands/` |
| `commands/` | One module per subcommand, each with a `run`; plain output through `render.rs`, JSON with `--json`; `doctor/` is named checks, pure verdicts and a report |
| `config/` | `config.toml`: load, validate, refuse secrets; `ai.rs` the `[ai]` table; `write.rs` rewrites key by key with `toml_edit` through a temp file only its owner reads; `keychain.rs` which keychain items a config names |
| `connection/` | `Connection`, parsed once from config: name, tier, `Target`, tunnel, secret source, write policy, TLS, timeout, schema and AI switches |
| `secret/` | `Secret` (prints `<redacted>`, never cloned, wiped on drop) and its sources; `holds_secret`, the one rule for a text holding a password |
| `tunnel/` | `Tunnel`: a child process forwarding a free `127.0.0.1` port; start, wait for the port, stop |
| `groups.rs` | The process groups pgbg owns, killed whole when dropped and by a panic hook; `output` and `exchange` run a command with capped pipes |
| `db/` | The only module that sees `tokio_postgres`: connect over rustls, open a session along its route, run, page, cancel |
| `db/model/` | The neutral model every other module uses: `ResultSet`, `Column` and its `Origin`, `Value`, `Catalog`, `Schema`, `ServerError`, `Route`, `Params`, `CancelToken` |
| `db/lex.rs` | The tokenizer the guard, parameters, completion and AI share; strings, comments and quoted names hide what they hold |
| `db/guard.rs` | Classifies a statement before it is sent; `admit` builds the `Admitted`, the only kind a session runs; `write_in` names a write for the confirmation |
| `db/run.rs`, `db/read_first.rs`, `db/pages.rs` | Running one statement in its transaction, a read with writes refused on a writable connection, and the portal kept open for its next pages |
| `db/limits.rs`, `db/server.rs`, `db/schema.rs` | The timeouts a session runs with, what the server and the role allow, and the schema read in one snapshot |
| `query/` | Saved queries: `Roots`, the header, load and save; `versions.rs` the texts a save replaced; `params.rs` `{{name}}` to `$n`; `condition.rs` a `WHERE` added from the grid; all but the file system pure |
| `complete/` | Completion, pure: the `Index` built from a `Catalog`, the `Ask` at the cursor, ranked candidates, joins on foreign keys |
| `ai/` | Asking a command for SQL: the prompt and its masking, the schema slice, the answer read back, all pure; `command.rs` runs the command |
| `format/`, `chart/`, `keys.rs`, `export.rs` | Pure: cell formats under a `Clock`, chart series, a result's keys and lookups, copies as CSV, JSON or Markdown |
| `history.rs` | Every statement run, per connection, in `<data folder>/history/<connection>.jsonl`; never a row nor a secret |
| `syntax/` | SQL colours from tree-sitter, JSON colours by hand |
| `act.rs` | Every action in one table: name, default keys, help line, pane |
| `tui/mod.rs` | The event loop: keys, pastes, clicks, answers and timers in, frames out |
| `tui/app/` | `App`, the pure state machine: keys in, `Action`s out, `Answer`s applied; `protocol.rs` what the app and the backend say; one `Overlay` at most takes the keys |
| `tui/backend/` | `Backend::dispatch` hands each `Action` to its part without waiting: one session task per connection, queries, config, history, AI, clipboard |
| `tui/ui.rs`, `tui/*_view.rs`, `tui/grid_view/`, `tui/chart_view/`, `tui/theme.rs` | Drawing: one view per pane or overlay, in vtui's palette with the tier colours |
| `vtui` (git dependency) | Shared with the sibling TUIs: terminal, palette, fields, completion index, help, fuzzy matching, usage |
| `failure.rs` | Why a session did not open, was lost, or gave no result; the history's `Outcome` derives from it |
| `files.rs` | `make_private_dir`, folders for their owner alone (`0700`), and `write_whole`, a whole file through a synced temp file and a rename; every file pgbg writes whole goes through it |
| `lock.rs` | A file opened to append under an exclusive `flock`, so two pgbg never lose each other's writes |
| `usage.rs`, `log.rs` | The `[usage]` counts and the log file, both in pgbg's cache folder |
| `update.rs`, `release.rs` | What `pgbg update` runs, and the commit `pgbg --version` shows |
| `signal.rs` | The signals that stop pgbg and the exit code each gives |

## Data flow

These invariants hold across every flow; a change that breaks one is a bug.

- Every `Action` and every answer carries its connection and its `RunId`, `SessionId`, `LookId` or `AskId`: a late answer never lands on another connection, session, run or ask, and is dropped.
- `db::guard` runs before the server: nothing reaches a session but an `Admitted`, and a write on a read-only connection comes back refused, unsent.
- A read runs alone in `BEGIN READ ONLY`, always rolled back, with its `statement_timeout`, idle timeout and `standard_conforming_strings` set by `SET LOCAL`, so they follow a transaction pooler.
- A write on a connection that allows writes runs only with a `Confirmed`, built only by `tui/app/confirm.rs` after `y`, or by `--yes`; the session task refuses one without it.
- On a writable connection, a read the server finds writing (`25006`) is undone and asks before it runs again as a write: it may run twice.
- Inside the user's own `BEGIN`, pgbg sends no `BEGIN`, `COMMIT` or `ROLLBACK` and never uses the driver's transaction handle: the user's `COMMIT` or `ROLLBACK` decides.
- A parameter is always bound as `$n`, never pasted into the text; the history keeps the `{{name}}`, never the value.
- A result streams a page of 500 rows at a time through a portal; the portal closes on a new run, a cancel, the session closing or lost, its tunnel dying, or 5 minutes idle (a tenth before a stricter server timeout), and the result then stays cut.
- A writable connection and `pgbg run` keep the first 10 000 rows and say the result is cut.
- Every stop goes through `CancelToken::unless_stopped`: a cancel within 1 s, then the work is dropped, so no further round trip is sent.
- A lookup that follows a key runs as on a read-only connection whatever the connection allows, beside an open portal behind a savepoint, and never goes to the history.
- The schema is read lazily on a second, short session along the same route, never kept between the two logins; `schema = "off"` skips it.
- An AI prompt never holds a row, a value, a default, the history, a server's words, nor the connection's name, host or URL; literals are masked unless `literals = "sent"`, and a statement or comment holding a secret is never sent.
- An AI answer is a suggestion: taking it and running it are two keys, and the run goes through the guard.
- `tui/app/` stays pure: the loop hands it the clock as a `Clock` and the timers as events.

## Boundaries

- Only `src/db/` sees a `tokio_postgres` type; everything else uses `db/model/`.
- Side effects live at the edges: `main.rs`, `signal.rs`, `commands/`, `tui/mod.rs`, `tui/backend/`, `tunnel/`, `secret/`, `ai/command.rs`, `config/` when it rewrites the file, `query/` when it touches files, `history.rs`, `usage.rs`, `files.rs`, `lock.rs`, `tui/usage.rs` when it reads the clock.
- `tui/app/` is pure: no clock, no I/O; its tests drive it with keys and `Answer` values.
- `chart/` and `format/` are pure; drawing lives in `tui/`.
- A program that takes the terminal (`$EDITOR`) runs from the loop through `Screen::lend`.
- Secrets are read only in `secret/` and sent only by `Session::open` and the keychain write.

## Read only

Four layers, outermost first; only the last one is a guarantee.

1. **Policy**: a connection is read only unless it says `writes = "allowed"`, whatever its tier.
2. **Guard**: `db::guard` lets through `SELECT`, `WITH` without a data-changing part, `VALUES`, `TABLE`, `SHOW`, and `EXPLAIN` of a `SELECT`, `WITH`, `VALUES` or `TABLE` only, whatever its options, since `ANALYZE` runs it; never a row lock or `SELECT ... INTO`.
3. **Transaction**: one statement per round trip, in `BEGIN READ ONLY`, always rolled back.
   On a writable connection a read outside the user's transaction runs in pgbg's own `READ ONLY` transaction, committed, and a write the server refuses there asks before it runs again.
4. **Role**: only a read-only database role stops what a read-only transaction lets through (`dblink`, `pg_terminate_backend`, `pg_advisory_lock`); `pgbg doctor` says what the role can write.

The guard reads strings and blanks as PostgreSQL does with `standard_conforming_strings = on`, which pgbg sets; the one gap is a writable session where a pooler or the user turns it off.
Inside the user's transaction, a row-returning read goes through a `DECLARE CURSOR` named `pgbg_result_<n>`, free in `pg_cursors`, then `CLOSE`; a `CALL` or `DO` runs as is.
The session tracks the user's transaction from each statement's first words and, when unsure, asks once with a `SAVEPOINT` probe.
`Session::read_only` is refused while the user's transaction is open, since its `ROLLBACK` would end it.
Transaction control and cursor statements never ask for confirmation, save `COMMIT PREPARED` and `ROLLBACK PREPARED`.

## Secrets

- A password comes from the keychain, an environment variable or a command; it is read when the session opens, kept in a `Secret`, and dropped once connected.
- The config loader refuses a `password` key, and a URL with a password, an `sslpassword` or an `oauth_client_secret`.
- Keychain items live under the service `pgbg`; `security -i` writes one so it never shows in argv, and an error quotes only the exit status.
- `pgbg password set` reads the password typed hidden or piped in, never from a flag.
- `pgbg connection remove` deletes its keychain item, unless `--keep-password` or another connection names it.
- No secret in logs, history, cache, fixtures or `Debug` output; the history file is `0600`, as its literals may hold personal data.

## Tunnels

- A tunnel is a command with a `{port}` placeholder; pgbg picks the free port, so two pgbg run side by side.
- An optional `before` runs once first, within 2 minutes, and must be safe to run twice.
- The tunnel, `before`, password and `[ai]` commands each run in their own process group, killed whole on stop, drop, signal or panic (`groups.rs` keeps a registry for the panic hook, since release builds abort).
- A group is killed before its leader is reaped, so its id is never reused under it.
- Only what leaves the group escapes: an ssh `ControlMaster` or `ssh -f`; `stop` fails when the port still answers.
- A tunnel is ready once its port takes a connection while its command runs, within 30 s; one that dies first says how and its last stderr lines, never its arguments.
- pgbg cannot make a tunnel bind `127.0.0.1` only: a tunnelled connection needs TLS or `tls = "off"` on purpose, and the session connects to `hostaddr=127.0.0.1` while TLS checks the URL's host.
- `SIGINT`, `SIGTERM` and `SIGHUP` cancel running statements, stop tunnels, and exit with `128 + n`; only `SIGKILL` leaves a tunnel behind.
- The session task owns its tunnel: a tunnel that dies closes its session, and the next statement starts both.

## Saved queries

- A query is one `.sql` file under `<data folder>/queries/<connection>/` (`Query::folder`, folders `0700`); the working directory plays no part, so a query never lands in a repository.
- `queries = "<connection>"` uses another connection's folder; the config refuses a name that is no connection.
- A new query goes to the folder of the connection on screen, made at its first save.
- A root that is a link or owned by another user is skipped; each list and read checks the root again, follows no link, and reads at most 1 MiB.
- The file is the source of truth: read on open, written on save through a temp file renamed over; a new query never replaces another file, so saving twice changes nothing.
- A save over a query keeps the replaced text as a version in the data folder, best effort, the newest 50; a text holding a secret is kept as a note.
- `-- connection:` only suggests; a statement always runs on the session on screen.
- A query path is drawn with its control characters escaped.

## Performance

- Rows stream a page at a time; nothing waits on the whole result.
- The grid draws only the visible window; widths and colourings come from the first page, capped.
- Values are decoded once, at the boundary, into `Value`; drawing formats only what is on screen.
- Syntax colours rebuild once the text stayed still for 40 ms, so a key never waits on tree-sitter.
- Text from the database or the server is drawn through `render::printable`, its control characters and text-reversing marks escaped.
- Speed budgets live in `budget` modules, release only and `#[ignore]`d: completion on 20 000 tables and 200 000 columns stays under 2.5 ms a key.

## Glossary

One word per concept, in code, docs and UI.

| Word | Means |
|---|---|
| connection | A named database target in the config: where, how to reach it, how to log in |
| tier | `local`, `staging` or `production`; sets the colour and the timeout |
| session | An open link to one connection, with its tunnel, named by its `SessionId` |
| sheet | One connection in the TUI: its editor text, saved query, last run and session state |
| act | A thing a key does: a row of `act.rs` |
| action | What the app asks the backend to do |
| answer | What comes back from the backend, tagged with its connection |
| run | One sending of a statement, named by its `RunId` |
| tunnel | The child process that forwards a local port to a database out of reach |
| statement | One SQL statement, the unit pgbg runs |
| query | A saved `.sql` file in a connection's queries folder |
| result | The columns and rows a statement returned, page by page |
| page | One batch of rows fetched from the portal |
| chart | A bar or line drawing of a result |
| history | Every statement run on a connection |
| version | A text of a saved query that a save replaced, kept apart from the queries |
| schema | The schemas, tables, columns and indexes a session's role can read |
| catalog | The schema as one read gave it, with its search path and whether the table cap cut it |
| completion | A name the editor offers at the cursor |
| parameter | A `{{name}}` in a statement, bound when it runs |
| format | How a column's cells read: raw, or a readable form of its kind |
| lookup | A read that follows a foreign key, read only and out of the history, named by its `LookId` |
| trail | The tables of the rows a detail followed |
| origin | The table column a result's column holds as is; a computed column has none |
| ask | One sending of an instruction to the `[ai] command`, named by its `AskId`; its answer is a suggestion, never a run |
