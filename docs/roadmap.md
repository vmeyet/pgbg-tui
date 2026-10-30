# Roadmap

Each step is one MR under 500 lines, in order.
The MR that ships a step ticks its box.

## 0. Ground: done

## 1. Read from the command line: done

## 2. The TUI shell: done

## 3. Syntax colours: done

## 4. Connections: done

## 5. Tunnels: done

## 6. Saved queries: done

## 7. Big results: done

## 8. Schema: done

## 9. Reading a row: done

## 10. Charts: done

## 11. Doctor: done

## 12. Completion: done

## 13. Parameters: done

## 14. Formats: done

## 15. Craft: done

## 16. Confirm a write: done

## 17. Asking a command for SQL: done

## 18. Engineering pass: done

## 19. First real use: done

## 20. Review of the first-use batch

The review of #194 to #208 found these, most severe first; real use of the AI ask found the last ones.

- [x] The idle ask sends a comment as its instruction only with `[ai] literals = "sent"`, and masks it otherwise
- [x] A version whose text holds a secret is kept as a note, as the history keeps it
- [x] The user's own queries folders are made `0700`, as the versions folder is
- [x] The usage file is merged under the same lock the history takes
- [x] Editing in `$EDITOR` keeps the replaced text whatever the editor's exit status
- [x] Undo copies the editor's text only when a key changes it, and its doc states the 1 MiB worst case
- [x] The usage counts live in pgbg's data folder, and the cache folder follows XDG as the others do
- [x] pgbg warns once when a config or history sits in the old macOS folder, and says where to move it
- [x] One private-folder helper and one atomic-write helper serve the history, the log, usage, versions and saved queries
- [x] Showing another sheet re-lists its queries in one place, and the idle ask builds its question once
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
- [x] `e` in that detail writes the join into the editor
- [x] `/` filters the rows on screen and `s` sorts them by the column under the cursor, without a new query
- [x] `=` and `!` on a cell add `WHERE column = value` or `<>` to the statement
- [x] `i` on a column shows its distinct count, NULLs, min, max and five most common values
- [x] The row's detail scans as two columns: names on the left, a primary key's bold, values beside them in the grid's format and colours, a faint rule before the tables pointing to it
- [x] The grid keeps the table a result reads, so a followed key does not lex the statement again on every frame
- [x] Counts pause while a key is followed, and going back keeps the counts already read
- [x] `keys` finds a table once and gives its keys, so a lookup does not parse its own SQL again
- [x] One builder writes `column = $n AND …` for a lookup and a referrer, and the footer hints share one fitting
- [x] A lookup's answer reaches the sheet as an `Answer`, not app-wide

## 22. Review of the pleasure batch

The review of #214 to #231 found these, most severe first.

- [x] A result column's key comes from the table and column the server says it was read from, so an alias or a computed column is never taken for a key
- [x] Referrer counts run only on a key covered by an index, or on demand, each with a short timeout of its own, and never hold the user's next run behind them
- [x] The editor keeps its scroll as state, so a click or a drag never moves the view and a drag selects the lines under the mouse
- [x] Sort and figures refuse a column pgbg cannot decode and say so, instead of counting its values as one
- [x] NUMERIC compares by its decimal text, never through a float
- [x] `=` and `!` bind a cell through `Value::as_param`, and refuse a value pgbg could not decode, as a lookup does
- [x] Every act on the cursor's cell (`=`, `!`, `i`) is refused while the filter hides its row
- [x] A schema comment sent to the AI is cut to a few hundred bytes, and a line that does not fit is skipped, not the end of the slice
- [x] A failed referrer count keeps its failure, shown in the detail or logged
- [x] A double click selects an SQL word (letters, digits, `_`), not everything between two spaces

## 23. Preview a write

- [x] The question before a write says how many rows an `UPDATE` or `DELETE` would touch, counted by a read of its `WHERE`, in the TUI
- [ ] `pgbg run` says the same count in its `y/N` question

## Later

- `update` and the brew check move to vtui, shared by revu, slack and pgbg
- More chart kinds: a new `ChartKind` variant and one file in `chart/` and `tui/chart_view/`
