# pgbg

Read your PostgreSQL databases from the terminal: local, staging, and production behind a tunnel.

- **Read only by default.** A connection takes writes only when you say so, and then each write asks first, even one inside a function call.
- **Tunnels built in.** pgbg starts your `ssh` (or any other) tunnel, and stops it when you quit.
- **Saved queries are plain `.sql` files.** Keep them in your project, next to the code, and review them like code.
- **Charts in the terminal.** Press `v` on a result to see it as bars or a line.
- **Safe on production.** Its own frame colour, a short statement timeout, and `pgbg doctor` to check the role you log in with.

```text
╭ ● shop · local ──────────────────────────────────────────────────────────────╮
│╭ [Queries] Schema ─────────╮╭ Editor · orders/daily.sql ─ READ ONLY ────────╮│
││  ▾ orders/                ││ SELECT day, count(*) AS orders FROM orders …  ││
││    ● daily.sql            │╰───────────────────────────────────────────────╯│
││      weekly.sql           │╭ Result ─ 3 rows · 38 ms ─ [grid] chart ───────╮│
││    customers.sql          ││     day         orders                        ││
││                           ││ ▶   2024-01-01      12                        ││
││                           ││     2024-01-02      31                        ││
││                           ││     2024-01-03       7                        ││
│╰───────────────────────────╯╰───────────────────────────────────────────────╯│
│ ⏎ run  ^p find  ⇥ pane  F1 keys  ^s save  ^c quit       ⇄ :49213 · connected │
╰──────────────────────────────────────────────────────────────────────────────╯
```

## Install

You need Rust 1.92 or newer.

```sh
cargo install --git https://github.com/vmeyet/pgbg-tui
```

Or from a clone:

```sh
git clone https://github.com/vmeyet/pgbg-tui
cd pgbg-tui
cargo install --path .
```

## Quick start

```sh
pgbg connection add local --tier local --url postgres://nina@localhost:5432/shop
pgbg
```

The first command writes the connection to your config file.
The second opens the TUI on your first connection.
Type a statement, press `⏎` to run it, and `F1` to see every key.

## Connections

Connections live in `config.toml`, in pgbg's config folder: `$XDG_CONFIG_HOME/pgbg/` when it is set, else `~/.config/pgbg/`, on macOS too.

Set `$PGBG_CONFIG` to use another file.
`pgbg connection add|edit|remove` and `pgbg password set` edit this file for you, and keep your comments.

```toml
[connections.local]
tier = "local"
url = "postgres://nina@localhost:5432/shop"

[connections.prod]
tier = "production"
url = "postgres://nina@db.example.com/shop?sslmode=require"
password = { keychain = "pgbg/prod" }
tunnel = { command = ["ssh", "-N", "-o", "ControlPath=none", "-o", "ExitOnForwardFailure=yes", "-L", "127.0.0.1:{port}:db.example.com:5432", "bastion.example.com"] }
```

### Keys of a connection

| Key | What it does |
| --- | --- |
| `tier` | `local`, `staging` or `production`: sets the colour and the default timeout. |
| `url` | `postgres://user@host:port/database`, never with a password. |
| `password` | Where the password comes from (see below). |
| `tunnel` | A command that forwards a local port, and an optional `before` command. |
| `writes` | `"allowed"` lets writes through; read only when missing. |
| `tls` | `"off"` talks in clear on purpose; `"on"` asks for TLS whatever `url` says. |
| `statement_timeout` | Such as `"2min"`; `"0s"` for no limit. |
| `schema` | `"off"` skips the second, short login that reads the schema for the sidebar and completion: for a pooler, or a role with no login to spare. |
| `queries` | Another connection whose saved queries this one uses, as `"prod"` for a replica: its own folder when missing. |

### Tiers

| Tier | Colour | Statement timeout |
| --- | --- | --- |
| `local` | green | none |
| `staging` | amber | 1 minute |
| `production` | red-magenta | 30 seconds |

`statement_timeout` on a connection replaces its tier's timeout.
A stricter timeout set on the server is kept.

### Passwords

No password goes in `config.toml`, and pgbg refuses one there.
It comes from one of three places:

```toml
password = { keychain = "pgbg/prod" }                     # the macOS keychain
# password = { env = "SHOP_PGPASSWORD" }                  # an environment variable
# password = { command = ["pass", "show", "shop/prod"] }  # a command that prints it
```

`pgbg password set prod` saves a password in the keychain.
You type it hidden or pipe it in: no flag takes a password, since `ps` would show it.

### TLS

A host that is not this machine needs `sslmode=require` (or stronger) in its `url`.
pgbg checks the server's certificate.
To talk in clear on purpose, set `tls = "off"` on the connection.

### Tunnels

`tunnel.command` is any command that forwards a local port.
pgbg picks a free port, puts it in place of `{port}`, waits for it to answer, and connects to `127.0.0.1` on it.
The tunnel runs in its own process group and dies with pgbg.

For ssh, keep both options of the example:

- `ControlPath=none`, so the forward dies with pgbg instead of living on in a shared ssh connection.
- `ExitOnForwardFailure=yes`, so ssh stops when it cannot take the port.

A tunnelled connection needs TLS too, even when its `url` says `localhost`: another program could answer on that port.
An optional `before` command, such as one that wakes the bastion, runs once before each tunnel starts:

```toml
tunnel = { before = ["./wake-bastion.sh"], command = ["ssh", "..."] }
```

It may run more than once, so it must be safe to run twice.

## Staying safe

Four layers stand between you and a write, outermost first:

1. **The connection.** It is read only unless it says `writes = "allowed"`, whatever its tier.
2. **The guard.** pgbg reads each statement before it sends it, and refuses anything that is not a read.
3. **The transaction.** On a read-only connection, each statement runs alone in `BEGIN READ ONLY`, always rolled back, so the server refuses a write the guard missed.
   On a connection that allows writes, a statement the guard reads as a read runs with writes refused too, and a write the server finds in it, as in `SELECT archive_orders()`, asks before it runs again.
   Inside a transaction you began with `BEGIN`, it does not ask: your `COMMIT` or `ROLLBACK` decides what stays.
4. **The role.** Only a read-only database role truly stops a write: use one for production.

On a connection that allows writes, each write asks first; `BEGIN`, `COMMIT`, `ROLLBACK`, `SAVEPOINT` and cursor statements do not, but `COMMIT PREPARED` and `ROLLBACK PREPARED` do.
The TUI's footer names it, such as `DELETE from orders?`: `y` sends it, any other key drops it.
A write hidden in a read, as a function that writes or `setval`, keeps nothing at first, then asks `this statement tried to write, and may run twice: run it?`.
What a read-only transaction does not stop, as `dblink_exec` or `pg_advisory_lock`, has already run once, so a `y` runs it a second time.
`pgbg run` asks `y/N` on the terminal; with no terminal, it refuses the write unless you pass `--yes`.
A read-only transaction does not stop `dblink` or `pg_terminate_backend`: those run without asking, and only the role stops them.

`pgbg doctor prod` checks a connection, one line per check, with how to fix what fails:

- the config, TLS, the tunnel and the login;
- the statement timeout and where it comes from;
- what the role can write, and what it can do that a read-only transaction does not stop.

It exits 1 when a check fails, so a script can run it.

## Saved queries

Put `.sql` files under `.pgbg/queries/` in your project, in folders if you like.
pgbg finds the nearest one from the folder you start it in, the way git finds `.git`, and shows it on every connection.
Queries of your own belong to a connection: they live in `queries/<connection>/` in pgbg's data folder, as `~/.local/share/pgbg/queries/prod/`.
The sidebar and `ctrl-p` show the project's queries and those of the connection on screen; `queries = "prod"` on another connection shows the ones of `prod` there too.
`ctrl-s` on a new query saves it in the project's `.pgbg/queries/` when there is one, else in the folder of the connection on screen; pgbg never makes a project's folder itself.
Queries kept in `~/.config/pgbg/queries/` by an earlier pgbg move with `mv ~/.config/pgbg/queries ~/.local/share/pgbg/queries/prod`, naming the connection they belong to.
Each save over a query, from `ctrl-s` or `$EDITOR`, keeps the text it replaced under `versions/` in pgbg's data folder, never in your project: the newest 50 of each query.
`ctrl-v` lists them, and `⏎` puts one back in the editor, unsaved until you press `ctrl-s`.
A text holding a password is kept as a note instead, as the history does: the list shows the note, and it cannot be put back.

```sql
-- name: Orders per day
-- connection: prod
-- chart: bar x=day y=orders
-- param: since date = 2024-01-01
-- format: day=relative
SELECT date_trunc('day', created_at) AS day, count(*) AS orders
FROM orders
WHERE created_at > {{since}}
GROUP BY 1
ORDER BY 1;
```

| Header | What it does |
| --- | --- |
| `-- name:` | A readable name for the query. |
| `-- connection:` | The connection it was written for; the query still runs on the one on screen. |
| `-- chart:` | `bar` or `line`, with optional `x=` and `y=` columns (`y=orders,refunds`). |
| `-- param:` | A parameter: `name type = default`, or `name type optional` for NULL when empty. |
| `-- format:` | How columns open: `raw`, `local`, `relative`, `grouped`, `bytes`, `short` or `check`. |

`{{name}}` in a statement is a parameter.
Its value is always bound, never pasted into the SQL text.
In the TUI, a bar above the editor shows one field per parameter.

`[display] format = "readable"` in `config.toml` opens every result readable; the default is `raw`.

## In the TUI

| Key | Does |
| --- | --- |
| `⏎` | Run the statement under the cursor. |
| `⌥⏎` `⇧⏎` | New line. |
| `esc` | Drop the faded rest after the cursor while one shows; else cancel the running statement. |
| `tab` `⇧tab` | Next, previous pane: editor, parameters, result, sidebar; in the editor, `tab` takes the faded rest while one shows. |
| `⇧←` `⇧→` `⇧↑` `⇧↓` | The pane on that side; a click on a pane does the same. |
| `^o` | Pick another connection; sessions stay open. |
| `^p` `⌘k` | Find a saved query; `⌘k` needs a terminal with the kitty keyboard protocol, as Ghostty, Kitty or WezTerm. |
| `^t` | Find a table of the schema. |
| `^r` | Search the history of this connection. |
| `←` `→` `[` `]` | In the sidebar: switch between queries and schema. |
| `→` `tab` | Take the faded rest after the cursor: completion, or the idle ghost; after `JOIN`, a table with its alias and `ON` condition. |
| `^z` | Undo: the last word typed, paste, name taken or delete. |
| `⇧^z` `^y` | Redo; `^y` where the terminal sends `⇧^z` as `^z`. |
| `^space` `^n` | List the names that fit at the cursor, after `JOIN` the tables a foreign key links first; `^n` where macOS takes `^space` to switch the input source. |
| `v` | Show the result as a chart, or as a grid again. |
| `f` `F` | Next format for this column; every column raw or readable. |
| `y` `Y` `c` | Copy the cell; the row as CSV; the result as CSV, JSON or Markdown. |
| `^s` | Save the query. |
| `^q` | A new, empty query; the first `^s` asks for its path. |
| `^e` | Edit the saved query in `$EDITOR`. |
| `^v` | The versions of the saved query its saves replaced; `⏎` puts one back in the editor, unsaved. |
| `F1` `?` | Every key of the pane you are in; `?` outside the editor and the parameters, where it types. |
| `^c` | Quit. |

`pgbg tui <connection>` opens the TUI on another connection than the first.

### Reading a result

The grid colours each value by its kind, so a column reads at a glance.

| Value | Looks like |
| --- | --- |
| `NULL` | Dimmed and slanted, never like the text `NULL`. |
| A boolean | `true` green, `false` dimmed; `f` turns them into `✓` and `✗`. |
| A number | On the right; a negative one red. |
| A text column of a few values that repeat, as a status | One colour per value, the same in every result: eight values at most among the first 500 rows, each found twice on average. |
| A UUID | Its first eight digits plain, the rest dimmed. |
| A timestamp | Its date plain, its time dimmed. |
| JSON | Keys, strings and numbers each in their colour. |

When a statement reads one table, the header names its primary key in bold and ends a column with a foreign key with `→`.

## Asking AI

`ctrl-g` in the editor asks a command of your choice for SQL, and the idle ghost asks it for the rest of the statement you are typing.
Name the command in the `[ai]` table of `config.toml`:

```toml
[ai]
command = ["claude", "-p", "--model", "haiku"]  # reads the prompt on stdin, prints SQL; never run through a shell
env = ["USER"]                                  # claude finds its login through USER
ghost = true                                    # the idle ghost; off by default
literals = "masked"                             # or "sent"; masked by default
timeout = 60                                    # seconds; 60 by default
```

| Key | What it does |
| --- | --- |
| `command` | The program and its arguments; it runs from an empty folder. |
| `env` | The variables it gets besides `PATH`, `HOME`, `LANG` and `TERM`; one a password names is never passed. |
| `ghost` | Asks for the rest of the statement after 1 s without a key, when no completion shows, and shows all of it faded; `→` takes it. |
| `literals` | `masked` replaces the statement's values with markers, `sent` sends them as typed. |
| `timeout` | How long the command has before it is stopped. |

`ai = "off"` on a connection keeps both from asking there.

The statement under the cursor goes along with the tables it needs: those it or your words name, then those one foreign key away, each with its columns, types, foreign keys and comments, within 16 KiB.
A comment holding a password is left out.
When nothing names a table, as in `SELECT * FROM `, the names of the tables go alone.

The `--` lines right above the statement tell the idle ghost what you want; header lines such as `-- name:` are left out.
Their values are masked as the statement's are: strings, numbers and any word holding an `@`, such as an email.
A comment that opens a quote it never closes, as `don't`, cannot be masked: the idle ghost then only asks to complete the statement, and `pgbg.log` says so.
With `literals = "sent"`, the comment is sent as typed; one holding a password is never sent.
Press `⏎` at the end of `-- the ten biggest orders`, with nothing typed under it, and it asks at once for the whole statement.
While the command runs, the footer says `asking claude…`, named after your program.

When the idle ghost shows nothing, `pgbg.log` in pgbg's cache folder says why.

## On the command line

```sh
pgbg run local "SELECT id, total FROM orders LIMIT 5"
pgbg run local "SELECT * FROM orders WHERE created_at > {{since}}::date" --param since=2024-01-01
pgbg run local --query orders/daily --params-file params.txt
pgbg run local --file report.sql --json
pgbg run scratch "DELETE FROM orders WHERE id = 7" --yes
```

| Command | Does |
| --- | --- |
| `pgbg run <connection>` | Run one statement: as text, `--query <path>` or `--file <file>` (`-` for stdin). |
| `pgbg history <connection>` | The statements run, newest first; `--limit n`. |
| `pgbg schema <connection> [filter]` | The tables the role can read, with columns and indexes. |
| `pgbg doctor <connection>` | Check a connection. |
| `pgbg connections` | List the connections. |
| `pgbg connection add\|edit\|remove` | Change `config.toml`. |
| `pgbg password set <connection>` | Save a password in the keychain. |
| `pgbg usage` | What you use the TUI for: the keys never or rarely used, the most used, the time per pane; `--since 4w`, `7d` or `all`. |

`--param name=value` and `--params-file <file>` (one `name=value` per line, `-` for stdin) give parameter values.
`--yes` sends a write without asking, on a connection with `writes = "allowed"` such as `scratch` above.
`run`, `history`, `schema`, `doctor`, `connections` and `usage` take `--json` for machine-readable output.

The history lives in `$XDG_DATA_HOME/pgbg/history/` when it is set, else `~/.local/share/pgbg/history/`, on macOS too; set `$PGBG_DATA_DIR` to keep it elsewhere.
The versions of saved queries live beside it, in `versions/`.
It keeps when, how long and what came of each statement, never a row.

### Usage counts

The TUI can count which keys you use and how long you spend on each pane, so `pgbg usage` shows what to learn and what to drop.
It is off until you ask for it:

```toml
[usage]
enabled = true
```

The counts stay on this machine, in `usage.jsonl` in pgbg's cache folder (`~/Library/Caches/pgbg` on macOS, `~/.cache/pgbg` on Linux), and are never sent anywhere.
They hold act names and seconds per pane only: no statement, table, row, connection or host.
A key counts as what it did where you pressed it; a pane stops counting time a minute after your last key, and while the terminal is in the background.
Delete the file to start again.

## What's next

[`docs/roadmap.md`](docs/roadmap.md) lists the next steps, and [`CHANGELOG.md`](CHANGELOG.md) what changed.

## Contributing

Read [`AGENTS.md`](AGENTS.md) for the rules, and [`ARCHITECTURE.md`](ARCHITECTURE.md) for how the code fits together.

```sh
cargo test                                # unit, snapshot and CLI tests
cargo test -- --include-ignored           # also tunnels, processes and a real PostgreSQL
cargo insta review                        # after a deliberate change to the screen
```

The ignored tests need a throwaway database in `PGBG_TEST_DATABASE_URL`:

```sh
docker run -d -p 5432:5432 -e POSTGRES_DB=shop -e POSTGRES_HOST_AUTH_METHOD=trust postgres:17
export PGBG_TEST_DATABASE_URL=postgres://postgres@localhost:5432/shop
```

## License

[MIT](LICENSE)
