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

Connections live in `config.toml`, in the `pgbg/` folder of your config folder:

| System | Folder |
| --- | --- |
| macOS | `~/Library/Application Support/pgbg/` |
| Linux | `~/.config/pgbg/` |

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
4. **The role.** Only a read-only database role truly stops a write: use one for production.

On a connection that allows writes, each write asks first; `BEGIN`, `COMMIT`, `ROLLBACK`, `SAVEPOINT` and cursor statements do not, but `COMMIT PREPARED` and `ROLLBACK PREPARED` do.
The TUI's footer names it, such as `DELETE from orders?`: `y` sends it, any other key drops it.
A write hidden in a read, as a function that writes or `setval`, runs nothing at first, then asks `this statement tried to write: run it?`.
`pgbg run` asks `y/N` on the terminal; with no terminal, it refuses the write unless you pass `--yes`.
A read-only transaction does not stop `dblink` or `pg_terminate_backend`: those run without asking, and only the role stops them.

`pgbg doctor prod` checks a connection, one line per check, with how to fix what fails:

- the config, TLS, the tunnel and the login;
- the statement timeout and where it comes from;
- what the role can write, and what it can do that a read-only transaction does not stop.

It exits 1 when a check fails, so a script can run it.

## Saved queries

Put `.sql` files under `.pgbg/queries/` in your project, in folders if you like.
pgbg finds the nearest one from the folder you start it in, the way git finds `.git`.
Queries of your own, for every project, go under `pgbg/queries/` in your config folder.

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
| `-- format:` | How columns open: `raw`, `local`, `relative`, `grouped`, `bytes` or `short`. |

`{{name}}` in a statement is a parameter.
Its value is always bound, never pasted into the SQL text.
In the TUI, a bar above the editor shows one field per parameter.

`[display] format = "readable"` in `config.toml` opens every result readable; the default is `raw`.

## In the TUI

| Key | Does |
| --- | --- |
| `⏎` | Run the statement under the cursor. |
| `⌥⏎` `⇧⏎` | New line. |
| `esc` | Cancel the running statement. |
| `tab` | Next pane: editor, parameters, result, sidebar. |
| `^o` | Pick another connection; sessions stay open. |
| `^p` | Find a saved query. |
| `^t` | Find a table of the schema. |
| `^r` | Search the history of this connection. |
| `[` `]` | In the sidebar: switch between queries and schema. |
| `→` | Take the faded rest after the cursor: completion, or the idle ghost. |
| `^space` | List the names that fit at the cursor. |
| `v` | Show the result as a chart, or as a grid again. |
| `f` `F` | Next format for this column; every column raw or readable. |
| `y` `Y` `c` | Copy the cell; the row as CSV; the result as CSV, JSON or Markdown. |
| `^s` | Save the query. |
| `^e` | Edit the saved query in `$EDITOR`. |
| `F1` | Every key of the pane you are in. |
| `^c` | Quit. |

`pgbg tui <connection>` opens the TUI on another connection than the first.

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

`--param name=value` and `--params-file <file>` (one `name=value` per line, `-` for stdin) give parameter values.
`--yes` sends a write without asking, on a connection with `writes = "allowed"` such as `scratch` above.
`run`, `history`, `schema`, `doctor` and `connections` take `--json` for machine-readable output.

The history lives in `pgbg/history/` in your data folder, or in `$PGBG_DATA_DIR`.
It keeps when, how long and what came of each statement, never a row.

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
