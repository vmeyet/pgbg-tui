# pgbg

Read your PostgreSQL databases from the terminal: local, staging, and production behind a tunnel.
Read only by default, saved queries in plain `.sql` files, and charts of your results.

The TUI is early: `pgbg` opens it on your first connection, `pgbg tui <connection>` on another; [`docs/roadmap.md`](docs/roadmap.md) says what comes next.

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
tunnel = { command = ["ssh", "-N", "-L", "{port}:db.example.com:5432", "bastion.example.com"] }
```

No password goes in this file: it comes from the keychain, an environment variable or a command.
A host that is not this machine needs `sslmode=require` in its `url`, or `tls = "off"` on the connection to talk in clear on purpose.
A tunnel's `command` forwards the port pgbg puts in place of `{port}`; pgbg then connects to that port on `127.0.0.1` and still checks the certificate against the host in `url`.
An optional `before` command, such as one that wakes the bastion, runs once before each tunnel starts, so it must be safe to run twice.
`pgbg connection add|edit|remove` and `pgbg password set <connection>` write this file for you and keep your comments.

## Run a statement

```sh
pgbg run local "SELECT id, total FROM orders LIMIT 5"
pgbg run local "SELECT count(*) FROM orders" --json
```

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

## License

MIT
