# What's new in pgbg

Every change you can see or use, newest first.

Each entry says what changed, then where, then how to use it.

## Unreleased

### Fixed

- **Long lines wrap**, in the TUI's editor: a line wider than the pane goes on to the next row, between words where it can, and the saved text keeps it as one line.
  `↑` `↓` and the mouse move by the rows drawn, so a query on one line can be walked; `^a` `^e` still go to the ends of the whole line.

## 0.3.1 - 2026-11-03

### Fixed

- **One-line fields move and delete as the editor does**, in the TUI's `ctrl-g` instruction, save-as path, row filter and schema filter.
  `⌥←` `⌥→` and `⌥b` `⌥f` move by word, `Home` `End` `^a` `^e` go to the ends, and `Delete` `^w` `^u` `^k` `⌥⌫` delete; `⌥b` and `⌥f` no longer type a letter, and other option characters still do.
- **`ctrl-g` waits for the schema**, in the TUI: an instruction sent before the schema is read shows `reading schema…` and goes by itself once it arrives, and `esc` drops it.
  The read starts as soon as the session opens, a failed read says why on the status line instead of asking without tables, and `schema = "off"` still asks at once.
- **`ctrl-g` no longer answers `-- cannot:` for a table it can reach**, in the AI's schema slice: every table comes in detail when the schema fits in 64 KiB.
  Past that, it follows foreign keys two steps, what a table points to first, so a table many others point to no longer crowds out the one you need.
  The AI completion keeps its 16 KiB, and details a schema that fits in it too.

## 0.3.0 - 2026-11-03

### New

- **Typed commands**, in the TUI's command palette: `:set theme=nord`, `:go shop`, `:open orders/daily.sql`, `:run`, `:save`, `:copy`, `:help` and `:quit`, with the words revu and slack use.
  Typing still filters the list, and a line that reads as a command shows on top of it, so `⏎` runs it and `↓` picks a row below; `⇥` completes, `→` takes the faded rest.
  `↑` on an empty line, with the top row highlighted, brings back the last command you ran, and the arrows walk them until you type.
- **`^k` finds a saved query**, in the TUI on the result, the sidebar, the schema and a plan, as `⌘k` does everywhere and as it finds in revu and slack.
  In the editor and the fields it still deletes to the end of the line.
- **Ask for a fix**, in the TUI after a server's error: with `errors = "code"` in `[ai]`, the footer offers `^g ⏎`, and a blank instruction asks to fix the statement that failed.
  Only the SQLSTATE and the character position go along, never the server's message; off by default.
- **Ask for a chart**, in the TUI's `ctrl-g`: say `orders per week as a line`, take the answer with `⇥`, run it with `⏎`, and the result opens as that chart.
  The answer leads with a `-- chart:` line right above its statement, so each statement of a file can have its own, and the comments above the statement stay.
- **`ctrl-g` on the result pane**, in the TUI: asks about the statement that gave the result, for `chart this by week`, with its columns' names and types, never a row or a value.
- **Plans as a tree**: in the TUI editor, `^b` or `⌥e` shows the plan of the read under the cursor as a tree in the result pane, without running it, and `^f` or `⌥a` shows it as it ran, with `ANALYZE` and `BUFFERS`.
  Your own `EXPLAIN (FORMAT JSON …)` opens the same way: `⏎` folds a node, `v` shows the JSON, the costliest node is lit, and `est off ×80` flags an estimate far from the rows counted.
  `^f` runs the statement read only and rolled back under the connection's timeout, whatever the connection allows, and refuses a write.
- **Past statements as you type**: in the TUI editor, the read you ran most on this connection that starts with what you typed shows faded after the cursor, from three characters on; `→` or `tab` takes it.
  A table or column name shows first and a past read beats a keyword; only reads that succeeded count, never a write, and a match saves an AI completion ask; nothing leaves your machine and there is nothing to set.
- **Save as**: in the TUI, `^d` or `⌥s` saves the editor's text under a new path and leaves the open query as it is, with nothing to set in your terminal and on any keyboard layout.
  The prompt starts from the open query's path, or empty with none open; once saved, the editor stands for the new query, and a path that holds other text is refused.
- **A separate command for the AI completion**: `[ai] ghost_command` in `config.toml` names the program the idle ghost asks, as a quicker model, given as `command` is.
  `ctrl-g` keeps `command`, `ghost = true` still turns the ghost on, and with no `ghost_command` the ghost asks `command` as before.
- **The AI sees more of your schema**, in the prompt `[ai] command` gets: the labels of enum columns, and the name of every table beyond those it shows in detail, so it can reach a table your words did not name.
  Nothing to set; a label holding a secret is left out, as a comment holding one is.
- **The AI sees the tables you use first**, in the prompt `[ai] command` gets: among tables your words point to equally, those you run most on that connection come first, so they survive the 16 KiB cap.
  Nothing to set; your history only changes the order and is never sent.
- **pgbg reconnects by itself when the link is lost**, in the TUI: a read that fails on the link before any row shows goes once more on a new session, and the status line says `reconnecting…` until the result shows.
  A write, a read inside your own `BEGIN` and a result already on screen never go again, `esc` stops the reconnect, and the history keeps one entry per sending.
  A read that calls a function outside a short safe list, or that follows a statement that may have changed the session, as a `SET`, says the link was lost and waits for your `⏎`.
- **The editor underlines where the server stopped**, in the TUI, when the server rejects a statement and says where: the failing token is underlined in the theme's `danger` colour, a `{{name}}` whole, and scrolled into view while the cursor stays where it was.
  It works out where the text sent falls in the editor, so comments, `{{name}}` parameters, other statements and multi-byte text never shift it.
  The underline goes when the text changes or the next run starts; an error without a position shows as before, with none.
- **Command palette**: `^l` anywhere in the TUI, or `:` on the result and in the sidebar, lists every command with its keys, filtered as you type.
  What runs from the pane you are on comes first, and `⏎` runs it as its key does; the others name the pane they work in.
- **Pick a theme live**: type `theme` in the command palette and move over a name to see it on the whole TUI.
  `esc` brings the old one back, and `⏎` keeps it and writes `[tui] theme` in `config.toml`, comments untouched.
- **Stacked charts, and keys to reach every kind**, in the TUI's chart view and the `-- chart:` header: `-- chart: stacked x=day y=orders,refunds`, with `horizontal` to lay it on its side.
  On a chart, `t` steps through grouped, stacked and line, `r` turns the bars, and `v` comes back to the kind and orientation you left.
  The footer names what each key leads to, as `t stacked` and `r upright`, with no `r` on a line, and the pane's title names what shows now.
- **Grouped charts, upright or on their side**, in the `-- chart:` header: `-- chart: grouped x=day y=orders,refunds`, or with `horizontal` at the end.
  `bar` stays valid and means `grouped horizontal`.
- **A bar chart says what did not fit**, in the TUI's chart view: a pane too small says `+3 more` for the points it leaves out.
- **A legend names each series in its colour**, on a chart of several `y=` columns: it goes on to more rows in a narrow pane, and the colours keep neighbours apart on every `[tui] theme`, starting over past the sixth.
- **Frozen columns**: in the TUI's result grid, the primary key's columns a result starts with, or else its first column, stay on the left while the grid scrolls right, in the order the statement asked for.
  `z` switches that off and on, `Z` freezes the columns up to the cursor's; on a narrow terminal the frozen columns give way past half the width.
- **Hidden columns**: in the TUI's result grid, `x` hides the column under the cursor and `X` shows them all again; the title counts those hidden.
  Copies, `/`, the row detail and charts still hold every column, and what you hid or froze stays when a statement returns the same columns.

### Fixed

- **The AI no longer guesses a join**, in `ctrl-g`: it joins only along a listed foreign key and uses only listed columns, and when what it needs is not listed, the status line shows its reason in place of a statement.
- **`F2` copies the statement under the cursor**, in the TUI editor, on any keyboard layout, where `⌥c` needs the terminal to send option as meta.
- **Common words no longer pull tables into the AI prompt**: `for` does not bring a table `forms`, and a column such as `name` or `created_at` does not bring every table holding one.
- **A rewrite waits for `ctrl-g`**, in the TUI with `[ai] ghost = true`: when the answer rewrites what you typed instead of continuing it, the footer reads `^g rewrite waiting`.
  `ctrl-g` then shows it as a diff, with no new ask, and `tab` takes it; any other key drops it.
- **An apostrophe inside a word no longer loses the comment**, in the AI completion with literals masked: `-- what's the total` is now the instruction, and a quoted value after it is still masked.
- **Every quoted value stays out of an AI prompt with literals masked**: in a comment, `"..."`, backticks and curly quotes are masked too, and a comment whose quotes pair wrongly is left out; in a statement, a double-quoted name the schema does not hold is masked, and pgbg puts it back in the answer.
- **A comment left out of the AI completion says why**, on the status line until the next key, and not only in the log.
- **A secret in a comment stays out of the AI completion**: a comment naming `PGPASSWORD`, `pwd`, `passwd`, `token`, `api_key` or `secret`, or holding any `KEY=value`, is left out of the ask, and the status line says so.
- **A tunnel that still runs but no longer forwards is started anew**, on every connection with a tunnel: a run after a lost link no longer fails over and over until the connection is closed.
- **The history gives the real time of a run whose session did not open**, in `ctrl-r`: it said `0 ms` whatever the wait.
- **`v` comes back to the chart of the `-- chart:` header**, in the TUI: it went to the chart pgbg picks by itself.
- **A negative value draws its bar**, in the TUI's bar charts: it runs the other way from zero, where it drew none.
- **A point with a gap on each side shows**, on a line chart: a value between two NULLs drew nothing.

## 0.2.0 - 2026-10-30

### New

- **Themes**: `[tui] theme = "dracula"` in `config.toml` paints the whole TUI in that palette: frames, grid, SQL and JSON colours, charts, help and footer.
  The names are those of the other vtui TUIs (`default`, `dracula`, `catppuccin`, `catppuccin-latte`, `rosepine`, `rosepine-dawn`, `nord`, `tokyonight`, `monokai`); an unknown one warns and falls back to `default`, and tier frames keep their colours on each.
- **The question before a write says how many rows it would touch**, in the TUI, for an `UPDATE` or a `DELETE` on a connection that allows writes: the footer reads `DELETE from orders? 42 rows` while it waits for `y`.
  The count is a read of the same `WHERE`, run read only and rolled back within 2 s, so the write itself never runs before `y`; past 10 000 rows it says `more than 10 000 rows`.
- **Browse a table without SQL**: in the schema tab of the sidebar and in the `ctrl-t` table finder, `⏎` or a click on a table writes `SELECT * FROM` it `LIMIT 100` in the editor and runs it, so you can refine it there.
  Browsing the next table replaces that statement, unless you edited the editor since, and the result pages as any other.
  Its columns and indexes moved to `d` in the sidebar and `ctrl-d` in the finder.
- **JSON tree**: on a `json` or `jsonb` cell of the grid, `o` opens its value as a tree, coloured as the grid colours JSON.
  `⏎` folds or unfolds an object or an array, or the one holding the value under the cursor, `y` copies the value under the cursor and `Y` its path, as `.items[0].sku`.
- **`pgbg run` says how many rows a write would touch** in its `y/N` question, for an `UPDATE` or a `DELETE`: `DELETE from orders? 42 rows [y/N]`.
  `--yes` sends the write at once, without counting.
- **`interval` columns read, sort and count**, in the TUI's result grid and copies: on the right, as PostgreSQL prints them; `f` or `F` reads them as `1y 2mo 3d`, `s` and `i` compare a month as 30 days, and `=` or `!` bind them.
- **`i` shows its values in the column's format**, in the TUI's result grid: a grouped number or a duration reads as it does in the grid.

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
