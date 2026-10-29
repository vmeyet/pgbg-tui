# Roadmap

Each step is one MR under 500 lines, in order.
The MR that ships a step ticks its box.

## 0. Ground

- [x] Crate, lints, `clippy.toml`, `deny.toml`, CI (fmt, clippy, test, msrv, deny, machete)
- [x] `pgbg --version`, the log file, `config.toml` loaded and validated, `pgbg connections` lists them

## 1. Read from the command line

- [x] `db/`: connect over rustls, one read-only statement, decode the common types into `db/model/`
- [x] `db::guard`: refuse anything that is not a read, with a test per statement kind
- [x] `pgbg run <connection> "<sql>"`: a plain table, or `--json`
- [x] A PostgreSQL service container in CI for the `#[ignore]`d tests

## 2. The TUI shell

- [x] Event loop, `App`, `Action`, `Incoming`, screen, theme
- [x] SQL editor pane and results grid (visible window only), status line with the connection and its tier colour
- [x] Run the statement under the cursor, cancel with `Esc`, errors with the server's position marked in the editor

## 3. Syntax colours

- [x] `syntax/` on `tree-sitter-sequel`, rebuilt only on edit

## 4. Connections

- [x] Tiers and their colours, `statement_timeout` per tier
- [x] Secret sources: keychain, environment, command
- [x] `ctrl-o` connection picker; sessions stay open, switching back is instant
- [x] `pgbg connection add|edit|remove` and `pgbg password set <connection>`: the config rewritten key by key, the password written to the keychain with `security -i`
- [x] The same from the picker: a connection form in the TUI

## 5. Tunnels

- [x] `tunnel/`: free port, command with `{port}`, wait for the port, process group, stop on quit
- [x] `before` command; tunnel state in the status line

## 6. Saved queries

- [x] `ctrl-p` fuzzy finder over the `.pgbg/queries/` nearest the working directory, opening a query in the editor
- [x] The user config folder's queries, beside the project's
- [x] `.pgbg/queries/` tree in a sidebar
- [x] Header parsing: `-- name:`, `-- connection:`, `-- chart:`
- [x] Save from the editor with `ctrl-s`, asking before an open drops unsaved text
- [x] Open in `$EDITOR` with `ctrl-e`, read again when it exits

## 7. Big results

- [x] Portal pages fetched as the grid scrolls
- [x] History per connection: `pgbg run` adds each statement, `pgbg history <connection>` lists them
- [x] The TUI adds its statements to the history, `ctrl-r` searches it

## 8. Schema

- [x] The schema read on a second session along the same route when first needed, its password read again, capped at 20 000 tables in the SQL, with `current_schemas(false)`; kept by its sheet, read again with `r`, `schema = "off"` on a connection
- [x] Schema sidebar: a second tab beside the queries (`[` `]`), schemas, tables, columns and indexes, `/` to filter
- [x] `ctrl-t` finds a table from anywhere, fzf-narrowed like `ctrl-p`, with the same actions as the sidebar
- [x] From a table in the sidebar or the finder: `p` previews `SELECT * FROM <table> LIMIT 100`, `i` puts its name in the editor, `⏎` shows its columns, types and indexes in the result pane

## 9. Reading a row

- [x] Row detail and JSON view
- [x] Copy a cell, a row or the result as CSV, JSON or Markdown

## 10. Charts

- [x] `chart/`: `ChartSpec`, `chart::infer`, `Series`, all pure and tested
- [x] Bar chart and line chart views, `v` to switch between grid and chart
- [x] `-- chart:` in a saved query's header

## 11. Doctor

- [x] `pgbg doctor <connection>`: config, TLS, tunnel, login, timeout; `--json`, exit 1 on a failed check
- [x] Role privileges: what the role can write, a warning when a read-only connection's role can

## 12. Completion

- [x] `db/lex.rs`: the guard's tokenizer on its own, shared by the guard, completion and parameters
- [x] `complete/`, replacing `Schema::complete`: what the cursor wants (a table, a column of a table or alias, a keyword) from the statement under it, names in the search path bare; a prefix search in the index, asked only after an identifier character or `.`; a test holds it under a millisecond in a release build on 200 000 columns, the ghost and the `ctrl-space` list
- [x] Ghost in the editor: the rest of the best candidate, dimmed after the cursor, prefix only, taken with `→` at the end of the word
- [x] `ctrl-space` lists the candidates, prefix then fuzzy, in a popup at the cursor: `↑↓` to pick, `⏎` or `⇥` to take, `esc` to close

## 13. Parameters

- [x] Bound, never spliced, by both ways of running: `Admitted` carries the values and checks their count against the statement's `$n`, each goes in text through one `ToSql` wrapper, typed by the server's inference
- [x] `{{name}}` in a statement, outside strings and comments; each name becomes `$n`, so the guard reads the statement as sent; one mixing `$n` and `{{name}}`, or a `{{` the lexer cannot read, is refused; `pgbg run --param` and `--params-file`
- [x] `-- param: name type = default` and `-- param: name type optional` in the header: a type looked up with a bound `to_regtype`, an unknown one stopping the run; an `optional` one left empty bound as NULL
- [x] The parameter bar above the editor: one field per parameter of the statement under the cursor, `⏎` runs, an empty one without a default stops the run; the values stay on the sheet while pgbg runs, never on disk

## 14. Formats

- [x] `format/`: each column kind's formats (timestamps local or relative, numbers grouped or as a size, UUID short, bytes as a size), pure, the clock passed in
- [x] The grid draws its visible cells through them: `f` cycles the column under the cursor, `F` sets every column raw or readable
- [x] `-- format: created_at=relative size=bytes` in the header, and a default in `config.toml`; raw when neither says

## 15. Craft

Before more features land on top: the code keeps its own rules, and big results cost less.
A performance step measures before and after, and the MR gives the numbers.

- [x] Doc comments, one MR per top-level module: drop the ones that restate the code, write the rest in plain English
- [x] `App` and `Form` keep their fields private; the views read them through methods, and `App::paste` is one `match` on the overlay
- [x] Newtypes where the rules want them: `Column.type_name` as `TypeName`, the column name and the parameter values; the form's `(Slot, String)` alias becomes a named struct, and stops sharing the `Refusal` name with `db::guard`
- [x] `db/mod.rs` split under 800 lines along a concern
- [x] `tui/backend/session.rs` split under 800 lines along a concern
- [x] `Act::ALL` as a plain array; the `after` chain and its `const fn` loops go
- [x] The session task: an explicit state in place of the pending `Option<Woken>`, a `Notify` for schema asks, `Cancels.latest` private, no silent `self.session.as_mut()?`
- [x] A write statement knows whether the user's transaction is open without the `SAVEPOINT` probe's two round trips
- [x] `db::guard` lets `EXPLAIN ANALYZE` of a read through; `pgbg doctor` warns when the role can call what a read-only transaction does not stop (`pg_signal_backend`, `dblink`, `set_config`)
- [x] Performance: one `pg_settings` query reads both timeouts when a session opens
- [x] Performance: the SQL is lexed once per edit and shared by the guard, the statement under the cursor and completion; a word compares without an upper-case `String`, and the guard reads `Placed` without a second `Vec`
- [x] Performance: a result's cells cost less memory (`Box<str>` text, or rows stored by column), measured on 10 000 rows of 50 columns
- [x] Performance: a statement inside a transaction the user began reads its rows through a portal too, so the server sends nothing past the cap

## 16. Confirm a write

- [x] A statement the guard classifies as a write asks before it is sent, on every tier: the footer says its verb and target (`DELETE from orders?`), `y` sends it, any other key cancels and nothing is sent; only a connection that allows writes ever gets that far, a read-only one still refuses it

## 17. Asking a command for SQL

Needs step 12: the lexer, the statement under the cursor and the schema's index.

- [x] `ai/`: the prompt built from the instruction, the statement under the cursor (every string and number literal masked, comments stripped, parameters as `{{name}}`), the schema slice (names and types) and the tier, capped at 16 KiB, and refused for a statement holding a password; the answer read back as SQL statements with its literals put back; pure, tested
- [x] `[ai] command` in `config.toml`, `ai = "on"` or `"off"` on a connection (on unless it says off, whatever its tier); the command run from an empty temp folder with a cleared environment (`PATH`, `HOME`, `LANG`, `TERM`, `[ai] env`, never a password variable), in its own process group through a `groups.rs` helper that writes stdin and reads stdout at once, cut at 64 KiB, 60 s at most, `Esc` kills it; tested with a fake command
- [x] `ctrl-g` asks: an instruction on the footer line, then the answer shown as a diff of the statement under the cursor, a write said so in the failure colour and confirmed as step 16 says when it runs; `tab` takes it through the editor's paste, `esc` drops it, `⏎` does nothing; nothing runs by itself
- [x] The idle ghost, off by default (`[ai] ghost = true`): after 1 s without a key, the rest of the statement asked for and dimmed after the cursor, the local ghost first; a key kills the command

## 18. Engineering pass

The code keeps its own rules where they slipped, one MR per step.

- [x] One failure type crosses from `db/` to the app, in place of strings and a parallel `history::Outcome`
- [x] Newtypes where the rules already ask for them, parsed at the boundary
- [x] Channels between the session task and the app say why they cannot grow
- [x] The session task split by concern
- [x] Test helpers several modules share live in `fixtures`
- [x] `ResultSet` storage measured on a capped result: a frame costs the same at any row, so its rows stay as they are
- [x] `[lints]` keeps only the lints that guard a house rule or a real hazard

## 19. First real use

What a first real session asked for, one MR per step.

- [x] Config in `~/.config/pgbg`, history in `~/.local/share/pgbg`, as revu keeps its own
- [x] `ctrl-s` makes `.pgbg/queries/` when the project has none
- [x] Pane moves as in revu: `⇧⇥`, `⇧` and an arrow, a click, `←` `→` in the sidebar, `⌘k`, `?`, `^n`
- [x] The idle ghost reads an answer laid out differently from what was typed, and the log says why an ask gave nothing
- [x] A date column reads `today`, `3 days ago`; `f` on a column with one format says so
- [x] A key for a new, empty query
- [x] Undo and redo in the editor
- [x] Every save keeps the version it replaced, outside the project; a key lists them and puts one back
- [x] Completion after `JOIN` offers the table with its `ON` condition, from the foreign keys
- [x] The idle ghost shows the whole rest of the statement, and a `--` comment line above it is the instruction
- [x] `tab` takes the ghost while one shows; `esc` drops it
- [x] Usage counts as revu keeps them, opt-in, action names only, from a part both apps share in vtui
- [x] Your own saved queries belong to a connection, in the data folder; `queries = "<connection>"` shares another's; pgbg never makes a project's folder

## 20. Review of the first-use batch

The review of #194 to #208 found these, most severe first; real use of the AI ask found the last ones.

- [x] The idle ask sends a comment as its instruction only with `[ai] literals = "sent"`, and masks it otherwise
- [x] A version whose text holds a secret is kept as a note, as the history keeps it
- [x] The user's own queries folders are made `0700`, as the versions folder is
- [x] The usage file is merged under the same lock the history takes
- [ ] Editing in `$EDITOR` keeps the replaced text whatever the editor's exit status
- [ ] Undo copies the editor's text only when a key changes it, and its doc states the 1 MiB worst case
- [ ] The usage counts live in pgbg's data folder, and the cache folder follows XDG as the others do
- [ ] pgbg warns once when a config or history sits in the old macOS folder, and says where to move it
- [ ] One private-folder helper and one atomic-write helper serve the history, the log, usage, versions and saved queries
- [ ] Showing another sheet re-lists its queries in one place, and the idle ask builds its question once
- [x] The AI's schema slice shows each table's foreign keys
- [x] The slice brings the tables one foreign key away from those it keeps, the closest first within the cap
- [x] The slice shows table and column comments, one holding a secret left out
- [x] With no table named, the slice sends the table names alone
- [x] While an ask runs, the footer says `asking <command>…`
- [x] Ending a `--` line with a newline asks at once, without the idle second

## 21. A pleasure to use

Each step is one MR, in this order.

- [x] Editor moves as in a shell: `⌥b` `⌥f` by word as `⌥←` `⌥→`, `^a` `^e` to the line's ends, `^k` `^u` delete to them; in vtui's field, so every app has them
- [x] A mouse drag selects in the editor and copies on release; a double click takes a word, a triple click a line
- [x] `⌥c` copies the statement under the cursor
- [x] `⌘c` and `⌘v` copy and paste where the terminal sends them
- [x] The grid colours a value by its kind: `NULL` dimmed, one colour per repeated text value, booleans, negative numbers, keys, uuids, json and times
- [x] An FK column says so; `⏎` on its cell opens the row it points to, deeper with a trail and `⌫` back
- [x] The row's detail lists the tables that point to it; `⏎` runs that query as a new result
- [ ] `e` in that detail writes the join into the editor
- [ ] `/` filters the rows on screen and `s` sorts them by the column under the cursor, without a new query
- [ ] `=` and `!` on a cell add `WHERE column = value` or `<>` to the statement
- [ ] `i` on a column shows its distinct count, NULLs, min, max and five most common values

## Later

- A preview of what a write would change, before its confirmation
- More chart kinds: a new `ChartKind` variant and one file in `chart/` and `tui/chart_view/`
