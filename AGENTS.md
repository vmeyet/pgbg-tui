# Working in this repo

`pgbg` is a Rust TUI and CLI for reading PostgreSQL databases: local, staging and production behind a tunnel.
Read [`ARCHITECTURE.md`](ARCHITECTURE.md) first: the module map, the data flow, the boundaries and where to add things.
The code is the contract; `ARCHITECTURE.md` describes it.
When an MR changes a boundary or a flow, it updates `ARCHITECTURE.md` too.
[`docs/roadmap.md`](docs/roadmap.md) holds the order of work; the MR that ships a step ticks its box.

## Ground rules

- One MR per change a reviewer can read in one sitting, under 500 changed lines excluding fixtures and snapshots.
- Branches: `<scope>/<kebab-title>` with scope in `feat fix chore refactor docs test ci`; worktrees under `.claude/worktrees/<scope>+<kebab-title>`.
- Commits follow conventional commits; body in two short imperative sentences at most.
- `docs/` is for users and for the roadmap.
- Markdown files put one sentence per line.

## Code

- Rust 2024, `rustfmt.toml` as committed (140 columns, `use_small_heuristics = "Max"`).
- Lints live in `[lints]` in `Cargo.toml`: `cargo clippy --all-targets -- -D warnings` must be clean, and `unwrap`/`expect` only live in tests.
- Doc comments (`///`, `//!`) say what an item is for or why it is so; they describe the item they sit on and stay true when it changes.
- No comment restates the code; a `//` comment says why, and only when no name can.
- Immutability across function boundaries: pass helpers values or `&` borrows and take back a new value; only the owner of a state changes it, through `&mut self`.
- Side effects at the edges only, as `ARCHITECTURE.md` § Boundaries lists them; `tui/app/` stays pure.
- Flat bodies of named steps at one level of abstraction.
- Design a signature from the call site: a caller never needs to read the body to use it right.
- One word per concept: the glossary in `ARCHITECTURE.md`.
- Anything that can run twice (tunnel start, history writes, query save) is safe to run twice.
- Only `src/db/` sees a `tokio_postgres` type; everything else uses `src/db/model/`.

## Rust design rules

- Domain values get newtypes, not bare `String` or `u16` (connection names, query paths, ports, SQL text): the compiler then stops a mix-up.
- Parse into those types at the boundary (config, wire, files) and keep the invariant inside the type: code above it never checks again.
- Visibility is the smallest that compiles, and `unreachable_pub` enforces it: private by default, `pub(super)` or `pub(crate)` only where used.
  `pub` is only for what `main.rs` calls; `tests/` drive the binary.
  `clippy::redundant_pub_crate` stays off, it fights `unreachable_pub` inside private modules.
- Never drop a `Result` silently: `let _ =` or `.ok()` on a fallible call needs a reason a reader can see, and best-effort work logs the failure with `tracing::warn!`.
- `anyhow` at the edges; a typed error (`thiserror` or an enum) where a caller branches on the kind: a string is not a contract.
- No `bool` parameter on a function with more than one caller: use a two-variant enum or two functions, so the call site reads.
- No tuple or tuple alias for data that travels between modules: a struct with named fields says what each part is.
- A name says what the symbol does without reading its body.
- No shared mutable state across tasks when message passing works: the loop and its channel are the model.
- State shared between tasks (`Arc<Mutex<_>>`, atomics) says in a one-line doc comment on its field why it is shared.
- Prefer owned values passed by move, or `&` borrows, over clones; a `.clone()` of a result set or a row on a hot path needs a reason.
- Never block the runtime while the TUI loop runs: `std::process`, `std::fs` and heavy CPU work go through `spawn_blocking` or their tokio form.
- Never hold a `std::sync::Mutex` guard across an `.await`: take what you need and drop the guard.
- Tests are deterministic: no real network, no real clock, and no real-time sleep to make tasks overlap.
- Tests that need a real PostgreSQL are `#[ignore]`d and read `PGBG_TEST_DATABASE_URL`; CI runs them against a service container.
- One behaviour per test, named as a sentence that says it.
- `unsafe` only inside a safe function whose runtime checks make its `SAFETY:` comment true, not just a claim.
- One crate per job: prefer std or a dependency already in `Cargo.toml`; a new one needs a line in the MR saying why.
- About 800 lines is the guideline for a file, not a hard limit; a file grown past it is split when next touched.
- A module with children lives in `foo/mod.rs`, never `foo.rs` beside a `foo/` folder; `clippy::self_named_module_files` enforces it.
- A type with more than one reason to change gets split; a god struct is split by concern.

## Releases

- `CHANGELOG.md` is for users.
- An MR with a `feat` or `fix` a user can notice adds its entry under `## Unreleased` in the same MR.
- Entries sit under `### New`, `### Fixed` or `### Removed`, in that order.
- An entry is one sentence per line: what it is in bold, where it applies, then how to use it.

## Tests

```sh
cargo test                                   # unit, snapshots, CLI
cargo test -- --ignored                      # against PGBG_TEST_DATABASE_URL, a throwaway database
cargo insta review                           # after a deliberate rendering change
```

Every module ships its tests in the same file; snapshot tests render on `TestBackend`.
Fixtures are invented: database `shop`, tables `orders` and `customers`, user `nina`, host `db.example.com`.

## Security

The model is in `ARCHITECTURE.md` § Read only and § Secrets; these are the rules that keep it.

- Read only is a setting of each connection, and the default; writes need `writes = "allowed"` on that connection, whatever its tier.
- Every statement runs alone, in a `BEGIN READ ONLY` transaction that is always rolled back; `db::guard` refuses what is not a read before it reaches the server.
- The real guarantee is a read-only database role: the docs say so, and `pgbg doctor` warns when the role can write.
- No password in the config file or a URL in it: a password comes from the keychain, an environment variable or a command; the config loader refuses a `password` key.
- No secret in logs, history, cache, fixtures or `Debug` output; `Secret` prints `<redacted>`.
- TLS through rustls; a non-local host without `sslmode = "require"` or stronger is refused unless the connection says `tls = "off"` on purpose.
- A tunnel is a child process pgbg owns: it binds `127.0.0.1` only, and it dies with pgbg.
- No telemetry and no crash reporting.
- The repo may go public: no real host, project, database or table name, no work email in commits.
