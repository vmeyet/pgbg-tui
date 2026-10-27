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

- [ ] The schema read on a second session along the same route when first needed, its password read again, capped at 20 000 tables in the SQL, with `current_schemas(false)`; kept by its sheet, read again with `r`, `schema = "off"` on a connection
- [ ] Schema sidebar: a second tab beside the queries (`[` `]`), schemas, tables, columns and indexes, `/` to filter
- [ ] `ctrl-t` finds a table from anywhere, fzf-narrowed like `ctrl-p`, with the same actions as the sidebar
- [ ] From a table in the sidebar or the finder: `p` previews `SELECT * FROM <table> LIMIT 100`, `i` puts its name in the editor, `⏎` shows its columns, types and indexes in the result pane

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
- [ ] `complete/`, replacing `Schema::complete`: what the cursor wants (a table, a column of a table or alias, a keyword) from the statement under it, names in the search path bare; a prefix search in the index, asked only after an identifier character or `.`; a test holds it under a millisecond on 200 000 columns
- [ ] Ghost in the editor: the rest of the best candidate, dimmed after the cursor, prefix only, taken with `→` at the end of the word
- [ ] `ctrl-space` lists the candidates, prefix then fuzzy, in a popup at the cursor: `↑↓` to pick, `⏎` or `⇥` to take, `esc` to close

## 13. Parameters

- [x] Bound, never spliced, by both ways of running: `Admitted` carries the values and checks their count against the statement's `$n`, each goes in text through one `ToSql` wrapper, typed by the server's inference
- [x] `{{name}}` in a statement, outside strings and comments; each name becomes `$n`, so the guard reads the statement as sent; one mixing `$n` and `{{name}}`, or a `{{` the lexer cannot read, is refused; `pgbg run --param` and `--params-file`
- [ ] `-- param: name type = default` and `-- param: name type optional` in the header: a type looked up with a bound `to_regtype`, an unknown one stopping the run; an `optional` one left empty bound as NULL
- [ ] The parameter bar above the editor: one field per parameter of the statement under the cursor, `⏎` runs, an empty one without a default stops the run; the values stay on the sheet while pgbg runs, never on disk

## 14. Formats

- [x] `format/`: each column kind's formats (timestamps local or relative, numbers grouped or as a size, UUID short, bytes as a size), pure, the clock passed in
- [ ] The grid draws its visible cells through them: `f` cycles the column under the cursor, `F` sets every column raw or readable
- [ ] `-- format: created_at=relative size=bytes` in the header, and a default in `config.toml`; raw when neither says

## 15. Asking a command for SQL

Needs step 12: the lexer, the statement under the cursor and the schema's index.

- [ ] `ai/`: the prompt built from the instruction, the statement under the cursor (string literals masked, parameters as `{{name}}`), the schema slice and the tier, capped at 16 KiB, and refused for a statement holding a password; the answer read back as SQL statements with its literals put back; pure, tested
- [ ] `[ai] command` in `config.toml`, `ai = "on"` or `"off"` on a connection (off on production and on writes allowed unless it says on); the command run from an empty temp folder with a cleared environment (`PATH`, `HOME`, `LANG`, `TERM`, `[ai] env`, never a password variable), in its own process group through a `groups.rs` helper that writes stdin and reads stdout at once, cut at 64 KiB, 60 s at most, `Esc` kills it; tested with a fake command
- [ ] `ctrl-g` asks: an instruction on the footer line, then the answer shown as a diff of the statement under the cursor, a write said so in the failure colour; `tab` takes it through the editor's paste, `esc` drops it, `⏎` does nothing; nothing runs by itself
- [ ] The idle ghost, off by default (`[ai] ghost = true`): after 1 s without a key, the rest of the statement asked for and dimmed after the cursor, the local ghost first; a key kills the command

## Later

- Writes on connections that allow them, with a preview and a typed confirmation
- More chart kinds: a new `ChartKind` variant and one file in `chart/` and `tui/chart_view/`
