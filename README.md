# pgbg

Read your PostgreSQL databases from the terminal: local, staging, and production behind a tunnel.
Read only by default, saved queries in plain `.sql` files, and charts of your results.

The TUI is early: `pgbg` opens it on your first connection, `pgbg tui <connection>` on another, and `ctrl-o` switches between them; [`docs/roadmap.md`](docs/roadmap.md) says what comes next.

## Connections

`config.toml` in `pgbg/` under your config folder (`~/Library/Application Support/` on macOS, `~/.config/` on Linux), or the file `$PGBG_CONFIG` names:

```toml
[connections.local]
tier = "local"
url = "postgres://nina@localhost:5432/shop"

[connections.prod]
tier = "production"
url = "postgres://reader@db.example.com/shop?sslmode=require"
password = { keychain = "pgbg/prod" }
tunnel = { command = ["ssh", "-N", "-o", "ControlPath=none", "-o", "ExitOnForwardFailure=yes", "-L", "127.0.0.1:{port}:db.example.com:5432", "bastion.example.com"] }
```

No password goes in this file: it comes from the keychain, an environment variable or a command.
A host that is not this machine needs `sslmode=require` in its `url`, or `tls = "off"` on the connection to talk in clear on purpose.
A tunnel's `command` forwards the port pgbg puts in place of `{port}`; pgbg then connects to that port on `127.0.0.1` and still checks the certificate against the host in `url`.
A connection with a tunnel needs `sslmode=require`, or `tls = "off"` on purpose, even when its `url` says `localhost`: another program could answer on that port.
For ssh, keep `ControlPath=none`, so the forward dies with pgbg instead of living on in a shared connection, and `ExitOnForwardFailure=yes`, so ssh stops when it cannot take the port.
An optional `before` command, such as one that wakes the bastion, runs once before each tunnel starts, so it must be safe to run twice.
The TUI reads a connection's schema for its sidebar on a second, short login, the first time it needs it; `schema = "off"` on the connection never does, for a pooler or a `CONNECTION LIMIT` with no login to spare.
Each session gets a statement timeout from its tier: none on local, a minute on staging, 30 seconds on production; `statement_timeout = "2min"` on a connection sets its own, and a stricter one the server sets is kept.
`[display]` with `format = "readable"` opens every result in the TUI readable, as `F` does; the default is `raw`.
`pgbg connection add|edit|remove` and `pgbg password set <connection>` write this file for you and keep your comments.

## Check a connection

`pgbg doctor prod` checks the config, TLS, the tunnel, the login, the statement timeout and whether the role can write, one line each with how to fix it; it exits 1 when a check fails.

## Run a statement

```sh
pgbg run local "SELECT id, total FROM orders LIMIT 5"
pgbg run local "SELECT count(*) FROM orders" --json
pgbg run local "SELECT * FROM orders WHERE day > {{since}}::date" --param since=2024-01-01
pgbg run local --query orders/daily --param since=2024-01-01
pgbg history local --limit 5
pgbg schema local orders
```

Each statement run goes in the connection's history under `pgbg/history/` in your data folder, or the folder `$PGBG_DATA_DIR` names: when, how long and what came of it, never a row.

## Saved queries

Put `.sql` files under `.pgbg/queries/`, in folders if you like:

```sql
-- name: Orders per day
-- chart: bar x=day y=orders
SELECT date_trunc('day', created_at) AS day, count(*) AS orders
FROM orders
GROUP BY 1
ORDER BY 1;
```

In the TUI, `ctrl-p` finds a query by a few letters of its path and opens it in the editor.
`-- format: created_at=relative size=bytes` opens those columns readable; the formats are `raw`, `local`, `relative`, `grouped`, `bytes` and `short`.
Queries of your own, for every project, go under `pgbg/queries/` in your config folder.

## License

MIT
