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
| `config/` | `config.toml`: connections, theme, keys; load, validate, refuse secrets |
| `connection/` | `Connection`: name, tier, target, tunnel, secret source, write policy, TLS; parsed once from config |
| `secret.rs` | `Secret` (prints `<redacted>`) and its sources: keychain through `/usr/bin/security`, environment variable, command; `SecretSource::read` reads one when a session opens |
| `tunnel/` | `Tunnel`: a child process forwarding a free `127.0.0.1` port to the database; start, wait for the port, stop |
| `db/` | The only module that sees `tokio_postgres`: connect over rustls, run a statement, stream rows, cancel |
| `db/model/` | The neutral model: `ResultSet`, `Column`, `ColumnKind`, `Value`, `Page`, `Schema`, `Table` |
| `db/guard.rs` | Classifies a statement before it is sent: read, write, or more than one statement; `admit` turns a statement into an `Admitted`, the only kind a session runs |
| `db/decode.rs` | Wire values to `Value`, by column type |
| `query/` | Saved queries: the `.pgbg/queries/` tree, the header (`-- name:`, `-- chart:`), load and save |
| `history.rs` | Every statement run, per connection, in a local file |
| `chart/` | `ChartSpec` (kind, x column, y columns) and `Series` built from a `ResultSet`, both pure; one file per chart kind |
| `syntax/` | SQL colours from tree-sitter, loaded on first use |
| `act.rs`, `keymap.rs` | Every action in one table (name, default keys, help, palette verb, pane); the user's `[keys]` on top |
| `tui/mod.rs` | The event loop: terminal events, answers and ticks in, frames out |
| `tui/app/` | `App`, the pure state machine: keys in, `Action`s out, `Incoming` answers applied |
| `tui/actions.rs` | The action runner: one task per `Action`, answers sent back as `Incoming` |
| `tui/backend/` | What the runner calls, one part per concern: sessions (one per open connection), queries, history |
| `tui/ui.rs`, `tui/*_view.rs` | Drawing: layout in `ui.rs`, one view module per pane or modal |
| `tui/grid_view/` | The results grid: only the visible rows and columns are drawn |
| `tui/chart_view/` | Draws a `Series` with ratatui's `BarChart` and `Chart` |
| `tui/theme.rs` | The tier colours on top of vtui's palette |
| `vtui` (git dependency) | Shared with the sibling TUIs: the terminal in raw mode (`Screen`), the palette (`Theme`), the input box (`Field`), the `ctrl-k` palette, the `?` overlay, fzf-style matching |
| `log.rs` | The log file; never a secret, never a row |

## Data flow

Running a statement:

1. The editor hands `App` the statement under the cursor; `App` returns `Action::Run { session, sql }`.
2. `db::guard` classifies it; a write on a read-only connection comes back as `Incoming::Refused` and never reaches the server.
3. The session opens `BEGIN READ ONLY`, sets `statement_timeout` from the tier, and declares a cursor for the statement.
4. The first page (`FETCH 500`) comes back as `Incoming::Page`; the grid paints it, and more pages are fetched as the cursor nears the end.
5. `Esc` on a running statement sends the server a cancel request; the transaction is always rolled back at the end.

Switching connection:

1. `ctrl-o` opens the connection picker; picking one returns `Action::Open { connection }`.
2. When the connection has a tunnel, `tunnel::start` picks a free port, runs the command, and waits until the port answers.
3. `db::connect` opens the session through the tunnel and reads the schema in the background for completion and the sidebar.
4. The session stays open until pgbg quits, so switching back is instant; its tunnel stops with it.

Charting a result:

1. `v` on a result asks `chart::infer` for a `ChartSpec`: the first text or date column as x, the numeric columns as y.
2. A saved query can set it in its header instead: `-- chart: bar x=day y=orders`.
3. `chart::Series::from_result` builds the points; `chart_view` draws them.

## Boundaries

- Only `src/db/` sees a `tokio_postgres` type; everything else uses `db/model/`.
- Side effects live at the edges: `main.rs`, `commands/`, `tui/mod.rs`, `tui/actions.rs`, `tui/backend/`, `tunnel/`, `secret.rs`.
- `tui/app/` is pure: it reads time from `app.now`, set by the loop, and never does I/O; its tests drive it with keys and `Incoming` values.
- `chart/` is pure: a `ResultSet` in, a `Series` out; drawing lives in `tui/chart_view/`.
- A program that takes the terminal (`$EDITOR` on a saved query) runs from the loop through `Screen::lend`, with the key stream paused.
- Secrets are read only in `secret.rs` and sent only by `db::connect`.

## Read only

Four layers, outermost first; only the last one is a guarantee.

1. **Policy**: each connection chooses; it is read only unless it says `writes = "allowed"`, whatever its tier.
2. **Guard**: `db::guard` lets through `SELECT`, `WITH` without a data-changing part, `VALUES`, `TABLE`, `EXPLAIN` without `ANALYZE`, and `SHOW`, without a row lock (`FOR UPDATE`, `FOR SHARE`) or `SELECT ... INTO`; anything else is refused locally.
3. **Transaction**: one statement per round trip (the extended protocol refuses more), inside `BEGIN READ ONLY`, always rolled back; the server refuses writes even when the guard misses one.
4. **Role**: a read-only database role. A read-only transaction still lets a function with side effects run (`pg_terminate_backend`, `dblink`); only the role stops those.

`pgbg doctor <connection>` checks the role and says what it can write.

## Secrets

- A connection names where its password comes from: `password = { keychain = "pgbg/prod" }`, `{ env = "PROD_PGPASSWORD" }` or `{ command = ["gcloud", "secrets", "versions", "access", ...] }`.
- The password is read when the session opens, kept in a `Secret`, and dropped once connected.
- The config loader refuses a plain `password`, and a URL with a password in it.

## Tunnels

- A tunnel is a command with a `{port}` placeholder: `ssh -N -L {port}:localhost:5432 bastion`, `gcloud compute start-iap-tunnel ... --local-host-port=localhost:{port}`, `cloud-sql-proxy --port {port} ...`.
- An optional `before` command runs first, once, and is safe to run twice (starting a stopped bastion).
- pgbg picks the free port, so two pgbg can run side by side; the child runs in its own process group and is killed with it on quit or crash.
- The status line shows the tunnel state; a tunnel that dies closes its session and says why.

## Saved queries

- One `.sql` file per query under `.pgbg/queries/`, found by walking up from the working directory like `.git`, then under the user config folder.
- Folders nest; the sidebar shows the tree and the fuzzy finder searches every file.
- An optional header of `--` lines sets the name, the default connection and the chart; a file without a header is still a query.
- The file is the source of truth: pgbg reads it when it is opened and writes it back when it is saved, nothing else.

## Performance

- Rows stream through a cursor, a page at a time; nothing waits on the whole result.
- The grid draws only the visible window; column widths come from the first page and the header, capped.
- Values are decoded once, at the boundary, into `Value`; drawing formats only what is on screen.
- Syntax colours are rebuilt only when the editor text changes.

## Glossary

One word per concept, in code, docs and UI.

| Word | Means |
|---|---|
| connection | A named database target in the config: where, how to reach it, how to log in |
| tier | `local`, `staging` or `production`; sets the colour and the timeout |
| session | An open link to one connection, with its tunnel if it has one |
| tunnel | The child process that forwards a local port to a database out of reach |
| statement | One SQL statement, the unit pgbg runs |
| query | A saved `.sql` file under `.pgbg/queries/` |
| result | The columns and rows a statement returned, page by page |
| page | One batch of rows fetched from the cursor |
| chart | A bar or line drawing of a result |
| history | Every statement run on a connection |
