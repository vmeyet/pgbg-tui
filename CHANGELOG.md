# What's new in pgbg

Every change you can see or use, newest first.

Each entry says what changed, then where, then how to use it.

## Unreleased

### New

- **The question before a write says how many rows it would touch**, in the TUI, for an `UPDATE` or a `DELETE` on a connection that allows writes: the footer reads `DELETE from orders? 42 rows` while it waits for `y`.
  The count is a read of the same `WHERE`, run read only and rolled back within 2 s, so the write itself never runs before `y`; past 10 000 rows it says `more than 10 000 rows`.
- **Browse a table without SQL**: in the schema tab of the sidebar and in the `ctrl-t` table finder, `⏎` or a click on a table writes `SELECT * FROM` it `LIMIT 100` in the editor and runs it, so you can refine it there.
  Browsing the next table replaces that statement, unless you edited the editor since, and the result pages as any other.
  Its columns and indexes moved to `d` in the sidebar and `ctrl-d` in the finder.
- **JSON tree**: on a `json` or `jsonb` cell of the grid, `o` opens its value as a tree, coloured as the grid colours JSON.
  `⏎` folds or unfolds an object or an array, or the one holding the value under the cursor, `y` copies the value under the cursor and `Y` its path, as `.items[0].sku`.

### Fixed

- **`s` and `i` refuse a column pgbg cannot decode**, in the TUI's result grid: the status line says so, where `i` counted all its values as one.
- **`NUMERIC` values sort and count by their exact digits**, in the TUI's result grid with `s` and `i`: two values past a float's precision no longer tie.
- **`=` and `!` refuse a cell pgbg could not decode**, in the TUI's result grid: the status line says so, where the parameter bar got `<unsupported>`.
- **`=`, `!` and `i` do nothing while the filter hides every row**, in the TUI's result grid: they acted on a row the grid did not show.
- **A long schema comment no longer crowds out other tables**, in the prompt `ctrl-g` sends: each comment is cut to 300 bytes, and a table too long for the room left is skipped, not the end of the schema.
- **A double click selects an SQL word**, in the TUI's editor: letters, digits and `_`, so `orders.id` or `count(id)` gives `orders` or `id`, not the whole run between two spaces.
- **A query edited in `$EDITOR` keeps its old text as a version even when the editor exits with an error**, in the TUI: `ctrl-v` lists it as it lists any other version.
- **pgbg's usage counts are in `~/.local/share/pgbg/` beside the history**, for `pgbg usage`: they were in the cache folder, which the system may empty.
  pgbg warns of the old file, so you can move it there and keep the counts so far.
- **pgbg's log is in `~/.cache/pgbg/` on macOS too**: it was `~/Library/Caches/pgbg/`; set `$XDG_CACHE_HOME` to move it.
- **pgbg warns of a file left where it no longer reads it**, as any command starts and on the TUI's status line until the first key: a config, history or saved queries in `~/Library/Application Support/pgbg/`, saved queries in `~/.config/pgbg/queries/`, or usage counts in a cache folder.
  It says where each one goes, as `~/.config/pgbg/config.toml` or `~/.local/share/pgbg/queries/<connection>/`.
- **`⌫` keeps the counts already read**, in the TUI's row detail: going back to a row shows the tables pointing to it with the counts it had, without counting them again.
  No count runs while a followed key's row is on its way, so that row comes sooner.
- **A followed key never shows a row from a connection you changed**, in the TUI's row detail: a row or a count read before the connection's settings changed says it stopped with its session instead.
  Press `⏎` again to read it on the new session.

### Removed

- **A project's `.pgbg/queries/` is no longer read**, in the TUI and `pgbg run --query`: saved queries live only in pgbg's data folder, under `queries/<connection>/`, so a query never lands in a repository.
  Move a project's queries with `mv .pgbg/queries/* ~/.local/share/pgbg/queries/<connection>/`; `queries = "<connection>"` on another connection still shows them there too.

## 0.1.0 - 2026-10-29

### New

- **First release** of pgbg, a TUI and CLI to read PostgreSQL databases; the [README](README.md) covers each feature.
- **Read only by default**, on every connection: each statement runs alone in a read-only transaction, and a connection with `writes = "allowed"` asks before each write; see [Staying safe](README.md#staying-safe).
- **Connections with tiers, tunnels and safe passwords**, in `config.toml`: the tier sets the colour and the timeout, pgbg starts and stops your tunnel, and a password comes from the keychain, a variable or a command; see [Connections](README.md#connections).
- **Saved queries as `.sql` files**, per connection or in a project's `.pgbg/queries/`, with parameters, chart and format headers, and the versions each save replaced; see [Saved queries](README.md#saved-queries).
- **A result grid that reads at a glance**, in the TUI: values coloured by kind, keys in the header, filter, sort, column figures, and rows followed through their foreign keys; see [In the TUI](README.md#in-the-tui).
- **Completion and AI**, in the TUI's editor: names of the schema, joins on foreign keys, and SQL from a command of your choice; see [Asking AI](README.md#asking-ai).
- **The command line**, as `pgbg run`, `history`, `schema`, `doctor` and `update`, with `--json`; see [On the command line](README.md#on-the-command-line).
