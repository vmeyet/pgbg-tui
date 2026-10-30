# What's new in pgbg

Every change you can see or use, newest first.

Each entry says what changed, then where, then how to use it.

## Unreleased

### Fixed

- **`s` and `i` refuse a column pgbg cannot decode**, in the TUI's result grid: the status line says so, where `i` counted all its values as one.
- **`NUMERIC` values sort and count by their exact digits**, in the TUI's result grid with `s` and `i`: two values past a float's precision no longer tie.
- **`=` and `!` refuse a cell pgbg could not decode**, in the TUI's result grid: the status line says so, where the parameter bar got `<unsupported>`.
- **`=`, `!` and `i` do nothing while the filter hides every row**, in the TUI's result grid: they acted on a row the grid did not show.
- **A long schema comment no longer crowds out other tables**, in the prompt `ctrl-g` sends: each comment is cut to 300 bytes, and a table too long for the room left is skipped, not the end of the schema.
- **A double click selects an SQL word**, in the TUI's editor: letters, digits and `_`, so `orders.id` or `count(id)` gives `orders` or `id`, not the whole run between two spaces.
- **A query edited in `$EDITOR` keeps its old text as a version even when the editor exits with an error**, in the TUI: `ctrl-v` lists it as it lists any other version.

## 0.1.0 - 2026-10-29

### New

- **First release** of pgbg, a TUI and CLI to read PostgreSQL databases; the [README](README.md) covers each feature.
- **Read only by default**, on every connection: each statement runs alone in a read-only transaction, and a connection with `writes = "allowed"` asks before each write; see [Staying safe](README.md#staying-safe).
- **Connections with tiers, tunnels and safe passwords**, in `config.toml`: the tier sets the colour and the timeout, pgbg starts and stops your tunnel, and a password comes from the keychain, a variable or a command; see [Connections](README.md#connections).
- **Saved queries as `.sql` files**, per connection or in a project's `.pgbg/queries/`, with parameters, chart and format headers, and the versions each save replaced; see [Saved queries](README.md#saved-queries).
- **A result grid that reads at a glance**, in the TUI: values coloured by kind, keys in the header, filter, sort, column figures, and rows followed through their foreign keys; see [In the TUI](README.md#in-the-tui).
- **Completion and AI**, in the TUI's editor: names of the schema, joins on foreign keys, and SQL from a command of your choice; see [Asking AI](README.md#asking-ai).
- **The command line**, as `pgbg run`, `history`, `schema`, `doctor` and `update`, with `--json`; see [On the command line](README.md#on-the-command-line).
