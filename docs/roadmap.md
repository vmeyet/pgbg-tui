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
- [ ] The same from the picker: a connection form in the TUI

## 5. Tunnels

- [x] `tunnel/`: free port, command with `{port}`, wait for the port, process group, stop on quit
- [x] `before` command; tunnel state in the status line

## 6. Saved queries

- [x] `ctrl-p` fuzzy finder over the `.pgbg/queries/` nearest the working directory, opening a query in the editor
- [x] The user config folder's queries, beside the project's
- [x] `.pgbg/queries/` tree in a sidebar
- [x] Header parsing: `-- name:`, `-- connection:`, `-- chart:`
- [ ] Save from the editor, open in `$EDITOR`

## 7. Big results

- [ ] Portal pages fetched as the grid scrolls
- [x] History per connection: `pgbg run` adds each statement, `pgbg history <connection>` lists them
- [ ] The TUI adds its statements to the history, `ctrl-r` searches it

## 8. Schema

- [ ] Schema sidebar: schemas, tables, columns, indexes
- [ ] Completion of tables and columns in the editor

## 9. Reading a row

- [ ] Row detail and JSON view
- [ ] Copy a cell, a row or the result as CSV, JSON or Markdown

## 10. Charts

- [x] `chart/`: `ChartSpec`, `chart::infer`, `Series`, all pure and tested
- [x] Bar chart and line chart views, `v` to switch between grid and chart
- [x] `-- chart:` in a saved query's header

## 11. Doctor

- [x] `pgbg doctor <connection>`: config, TLS, tunnel, login, timeout; `--json`, exit 1 on a failed check
- [ ] Role privileges: what the role can write, a warning when a read-only connection's role can

## Later

- Writes on connections that allow them, with a preview and a typed confirmation
- More chart kinds: a new `ChartKind` variant and one file in `chart/` and `tui/chart_view/`
