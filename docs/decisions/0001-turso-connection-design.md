# Turso: Database vs Connection design decision

**Status:** Accepted

Decision about how the `Catalogue` will handle Turso `Database` vs `Connection` ownership and usage. Own a connection vs a database? Pooling?

## Decision

- `Catalogue` owns a `turso::Database`, not a `turso::Connection`.
- `Database` is cheap to `Clone` (it's `#[derive(Clone)]`, `Arc`-backed
  internally) and is meant to be shared across the whole app (TUI state, axum
  `AppState`).
- Each `Catalogue` method calls `self.database.connect()` internally to get a
  fresh `Connection` for that operation, then maps the result into `noodikogu`'s
  own domain types/errors. Neither the TUI nor the web server import `turso`
  directly or see turso types — they only call `Catalogue` methods.
- This is the pattern the official Turso + Axum guide documents: share
  `Database` in state, call `.connect()` per request. No hand-rolled connection
  pool — building one would go against the documented pattern with no evidence
  it's needed yet.
- Multiple simultaneous `Connection`s to the same `Database` in one process is
  the *intended* mechanism for concurrency, not an edge case to avoid. Each
  connection has its own transaction state, so concurrent axum requests each
  getting their own `Connection` is exactly how isolation between requests is
  supposed to work.
- Concurrent *writes* (`BEGIN CONCURRENT` / MVCC) are explicitly experimental /
  "early technology preview" upstream — don't design around concurrent writers
  yet. Default semantics are standard SQLite/WAL: many concurrent readers, one
  writer at a time.
- The "No multi-threading" line in Turso's own limitations list is a
  SQLite-compatibility gap (SQLite's configurable threading modes:
  single-thread/multi-thread/serialized), **not** a warning against using
  connections from multiple OS threads. `turso_core` explicitly asserts
  `Connection: Send + Sync` at compile time, and the official axum example runs
  on tokio's default multi-threaded runtime without comment.

## Why `Database`, not `Connection`, as the owned handle

A `turso::Connection` carries mutable per-connection state:
`transaction_behavior`, `dangling_tx` (an unfinished transaction left dangling
on drop), autocommit status, busy timeout. If two concurrent axum requests
shared clones of one `Connection`, one request's transaction could leak into
another's. `Database` has no query methods of its own — its only real job is
`connect()`, i.e. minting isolated connections — so it's the safe thing to hold
and share.

## `connect()` is not free, but the docs don't treat it as a problem

Reading `turso_core-0.6.1/lib.rs` (`Database::connect` → `_connect` → `_init`),
every `connect()` call does real work: builds a new `Pager`, begins a read
transaction to read page 1 (page size), deep-clones the in-memory schema,
refreshes ANALYZE stats, and allocates a `Connection` struct with ~40
atomic/lock fields. A doc comment on the wrapper's `Connection`
(`turso-0.6.1/src/connection.rs:41`) claims dropped connections get recycled
into a "ConnectionPool" — but there is no `ConnectionPool` type and no `Drop`
impl anywhere in `turso`, `turso_sdk_kit`, or `turso_core` (checked via source
search across all turso crates in the local cargo registry, version 0.6.1). That
comment appears aspirational/stale, not a description of real pooling.

Despite that real cost, the *official* Turso + Axum guide calls `.connect()` per
request with no pool and no caveat about overhead. Given `noodikogu` is a
modest-scale personal catalogue, connect-per-operation is the right default;
only build a real connection pool later if profiling shows it matters.

## Sources

- [Turso + Axum guide](https://docs.turso.tech/sdk/rust/guides/axum) — the
  `AppState { db: Database }` + per-request `.connect()` pattern.
- [Turso Rust quickstart](https://docs.turso.tech/sdk/rust) — basic `Builder` →
  `Database` → `Connection` flow.
- [`tursodatabase/turso/docs/manual.md`](https://github.com/tursodatabase/turso/blob/main/docs/manual.md)
  — "Each connection can have exactly one active transaction at a time... When
  you need concurrency (including `BEGIN CONCURRENT`), you need to use
  *different connections*"; also the "Limitations" list containing "No
  multi-threading" / "No multi-process access".
- [Beyond the Single-Writer Limitation with Turso's Concurrent
  Writes](https://turso.tech/blog/beyond-the-single-writer-limitation-with-tursos-concurrent-writes)
  — MVCC/concurrent writes are "early technology preview," not for production
  use.
- [Multi-Process Access -
  Turso](https://docs.turso.tech/sql-reference/multiprocess-access) — clarifies
  the multi-process limitation separately from threading.
- Local crate source (`~/.cargo/registry/src/.../turso-0.6.1`,
  `turso_core-0.6.1`, `turso_sdk_kit-0.6.1`, version 0.6.1 as vendored in this
  project's `Cargo.lock`):
  - `turso-0.6.1/src/lib.rs` — `Database` struct, `#[derive(Clone)]`,
    `connect()`.
  - `turso-0.6.1/src/connection.rs` — `Connection` struct fields
    (`transaction_behavior`, `dangling_tx`, etc.),
    `assert_send_sync!(Connection)`, and the unimplemented "ConnectionPool" doc
    comment.
  - `turso_core-0.6.1/lib.rs:1636-1738` — `Database::connect` / `_connect` /
    `_init`, showing the real per-connection setup cost.
  - `turso_sdk_kit-0.6.1/src/rsapi.rs:739` — `TursoDatabase::connect`,
    confirming no pool checkout happens.
