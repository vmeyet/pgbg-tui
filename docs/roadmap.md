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

- [ ] Portal pages fetched as the grid scrolls
- [x] History per connection: `pgbg run` adds each statement, `pgbg history <connection>` lists them
- [x] The TUI adds its statements to the history, `ctrl-r` searches it

## 8. Schema

- [ ] The schema read on a second session along the same route once a session opens, kept by its sheet, read again with `r` in the sidebar; the status line says how many tables, or that it is cut
- [ ] Schema sidebar: a second tab beside the queries (`[` `]`), schemas, tables, columns and indexes, `/` to filter
- [ ] From a table in the sidebar: `p` previews `SELECT * FROM <table> LIMIT 100`, `i` puts its name in the editor, `⏎` shows its columns, types and indexes in the result pane

## 9. Reading a row

- [x] Row detail and JSON view
- [ ] Copy a cell, a row or the result as CSV, JSON or Markdown

## 10. Charts

- [x] `chart/`: `ChartSpec`, `chart::infer`, `Series`, all pure and tested
- [x] Bar chart and line chart views, `v` to switch between grid and chart
- [x] `-- chart:` in a saved query's header

## 11. Doctor

- [x] `pgbg doctor <connection>`: config, TLS, tunnel, login, timeout; `--json`, exit 1 on a failed check
- [x] Role privileges: what the role can write, a warning when a read-only connection's role can

## 12. Completion

- [ ] `db/lex.rs`: the guard's tokenizer on its own, shared by the guard, completion and parameters
- [ ] `complete/`: what the cursor wants (a table, a column of a table or alias, a keyword) from the statement under it, and the candidates ranked from the schema's index; pure, under a millisecond on 50 000 columns
- [ ] Ghost in the editor: the rest of the best candidate, dimmed after the cursor, taken with `→` at the end of the word
- [ ] `ctrl-space` lists the candidates in a popup at the cursor: `↑↓` to pick, `⏎` or `⇥` to take, `esc` to close

## 13. Parameters

- [ ] `{{name}}` in a statement, outside strings and comments, and `-- param: name type = default` in the header; each name becomes `$n`, so the guard and the statement under the cursor read the statement as sent
- [ ] Bound, never spliced: each value goes in text, and the server reads it as the type it infers or the header names; `pgbg run --param name=value`
- [ ] The parameter bar above the editor: one field per parameter of the statement under the cursor, `⏎` runs, an empty one without a default stops the run; the last values kept per query and connection

## 14. Formats

- [ ] `format/`: each column kind's formats (timestamps local or relative, numbers grouped or as a size, UUID short, bytes as a size), pure, the clock passed in
- [ ] The grid draws its visible cells through them: `f` cycles the column under the cursor, `F` sets every column raw or readable
- [ ] `-- format: created_at=relative size=bytes` in the header, and a default in `config.toml`

## Later

- Writes on connections that allow them, with a preview and a typed confirmation
- More chart kinds: a new `ChartKind` variant and one file in `chart/` and `tui/chart_view/`
