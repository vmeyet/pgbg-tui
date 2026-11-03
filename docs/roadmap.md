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
- [x] Browse without SQL: `⏎` or a click on a table writes its preview in the editor and runs it, the next table replacing it; its columns move to `d` and `^d`
- [x] `o` on a JSON cell opens it as a tree that folds; `y` copies the value, `Y` the path
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
- [x] `pgbg run` says the same count in its `y/N` question

## 24. After a week of use

- [x] `⌥s` saves the editor's text under a new path, starting from the open query's, and the editor stands for the new query
- [x] `[ai] ghost_command` names the command the idle ghost asks; `ctrl-g` keeps `command`
- [x] The AI's schema slice always names the other tables after the detailed ones, within the cap
- [x] A common word or a generic column name (`for`, `name`, `created_at`) no longer brings a table into the slice
- [x] The slice shows the labels of an enum column, one holding a secret left out
- [x] Within one reason, the slice puts first the tables the connection's history runs most, and only the order changes
- [x] The idle ghost reads an apostrophe inside a word as no quote, says on the status line when it leaves a comment out, and keeps a rewrite for `ctrl-g`
- [x] A lost link reconnects by itself, a tunnel that carries nothing starts anew, and a read goes once more
- [x] The editor underlines the token a server error points at and scrolls it into view, the cursor left where it was
- [x] A command palette on `^l` and `:` lists every act of the action table, and runs those that work in the pane it opened on
- [x] Plain `cargo test` needs no database for the schema tests, and a tunnel that just died is never taken for ready, whatever the timing of its watching thread
- [x] The palette lists every theme: each shows live while highlighted, `esc` restores the old one, and `⏎` writes `[tui] theme`
- [x] Several series on a chart: colours that stay apart on every palette, and a legend that names each in its colour
- [x] Bars draw from one model, so a negative value runs the other way from zero and a small pane says what it left out
- [x] Grouped charts upright or on their side, from the `-- chart:` header
- [x] Stacked charts, and `t` and `r` on a chart, so a result with no header reaches every kind
- [x] The grid keeps the primary key it starts with, or else its first column, on the left while it scrolls, `z` `Z`, and gives way past half its width
- [x] The grid hides columns, `x` `X`, and keeps what was hidden and frozen while a result has the same columns
- [x] `⌥e` and `⌥a` show `EXPLAIN` as a tree that folds, the costliest node lit and an estimate far off flagged; `⌥a` runs a read only, rolled back whatever the connection allows
- [x] The ghost offers the rest of the past read run most on this connection, from three typed characters, after a schema name and before a keyword or an idle ask
- [x] The history ghost ends the keyword you are typing in your case, and leaves the rest as the past statement wrote it
- [x] Snapshots pin negative grouped bars again, upright and on their side
- [x] Ask for a fix after a server's error, with `[ai] errors = "code"`: the SQLSTATE and the position go along, never the server's words
- [x] Ask for a chart: the answer leads with a `-- chart:` line, its statement's own, and `ctrl-g` on the result pane sends the columns' names and types
- [x] The `⌥` keys have a key beside them that needs no terminal setting, on any layout: `^d` save as, `^b` plan, `^f` plan as it ran, `F2` copy the statement

## 25. Review of the week-of-use batch

The review of #260 to #285 found these, most severe first.

- [x] `v` on a table's description after a chart never panics: `left_chart` moves into `Run::Done`, so it cannot outlive its result
- [x] The idle ask sends no secret from a comment: `PGPASSWORD=x`, `pwd x`, `api_key x` and `token x` count as secrets, as any `KEY=value` does
- [x] With literals masked, a double-quoted, backticked or curly-quoted span in a comment is masked, and a quoted name the catalog does not hold is masked or refused in a statement
- [x] The prompt says to join only along a listed foreign key and use only listed columns, and to say so when what it needs is not listed
- [x] A read is not sent again once the session's settings may have changed, as after a confirmed `SET search_path` or `SET ROLE`, or a read calling `set_config`
- [x] A read is sent again on its own only when it calls no function outside a short safe list; any other says the link was lost and `⏎` runs it again
- [ ] A read-only rollback also releases session-level advisory locks, and the docs stop saying that everything is rolled back
- [ ] Taking an answer keeps every comment above the statement, blank-separated and `/* */` ones too, and removes only the `key:` lines the answer sets
- [ ] An idle answer leading with `-- chart:` waits as a rewrite, so taking it never loses the chart
- [ ] A plan's result never overwrites the hidden and frozen columns of the statement before it
- [ ] The timeout covers a login's follow-up round trips, so a tunnel gone silent after login is replaced and stalls no job
- [ ] A save `⌥s` refuses keeps the prompt open with the path typed and the refusal in it
- [x] A comment whose quotes pair wrongly is refused: `5' tall '1 Main St.' x'` and `'' nina ''` sent their value in clear
- [ ] Only the user's words choose the schema slice: pgbg's own instruction text pulled in tables named by `server` or `statement`
- [ ] `:` and `^g` work on a plan, with a test that every pane with no text field has a `:` row
- [ ] The palette says when an act cannot run on what the pane shows, as a grid act on a chart, or `t` and `r` on a grid
- [ ] `esc` stops a reconnect a lookup or a schema read started
- [ ] Cancelling a reconnect says the tunnel stopped, so it never stays shown as starting
- [ ] During an outage, opening again does not restart the tunnel and rerun `before` each time, and the docs say what happens
- [ ] The idle ghost ends a keyword in the case typed, through one function both ghosts call
- [ ] The history ghost leaves out `pg_terminate_backend`, `pg_cancel_backend`, `setval`, `nextval` and any run confirmed as a write
- [ ] `E'Paid'` and `E'paid'` stay two statements in the history index
- [ ] One read of the history serves the index and `TableUse`, so a schema read does not parse it again
- [ ] A comment the answer repeats is not kept twice
- [ ] Schema names go through `printable`, a column named as a secret follows one rule in the slice and the result columns, and a cut falls between enum labels
- [ ] `v` keeps a plan's tree and folds, the title promises a plan only when one parses, and a plan past serde's depth says why it shows as a grid
- [ ] Hiding the sorted column keeps a sign of the sort
- [ ] A stacked sum that is not finite counts as missing, and a very large or small number keeps a short form
- [ ] `set_fold` takes no `bool`, and `backend/config.rs` names each `ConfigAnswer` instead of a catch-all
- [ ] `Failure::Link` holds only link failures, through a typed kind, and `run` and `run_again` share one send helper
- [ ] Character positions and byte offsets get their own newtypes
- [ ] `Sheet` and `act.rs` are split by concern, with `Showing` holding one result's state
- [ ] `rest_after` moves to `ghost.rs`, `match ()` chains become plain `if`, `prompt_about` takes `PromptInput`, and the writing-function test helper moves to `fixtures`
- [ ] CI runs a plain `cargo test` without `PGBG_TEST_DATABASE_URL`, so a test reading it un-ignored fails there
- [ ] The tunnel examples add `ExitOnForwardFailure=yes`, as a live ssh that failed to bind still counts as ready when something else answers on its port

## 26. One palette across the TUIs

pgbg, revu and slack read the same keys and share the plumbing in vtui.

- [x] `⌘k` finds in all three, and `^k` does too in pgbg wherever no text takes it

## Later

- revu and slack run `update` and the brew check through `vtui::update`, as pgbg does
- More chart kinds: a new `ChartKind` variant and one file in `chart/` and `tui/chart_view/`
