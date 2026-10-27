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
| `connection/` | `Connection`: name, tier, target (a `Target`, the `postgres://` URL parsed from the config or `--url`, whose `Debug` shows only its place), tunnel, secret source, write policy, TLS, statement timeout, whether the TUI reads its schema; parsed once from config |
| `secret/` | `Secret` (prints `<redacted>`, never cloned, wiped from memory on drop, as is the hex line written to `security -i`; a password typed or piped in grows in place within a buffer sized ahead) and its sources (a `KeychainItem` is one word `security -i` cannot split): keychain through `/usr/bin/security`, environment variable, command; `SecretSource::read` reads one when a session opens; `Secret::from_stdin` reads one typed hidden or piped in, and the TUI's connection form types one into a `Secret` it never shows |
| `tunnel/` | `Tunnel`: a child process forwarding a free `127.0.0.1` port to the database; start, wait for the port, stop; its `Route` is what a session opens along |
| `program.rs` | `Program`: the first word of a command line, which messages name without its arguments |
| `groups.rs` | The process groups pgbg owns (a tunnel's, a `before`, password or `[ai]` (planned) command's): each one killed whole when dropped, and every live one by a panic hook |
| `db/` | The only module that sees `tokio_postgres`: connect over rustls, open a session along its route, through the connection's tunnel when it has one (`open_along` for the TUI's session task, which keeps the tunnel for the next open and hears it start; `open_once` for a command, which owns the tunnel and stops it), run a statement, stream rows, cancel |
| `db/model/` | The neutral model: `ResultSet` (its columns, and its rows a `Page` at a time, `push`ed on; its `Extent` says whether it holds them all, was cut, or has more waiting in its portal), `Column`, `ColumnKind`, `Value`, `JsonText` (`json.rs`: a JSON cell as the compact text the server sent, its keys in the server's order, indented for the row's detail by a pass over that text that stops at the characters shown, never a parse), `Catalog` (a `Schema` as one read gave it, with the session's `current_schemas(false)` and whether the table cap cut it), `Schema` (its tables, their columns and indexes, filtered by name), `Table`, `ServerError`, the name a statement writes for a table (`name.rs`: bare when PostgreSQL reads it back the same and the guard reads no write in it, as it would in `delete`, else quoted), the words the guard reads as a write (`words.rs`), `Route` (direct, or a tunnel's `LocalPort`), `Params` (the values bound to a statement's `$n`, in order), and `CancelToken`, whose driver part only `db/` reads |
| `db/connect.rs` | `Session::open`: the driver's config for a target along its route, the TLS it logs in with, the connect wait (10 s unless the URL's `connect_timeout` says otherwise, TLS and login included), and the task that drives the link and says why it ended |
| `db/limits.rs` | What a session runs with: both of the server's timeouts, read from `pg_settings` in one simple query when it opens, the `statement_timeout` in effect (the connection's choice, else the stricter of its tier's and the server's), set on the link only where writes are allowed, and `ReadLimits`, which each read-only transaction sets with `SET LOCAL` |
| `db/run.rs` | `Session::run`: one statement alone and its first `ROW_CAP` rows; a read-only one through a portal in `BEGIN READ ONLY`, always rolled back, a write that returns rows through a portal in its own committed transaction, or a cursor in the one the user began, which pgbg never ends; `Session::read_only` for the schema and the server facts |
| `db/cancel.rs` | `CancelToken`: the server's cancel request, and `unless_stopped`, the one way pgbg stops a statement it leaves (a signal under a command, a session closing in the TUI): cancel within `CANCEL_WAIT`, then drop the work |
| `db/pages.rs` | `Session::start`: a statement's first page of 500 rows, and on a read-only connection its portal kept open in `Pages`, in its `BEGIN READ ONLY` transaction, until `next` fetches the last page or `close` rolls it back; each read-only transaction has the server end it once left idle 6 minutes (`SET LOCAL idle_in_transaction_session_timeout`, unless the server's own is stricter), and the TUI closes the portal first: after 5 minutes, or a tenth before the server's stricter timeout |
| `db/schema.rs` | `Session::read_schema`: every schema, table, column and index the role can read, from `pg_class`, `pg_attribute` and `pg_index` in one read-only `REPEATABLE READ` transaction, so its queries see one snapshot, a partition left to its parent, the first 20 000 tables by name (`LIMIT` in the SQL, after the filter `pgbg schema` passes it), with `current_schemas(false)`; `read_schema_along` reads it on a session of its own along a route, which the TUI gives it from the session it already has open |
| `db/server.rs` | What the server says of a session (`ServerFacts`: user, version, the `statement_timeout` in effect and the server's own, from `pg_settings`) and what its role can write (`RoleRights`: for the login role, each role it may `SET ROLE` to and `PUBLIC`, the flags, `pg_write_all_data`, database `CREATE`, and the schemas, tables and sequences each may write, read from the ACLs so each grant names its role) and what it can do that a read-only transaction does not stop (`ReadOnlyGaps`: `pg_signal_backend`, the `dblink` functions it may call, the settings granted it with `GRANT SET ON PARAMETER`), read with `Session::read_only` like the schema |
| `db/lex.rs` | The tokenizer: words (compared ASCII case aside, as PostgreSQL reads keywords), `;`, `{{name}}` and `$n`, each with the bytes it spans, strings, comments, dollar quotes and quoted names hiding what they hold; a `{{` that opens no name, or one glued to a word, is unreadable; the guard, parameters and completion read its tokens; the editor keeps its text read once per change as a `Lexed`, which the statement under the cursor, its parameters and completion share; `span_at` says whether a byte falls in code, a string, a comment, a quoted name or a `{{name}}`, which keeps completion out of all but code |
| `db/guard.rs` | Classifies a statement before it is sent: read, write, or more than one statement; `admit` turns a statement and its `Params` into an `Admitted`, the only kind a session runs, with the `TransactionStep` it takes on a transaction the user began, refused when the count of values is not the count of its `$n` or when it holds more than one statement, whatever the connection allows, and the `Write` it is when it is not a read; `write_in` names that `Write` (each write it holds, with its verb and the tables it lists, as `INSERT into archived_orders and DELETE from orders`, and `CASCADE` when it says so, or more than one statement) from tokens already read, for the confirmation, and none for transaction control or cursor statements (`BEGIN`, `COMMIT`, `SAVEPOINT`, `FETCH`, `CLOSE`), which change no data by themselves; the words it takes for a write live in `db/model/words.rs`, which `name.rs` reads too; `statement_at` finds the statement under the editor's cursor with the same tokenizer |
| `db/bind.rs` | `AsText`, the one `ToSql` a value is bound through: any parameter type, sent as text for the server to read with that type's input, `None` as NULL; `prepare` looks up each type a header names (`to_regtype`, the name bound) and leaves the others to the server (id 0) |
| `db/decode.rs` | Wire values to `Value`, by column type, a domain by its base type; a value it cannot decode is `Unsupported`, never an error |
| `query/` | Saved queries: the `.pgbg/queries/` tree (`Roots`, which the TUI lists and `pgbg run --query` reads by path), the header (`-- name:`, `-- connection:`, `-- chart:`, `-- format:` and `-- param:`), load, and save |
| `query/params.rs` | Parameters, pure: the `{{name}}` of a statement turned into `$n`, and the values `Given` by name (`--param`, a `--params-file` line, the parameter bar) into the `Params` its run binds, a name left empty taking its `-- param:` default, or NULL when `optional`, then `admit`ted by the guard; its `Numbering` reads a place in the filled text, where a server error points, back in the template |
| `complete/` | Completion, pure: the `Index` built once from a `Catalog` (a vtui completion index per list: the tables, every column, each table's own columns and the keywords, in the catalog's order, and the names that need quotes with how they go in; a table bare in the first schema of `current_schemas(false)` that holds its name, else as `schema.table`; each column name once, and each table's own), the `Ask` at the cursor from the tokens of the statement under it, never of the whole editor, and none inside a string, a comment or a quoted name (a table after `FROM`, `JOIN` or a `,` of a `FROM` list; a column of `orders` after `o.` when `FROM orders o`; a keyword at a statement's start; else a column or a keyword), and the ranked candidates: vtui ranks each list, the ghost's prefix only and the list's fuzzy too, and `complete/` merges the lists, quotes a name, cases a keyword as typed and offers a table's first column after a bare `.` |
| `ai/` | Asking a command for SQL, pure: the statement's literals masked and its comments stripped (`mask.rs`), and the command's answer read back as statements with its literals put back (`answer.rs`); the `Prompt` (instruction, statement, schema slice, tier) as the text on the command's stdin, and the backend running the command, are planned |
| `format/` | Formats, pure: for each `ColumnKind`, the ways a cell can read (raw first), the step to the next one, and the text of one cell in one of them, the clock and the time zone passed in as a `Clock`; `number.rs` groups digits and reads sizes, `time.rs` reads timestamps local or relative |
| `export.rs` | A cell, a row or a whole result as text to paste: CSV (a NULL left empty), JSON (one object per row, as `--json` gives values, a JSON cell as its compact text, a later column of a name already taken suffixed `_2`, `_3`, past every real name) or a Markdown table; every value raw and whole, a CSV formula not guarded; pure |
| `history.rs` | Every statement run, per connection: one JSON line each in `<data folder>/pgbg/history/<connection>.jsonl` (folder `0700`, file `0600`), rolled over to `.1` past 4 MB under an exclusive `flock`, so two pgbg never lose each other's lines, the file opened again a few times at most when another pgbg rolled it over meanwhile; the text, when, how long and what came of it, never a row, the server's words or a statement holding `PASSWORD` or a URL with a password; `History::distinct` lists each statement once for `ctrl-r`, which `Search` narrows as the finder does |
| `chart/` | `ChartSpec` (kind, x column, y columns) and `Series` built from a `ResultSet`, both pure; one file per chart kind |
| `syntax/` | SQL colours from tree-sitter, loaded on first use; JSON colours for the row's detail, by hand in `syntax/json.rs` |
| `act.rs`, `keymap.rs` (planned) | Every action in one table (name, default keys, help, pane, and palette verb (planned)); the user's `[keys]` on top (planned) |
| `tui/mod.rs` | The event loop: terminal events, answers and the recolour timer in, frames out |
| `tui/app/` | `App`, the pure state machine: keys in, `Action`s out, `Answer`s applied to the sheet of the connection that gave them, a saved query read to the sheet that asked for it, a config answer to the form that sent its `ConfigChangeId` (else to the status line); `protocol.rs` holds what the app and the backend say to each other, `keys.rs` which pane or overlay takes a key or a paste and the act it does, `sheet.rs` one connection's state, `search.rs` the fzf-narrowed list the finder and the picker share; one `Overlay` at most (help, finder, picker, connection form, row detail, the footer prompts (`save.rs`; `confirm.rs`: a write waiting for `y`), the copy format question, the schema filter, the completion list and the `ctrl-t` table finder (`table_finder.rs`: every table of the schema, with the schema tab's actions: `⏎` its columns, `^r` its preview, `⇥` its name in the editor)) takes the keys; the sidebar shows the queries or the schema (`schema_tab.rs`: its schemas and their tables, folded and filtered, `[` `]` to switch; on a table `p` runs `SELECT * FROM` it `LIMIT 100` as the editor's statements run, through the guard and into the history, `i` puts its name at the editor's cursor, and `⏎` shows its columns and indexes in the result pane), and the parameter bar (`param_bar.rs`: a secret vtui field per `{{name}}` of the statement under the cursor, so `Debug` never prints a value, its values kept by name on the sheet) sits above the editor |
| `tui/backend/` | `Backend::dispatch` hands each `Action` over without waiting, to one part per concern: sessions (`session/`: one per connection opened, a task owns each one (`mod.rs`), hearing jobs and cancels and saying answers through its `Mailbox`, reached through its link (`link.rs`), runs its statements in turn, takes cancels by `RunId` while a statement runs (`run.rs`), serves the pages of a result while its portal is open (`paging.rs`), reads the schema beside its statements (`schema.rs`), and says when the session is lost), queries, the config (one change at a time, answered in the order asked, through `config/` on a blocking thread, then read back; a typed password goes to the keychain only once the config took the change, and moves back to the form when it did not; once it did, the connections are read back whatever the keychain says, and a keychain step that failed is a warning in the status line), history (each run added on a blocking thread once answered, best effort and logged; read for `ctrl-r` the same way), and the clipboard (a whole result's text built on a blocking thread and handed back to the app as `Answer::Exported`; `pbcopy` in a task, while the loop sends the terminal's OSC 52 through vtui for a copy of 100 KB at most, so a copy reaches one or the other inside tmux or over ssh) |
| `tui/ui.rs`, `tui/*_view.rs` | Drawing: layout in `ui.rs`, one view module per pane or modal |
| `tui/grid_view/` | The results grid: only the visible rows and columns are drawn, the cursor's cell lit, each cell through its column's format and the clock the loop read for the frame |
| `tui/chart_view/` | Draws a `Series` with ratatui's `BarChart` (bars across, so text labels stay readable) and `Chart` (braille line), one file per kind; a legend in vtui's user colours |
| `tui/theme.rs` | pgbg's `Theme`: vtui's palette, read through `Deref`, with the tier colours and a production colour of its own (`danger` leaning to magenta), so a production frame never reads as an error drawn in `danger`; the SQL token colours are vtui's `Syntax` |
| `vtui` (git dependency) | Shared with the sibling TUIs: the terminal in raw mode (`Screen`), the palette (`Theme`) and its code colours (`Syntax`), the input box (`Field`, its cleaned paste, and the secret one whose `Debug` hides its text), styled lines wrapped to a width (`text::wrap_line`), the completion index (`complete::Index`), the `ctrl-k` palette and its `Pick` rows, the `?` overlay, fzf-style matching |
| `log.rs` | The log file; never a secret, never a row |
| `signal.rs` | The signals that stop pgbg (`SIGINT`, `SIGTERM`, `SIGHUP`) and the exit code each gives |

## Data flow

Running a statement:

1. The editor hands `App` the statement under the cursor; `App` returns `Action::Run { connection, run, statement }`, and every answer about it carries the same connection and `RunId`, so a late one never lands on another connection or the next run.
2. `db::guard` classifies it; a write on a read-only connection comes back as `Incoming::Refused` in an `Answer` and never reaches the server.
   A write on a connection that allows writes asks before `App` returns its `Action::Run`: the footer names its verb and target (`DELETE from orders?`), `y` sends it, any other key drops it unsent, on every tier.
   `pgbg run` asks `y/N` on the terminal, and refuses a write when stdin is no terminal unless `--yes` says yes ahead.
   A table preview (`p`, `^r`) is always a read: `name.rs` quotes a table name the guard would read as a write.
3. The session opens `BEGIN READ ONLY` and binds the statement to a portal (`Transaction::bind`).
   A portal takes any statement the guard admits; `DECLARE CURSOR` takes only a `SELECT` or `VALUES`, not `SHOW` or `EXPLAIN`.
4. The first page (`query_portal(&portal, 500)`) comes back in `Incoming::Result`; the grid paints it, and once its cursor is 100 rows from the last one it has, `Action::NextPage` asks for the next, which comes back as `Incoming::Page`.
   While more pages wait, the session task serves them in `backend/paging.rs`, the portal and its transaction held open; the title says `first 1 500 rows · more…`, then `all 1 732 rows` at the end.
   The portal closes before its last page on a new run, a cancel, the session closing or lost, its tunnel dying, or 5 minutes without a page asked, so a held snapshot never keeps the server from cleaning up for long; an empty `Cut` page then says the result stays at the rows it has, and a page the server fails comes back as `Incoming::PageFailed`, its words in the status line.
   A connection that allows writes, and `pgbg run`, keep the first 10 000 rows of a result and say it is cut.
5. A server error comes back as a `ServerError` with the server's position; the editor marks that character until the text changes.
6. `Esc` on a running statement returns `Action::Cancel { connection, run }`; the session task sends the server a cancel request over a link of its own, with the session's TLS, only while that run is on the server, and before the next one starts.
   The task keeps the latest run a cancel came for, so one for a run queued behind a page being fetched still keeps it from the server.
   The statement fails with the server's words, and the transaction is always rolled back at the end; a run cancelled before it started is never sent.
7. Closing a session (`ctrl-c` to quit, `ctrl-x` in the picker, a connection removed or changed) stops its running statement as every stop does (`CancelToken::unless_stopped`): it asks the server to cancel it, waiting 1 second at most (`CANCEL_WAIT`), then drops it, so no further round trip is sent.
   A cancel the server gets while idle between two round trips (a prepare, the savepoint probe when unsure, `BEGIN`, a bind, `COMMIT`) does nothing: only the drop keeps the rest of the statement, a `COMMIT` included, from the server.
   One not yet sent never is; pgbg waits 3 seconds at most for the session task (`STOP_WAIT`, as for a signal), then sends the cancel again and aborts it.
   A `Session` dropped in the middle of a statement cuts its link instead of letting it finish.
   Only a server that sees pgbg go stops the statement then (`client_connection_check_interval`: PostgreSQL 14 or later, on Linux, and not behind PgBouncer in transaction pooling); elsewhere the cancel request is what stops it.

Switching connection:

1. `ctrl-o` opens the connection picker; picking one not yet open starts its sheet and returns `Action::Open { connection, session }` with a new `SessionId`, picking one whose session failed returns it again so its task opens it again, picking one closed with `ctrl-x` opens a new session, picking an open one shows its sheet and returns nothing.
2. The session task opens through `db::open_along`, as `pgbg run` does: when the connection has a tunnel that is not running, it starts it with `Tunnel::start`, which picks a free port, runs the command, and waits until the port answers.
   Every answer of the task carries its `SessionId`, and a sheet takes only those of its current session, so the words of one closed or replaced never reach the next.
3. `Session::open` takes the tunnel's `Route`: it connects to `hostaddr=127.0.0.1` and the tunnel's port, and keeps the URL's host for the TLS certificate check.
4. The session stays open until pgbg quits or `ctrl-x` in the picker closes it, so switching back is instant; its tunnel stops with it.
   `ctrl-x` keeps the sheet and its editor text, marks its session closed so a late word from the old task changes nothing, and picking it again opens a new session.
   When the config changes, the sheet of a connection gone closes and the sheet of one changed opens a new session on it (`Action::Reopen`), unless its session was closed: it only takes the connection as it is now.
   Either way a statement running on the old session fails, since its answer goes with it.
   A close or a reopen cuts a result still paging at the rows it has, since its portal closes with the old session.
   Each sheet keeps its editor text and its result while another one is shown, and the answers of its session land on it alone.
5. A session whose link is gone, or whose rollback failed, is opened again before the next statement; it is never retried on its own.
   A statement sent while its session fails to open fails with it, and is never sent on a later open; once pgbg quits, one still queued is never sent.
6. Opening picks `statement_timeout`: the connection's own, else the stricter of its tier's (none on local, 60 s on staging, 30 s on production) and the one the server already applies, read from `pg_settings`.
   Each read-only transaction sets it with `SET LOCAL` right after `BEGIN READ ONLY`, as it sets its idle timeout, so behind PgBouncer in transaction pooling both follow every transaction and never reach another client.
   A connection that allows writes also sets it on the session, since its statements run outside pgbg's transactions: behind a transaction pooler that holds only on the server link it reached, so give the role its own `statement_timeout` there.
7. Opening gives up after 10 seconds, TLS and login included, unless the `url` sets `connect_timeout`; and each session asks the server to stop its statement once pgbg is gone (`client_connection_check_interval`), best effort, as step 7 above says.

Reading the schema:

1. The schema is read once it is first needed: the schema tab drawn while the session on screen is open, or switched to, `ctrl-t`, or a first completion asked while the session is open; only switching to the tab, `ctrl-t` and `r` open a failed session again, so a typed key never does; `schema = "off"` on a connection never reads it, and `r` in the schema tab reads it again.
   `Action::ReadSchema` asks the session task, which opens its session first unless it is open; its `SchemaReader` waits for the route that session took, opens a second session along it, reading the password again from its source (a command runs again), reads the schema and `current_schemas(false)`, closes it and answers `Incoming::Schema`; no password is kept between the two opens, and a failed read is a warning, never a lost session.
   A session that does not open, or is lost, fails a read waiting on it, so `r` asks again; asking on a failed session shows it opening again, since the ask opens it.
   The reader is a future of the session task polled beside its statements, so a statement never waits on it, nor it on a statement, and it goes when the task stops.
   That second session is one more login: it takes a slot of the role's `CONNECTION LIMIT` or of a PgBouncer pool, which can make the main session's reopen fail while it lasts, and one more IAP websocket through a `gcloud` tunnel; that is why it is lazy, short and can be turned off.
   The catalog query keeps the first 20 000 tables in name order (`LIMIT` in the SQL), and the status line says when it is cut.
2. The sheet keeps the `Catalog` behind an `Arc`, shared and never changed; a new read replaces it whole, and a sheet opened again on a changed connection reads it again when next needed.

Completing a name:

1. Once the schema reader answers `Incoming::Schema`, it builds the `complete::Index` from that `Catalog` on a blocking thread and answers `Incoming::Index` with both; the sheet keeps the index behind an `Arc` while the catalog it was built from is still the one read.
2. Only a key that types an identifier character or `.` asks for candidates, and only when the cursor ends the word; the first ask reads the schema while the session is open, and a failed or closed session stays so, and until the index comes the keywords alone complete; the context comes from the tokens of the statement under the cursor alone.
3. The ghost is prefix only, a binary search in each of vtui's indexes: the rest of the shortest name longer than what was typed shows dimmed after the cursor, and `→` at the end of the word takes it, quoted when its name needs it; any other key drops it, one that acts (`⏎`, `⇥`, `esc`, `ctrl-s`) too, and it shows only while the editor has the keys.
4. `ctrl-space` opens the list in a popup at the cursor, where a fuzzy match joins the prefix ones: vtui ranks each list (prefix matches shortest first, then fuzzy ones by score, only to fill 50), and pgbg merges them: a prefix before a fuzzy match, a column of a table the statement names before any other, the shorter prefix match first, each name once; at most 50 are kept; `↑↓` pick, `⏎` or `⇥` take, `esc` closes, and typing on narrows it, while `⇥` outside it still switches panes.
   When no name fits, it opens or stays open saying `no match` and `⏎` and `⇥` do nothing, so a half-typed name never runs; a paste closes it and goes to the editor.
5. A test holds the budget: 20 000 tables and 200 000 columns, one key, under a millisecond in a release build.

Running a statement with parameters:

1. `query::params` reads the `{{name}}` of the statement under the cursor from `db::lex`'s tokens, never inside a string, a comment or a quoted name, and the header's `-- param: since date = 2024-01-01`, `-- param: owner text optional`.
   A statement holding both a `$n` of its own and a `{{name}}` is refused, and a `{{` the lexer cannot read as a name is too: a doubt never runs.
2. The parameter bar above the editor shows one field per name, an empty one with what it runs with (its default, `NULL` or `required`); `⇥` from the editor reaches its first field, `⇥` and `⇧⇥` step through them, `⏎` in a field runs and a modified `⏎` types nothing, `esc` cancels a running statement, else goes back to the editor.
   The values typed stay on the sheet, by name, until pgbg quits and are never written to disk; the run carries them and the header's `-- param:` lines to the session task, which fills and admits the statement as `pgbg run` does.
3. Each name becomes `$1`, `$2` (the same name the same number), so the guard classifies what the server gets; `admit` takes the values with the statement and, when values came, refuses one whose count of values is not its count of `$n`; a statement given none (`CREATE FUNCTION add1(int) RETURNS int RETURN $1 + 1`) is left to the server.
   A name left empty, with no default and not `optional`, stops the run before anything is sent; an `optional` one is bound as NULL.
   A `--param` for a name the statement does not hold, or a name given twice, stops the run too; a params file may name more, so one file serves several queries, and a line of it that is no `name=value` is told by its number only.
4. Both ways of running bind them: the read-only portal and the one a connection that allows writes uses.
   The statement is prepared with unknown types (id 0) where the header names none, so the server infers them; a header type is looked up first with `SELECT to_regtype($1)::oid`, the name itself bound, and a type the server does not know stops the run.
   Each value goes through one `ToSql` wrapper that accepts any type and encodes as text (`encode_format`): the server parses it with that type's own input, and a value it cannot read fails with its words.
5. The history keeps the statement with its `{{name}}`, never the values; a run its parameters stopped, a type the server does not know included, is `refused`.
6. `pgbg run` reads the `-- param:` lines heading its statement, and takes `--param name=value`, `--params-file <file>` (one `name=value` a line) or `--params-file -` on stdin, so a value need not sit in argv or the shell's history.

Asking a command for SQL (planned):

1. `[ai] command` in `config.toml` names any program that reads a prompt on stdin and writes SQL on stdout (`llm`, a wrapper around ollama, `claude -p`); without it, or with a program that cannot be run, the feature is off and the keys say so.
   The `claude` example turns its tools off (`["claude", "-p", "--tools", "", "--strict-mcp-config"]`): a table or column name can carry instructions to the model, and any process of the user, the command included, can read pgbg's keychain items.
2. A connection says `ai = "off"` to turn it off; it is on otherwise, on every tier and whether the connection allows writes, because of the invariant below.
3. `ctrl-g` in the editor asks for an instruction on the footer line.
   The prompt holds only:
   - the instruction, sent as typed: whatever the user writes in it leaves the machine;
   - the statement under the cursor with every literal masked (a string as `'<literal 1>'`, a number as `<number 2>`, found with `db::lex`) and its comments stripped, a parameter as its `{{name}}`;
   - a slice of the schema, names and types only: the tables the statement names and the closest matches in the index to the instruction's words, with their columns;
   - the tier.
   It never holds a row, a value from a table, a default, the history, a server's words, nor the connection's name, host or URL; a test builds a prompt from a fixture holding each of them and finds none.
   The answer's markers are put back from the masked list before it is shown, and a marker of no literal, or of one of another kind, stops it; `[ai] literals = "sent"` sends the literals as typed, comments still stripped.
   A statement holding `PASSWORD` or a URL with a password, by the rule the history uses, is never sent: the ask stops and says why.
   The prompt is capped at 16 KiB, the schema slice cut first, table by table, and the prompt says when it was.
4. The command runs from a new empty temp folder, removed after, with a cleared environment: `PATH`, `HOME`, `LANG`, `TERM` and the names `[ai] env` lists, never a variable a connection's `password = { env = ... }` names, even when listed.
   It runs in its own process group in `groups.rs`, through a helper of its own there: stdin written by its own task then closed, stdout read at the same time and cut at 64 KiB, stderr's last lines kept, the group killed on a drop; `groups::output` would deadlock on a big prompt and keeps no cap.
   It gets 60 s at most (`[ai] timeout`); `Esc`, a new ask or a closed sheet drops it, which kills the group, and a late answer is dropped by its `AskId`.
   No streaming: the answer is shown whole, once the command exits with 0; another exit shows its last lines of stderr.
5. `ai::answer` drops a Markdown fence and the prose around it, and splits what is left with `db::lex`: one statement is shown as a diff against the statement under the cursor, several as a list to pick one from, all drawn through `render::printable`.
   An answer `db::guard` classifies as a write says so in the failure colour, above the diff.
   `tab` takes it, replacing the statement under the cursor (or inserting at the cursor when there is none) through the editor's paste, so its control characters are dropped as a paste's are; `esc` drops it.
   `⏎` takes nothing, so taking an answer and running it are always two keys; running it goes through `db::guard` like any statement.
6. The idle ghost, when `[ai] ghost = true`, asks for the rest of the statement after 1 s without a key, the local completion's ghost first; any key kills that command, and its answer shows only while the text is the one it was asked for.
7. The log says which program ran, how long, how it exited and how many bytes went each way; never the prompt, the instruction or the answer.

Formatting a cell:

1. A result opens with every column in the format the header's `-- format:` names, else the config's `[display] format` (`raw` or `readable`), else raw.
2. `f` on a column steps through its kind's formats, `F` sets every column raw or readable (a `timestamp` without time zone stays raw, as relative reads it as UTC); the sheet keeps the choice by column index with the result, and the grid widths are measured again on the first page, a relative column at least as wide as `59 min ago`.
3. The loop reads the clock and the local time zone before each frame and hands them to the app as a `Clock`, so the app and `format/` stay pure; a relative time redraws each minute while one is shown.
   A header format a column's kind does not allow is left out, and a header name matches a column whatever its case.
   The row's detail shows values raw and whole.
4. Formats are for reading only: `pgbg run`, `--json` and a copy give raw values.

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
- `format/` is pure: a `Value`, a `Format` and a `Clock` in, the cell's text out; it never reads the clock itself.
- A program that takes the terminal (`$EDITOR` on a saved query) runs from the loop through `Screen::lend`, with the key stream dropped and, through vtui, pastes given back until it exits; the query is read again after.
- Secrets are read only in `secret/` and sent only by `Session::open` and the keychain write.

## Read only

Four layers, outermost first; only the last one is a guarantee.

1. **Policy**: each connection chooses; it is read only unless it says `writes = "allowed"`, whatever its tier.
2. **Guard**: `db::guard` lets through `SELECT`, `WITH` without a data-changing part, `VALUES`, `TABLE`, `EXPLAIN` of a `SELECT`, `WITH`, `VALUES` or `TABLE` only, whatever its options, since a quoted `"analyze"` runs the statement too, and `SHOW`, without a row lock (`FOR UPDATE`, `FOR SHARE`) or `SELECT ... INTO`; anything else is refused locally.
3. **Transaction**: one statement per round trip (the extended protocol refuses more), inside `BEGIN READ ONLY`, always rolled back; the server refuses writes even when the guard misses one.
4. **Role**: a read-only database role. A read-only transaction still lets a function with side effects run (`pg_terminate_backend`, `dblink`); only the role stops those.

A parameter is always bound, never put in the statement's text: its value cannot change what the guard admitted.

The guard reads a backslash in a string as itself, and blanks as PostgreSQL does (` `, tab, newline, carriage return, form feed, vertical tab; any other character outside ASCII is part of a word).
So the server reads strings the same way, every read-only transaction sets `SET LOCAL standard_conforming_strings = on` with its other limits, and a connection that allows writes sets it for its session.
That session setting is the one gap, on `Mode::Writes` only: a transaction pooler keeps it on the server link it reached alone, and the user's own `SET standard_conforming_strings = off` turns it off, so there the server can read a backslash in a string as the guard does not.

On a connection that allows writes, a write runs only once `y` or `--yes` confirms it.
In the TUI the `y` gives the `Action::Run` a `Confirmed`, which only `tui/app/confirm.rs` builds, and the session task refuses a write that comes without one.
A statement runs as typed, except one that returns rows and is no `CALL` or `DO` (`WritePath`): outside a transaction of the user it goes through a portal in its own committed transaction; inside one, a `SELECT`, `WITH`, `VALUES` or `TABLE` the guard reads as changing nothing goes through `DECLARE CURSOR`, named `pgbg_result_<n>` with the first `n` that no cursor open in `pg_cursors` holds, so it meets none of the user's, and `FETCH` of its first 10 000 rows, then `CLOSE` whether the fetch failed or not, so pgbg sends no `BEGIN`, `COMMIT` or `ROLLBACK` there.
The driver's transaction handle is never used inside the user's transaction: its `START TRANSACTION`, failing in a failed transaction or cancelled, sends a `ROLLBACK` that would end the user's.
A `CALL` or a `DO` block, and any other statement returning rows inside the user's transaction, runs as is, every row read; a procedure may `COMMIT`, which it cannot inside a transaction pgbg opened.
The session tracks that transaction from the first words of each statement the server ran (`BEGIN`, `START`, `COMMIT`, `END`, `ROLLBACK`, `ABORT`, `PREPARE TRANSACTION`, with `AND CHAIN` and `TO SAVEPOINT` read too); when one of those failed, never got its answer or reads unclear, or after a `CALL` or a `DO` block, which may commit inside, a `SAVEPOINT` probe asks the server once.
pgbg's own read-only transaction (`Session::read_only`) is refused with `InsideUserTransaction` while the user's is open, since its `ROLLBACK` would end it.

`pgbg doctor <connection>` checks the role and says what it can write.

## Secrets

- A connection names where its password comes from: `password = { keychain = "pgbg/prod" }`, `{ env = "PROD_PGPASSWORD" }` or `{ command = ["gcloud", "secrets", "versions", "access", ...] }`.
- The password is read when the session opens, kept in a `Secret`, and dropped once connected.
  `tokio_postgres`'s `Config` keeps its own copy until the session is open, and that copy is not wiped; nor is what `security` or a password command leaves in the memory of the process that read its output.
- The config loader refuses a plain `password`, and a URL with a password in it.
- Keychain items live under the service `pgbg`, one account per item; `security -g` reads one on a pipe, in hex when it is not printable ASCII, and `security -i` writes one so the password never shows in argv; an error quotes `security`'s exit status only.
- `pgbg password set <connection>` takes the password typed hidden or piped in, never as a flag.
- `pgbg connection remove` deletes the connection's keychain item before its table, unless `--keep-password` or another connection names the same item.
- A statement in the history may hold personal data in its literals (`WHERE email = '...'`): that is why its file is `0600` and never leaves this machine.

## Tunnels

- A tunnel is a command with a `{port}` placeholder: `ssh -N -o ControlPath=none -o ExitOnForwardFailure=yes -L 127.0.0.1:{port}:localhost:5432 bastion`, `gcloud compute start-iap-tunnel ... --local-host-port=localhost:{port}`, `cloud-sql-proxy --port {port} ...`.
- An optional `before` command runs first, once, within 2 minutes, and is safe to run twice (starting a stopped bastion); like a password command, it runs in a process group of its own, killed whole when pgbg stops waiting on it.
- pgbg picks the free port, so two pgbg can run side by side; the child runs in its own process group, and stopping or dropping the `Tunnel` kills the whole group, so an `ssh` or `gcloud` it started dies too.
- Only what stays in that group dies with it: an ssh `ControlMaster` or `ssh -f` keeps the forward elsewhere, so `stop` fails when the port still answers, and a command that ends cleanly before its port answers is said to have handed the forward off.
- pgbg cannot make the command bind `127.0.0.1` only, nor know who answers on the port: the examples bind it explicitly, and a tunnelled connection needs TLS or `tls = "off"` on purpose, never the clear a local host gets.
- A tunnel is ready once its port takes a TCP connection while its command still runs, within 30 seconds; one that dies first says how it exited and its last lines of stderr, never its arguments.
- `SIGINT`, `SIGTERM` and `SIGHUP` stop pgbg through `signal.rs`: `pgbg run`, `pgbg doctor` and `pgbg schema` have the server cancel their statement (within a second) while the link and the tunnel still stand, stop the tunnel, and are dropped past 3 seconds or on a second signal, so every `Drop` runs and the tunnel goes; the TUI quits as `ctrl-c` does, cancelling every running statement, and each session task stops its tunnel, warning when its port still answers, unless a second signal drops them at once; pgbg exits with `128 + n`. Only a `SIGKILL` leaves a tunnel behind.
- A session through a tunnel connects to `hostaddr=127.0.0.1` on the tunnel's port, and TLS still checks the certificate against the URL's host; that is why the config refuses a `hostaddr` of its own.
- The session task owns the tunnel with the session: the status line shows `⇄ tunnel :<port>`, `starting…` or how it died; a tunnel that dies closes its session and says why, and the next statement starts both again.
- A session reopened while its tunnel still runs keeps that tunnel.
- Release builds abort on a panic, so no `Drop` runs: `groups.rs` keeps every live process group in a registry, and a panic hook kills them all.
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
- An optional header of `--` lines sets the name, the default connection, the chart, the formats and the parameters; a file without a header is still a query.
- The header is read again whenever the editor's text changes; a line it cannot read is a warning in the editor's title, never a failure.
- `-- connection:` only suggests: the editor's title names it, in red when it is a production connection, and a statement always runs on the session on screen.
- The file is the source of truth: pgbg reads it when it is opened and writes it back when it is saved, nothing else.
- A save writes a temporary file beside the query and renames it over; it makes the missing folders, refuses a link or a root that changed, and a new query never replaces another file, so saving twice changes nothing.
- A new query goes under the project's root, else under the user's; opening another query over unsaved text asks first.

## Performance

- Rows stream through a portal, a page at a time; nothing waits on the whole result.
- The grid draws only the visible window; column widths come from the first page and the header, capped.
- Values are decoded once, at the boundary, into `Value`; drawing formats only what is on screen, and the row's detail at most 64 Ki characters of a value, its lines built once per row and width and kept by the app.
- Syntax colours live in `tui/app/editor.rs` with the text they colour: an edit or an opened query marks them stale, and the loop rebuilds them once the text stayed still for 40 ms, so a key never waits on tree-sitter.
- The editor view draws only the lines in view, each in one pass over its spans.
- vtui's `Screen` asks the terminal for bracketed paste: a paste comes as one `Event::Paste`, inserted whole, so its line ends never run a statement; `Field::paste` and `Palette::type_str` drop control characters and text-reversing marks.
- Text from the database or the server (grid cells, the row's detail, chart labels and names, a failed run, the status line) is drawn through `render::printable`, its control characters and text-reversing marks escaped.

## Glossary

One word per concept, in code, docs and UI.

| Word | Means |
|---|---|
| connection | A named database target in the config: where, how to reach it, how to log in |
| tier | `local`, `staging` or `production`; sets the colour and the timeout |
| session | An open link to one connection, with its tunnel if it has one, named by its `SessionId`; its `SessionState` (opening, open, failed, closed) is what the status line says |
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
| catalog | The schema as one read gave it, with the search path it was read under and whether the table cap cut it |
| completion | A name the editor offers at the cursor, from the schema or the keywords |
| parameter | A `{{name}}` in a statement, filled in the parameter bar and bound when it runs |
| format | How a column's cells read: raw, or a readable form of its kind |
| ask | One sending of an instruction to the `[ai] command`, named by its `AskId`; its answer is a suggestion, never a run |
