# Migrations: hand-rolled, tracked via `PRAGMA user_version`

**Status:** Accepted

Decision about how `catalogue` versions and applies schema changes:
external migration-framework dependency vs. something hand-rolled, and how
"which migrations have run" gets tracked.

## Decision

- No external migration-framework dependency (e.g. `refinery`,
  `rusqlite_migration`, `sqlx::migrate!`). Migrations are a small
  hand-rolled `Migration` type plus a `run()` function in
  `src/catalogue/migration.rs`.
- Each `Migration` is a `name: &'static str` and an
  `apply: fn(&Connection) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send>>`
  — arbitrary Rust, not a fixed SQL string. A migration can read rows,
  transform them, and write them back, not just execute one `.sql` file.
  There's no explicit `version` field or a `down`/revert counterpart:
  a migration's version is its 1-based position in the migration list, and
  migrations are forward-only — this project's recovery mechanism for a
  bad state is restoring a backup, not undoing a migration.
- Applied schema state is tracked via `PRAGMA user_version`, a persisted
  integer in the database file header — no separate bookkeeping table.
- Each migration runs inside its own transaction (`conn.transaction()`,
  default `TransactionBehavior::Deferred`, i.e. plain `BEGIN`, never
  `BEGIN CONCURRENT`). `user_version` is bumped inside that same
  transaction, right before commit. If a migration's `apply` returns
  `Err`, the `Transaction` guard rolls back on drop — no manual
  `ROLLBACK` needed, and `user_version` is left at its previous value.

## Why

- **Arbitrary-Rust requirement.** Some future migrations (e.g. splitting a
  name into `person_names` components) need real Rust logic over rows, not
  just a `.sql` file — most migration frameworks in the Rust ecosystem
  assume "a migration is a SQL string," which doesn't fit.
- **Avoiding a dependency for little benefit.** Existing SQL-file migration
  crates target `rusqlite`/`sqlx` connection types, not `turso`; adapting
  one would mean writing a compatibility shim for a problem a small amount
  of hand-rolled code solves directly.
- **`user_version` is exactly the mechanism SQLite designed for this.**
  Fully supported by turso — `ReadCookie`/`SetCookie` on
  `Cookie::UserVersion` (`turso_core-0.6.1/translate/pragma.rs`). No need
  for a parallel `schema_migrations` table.
- **DDL transaction requirement.** DDL statements require an exclusive
  (non-MVCC) transaction — plain `BEGIN`, not `BEGIN CONCURRENT`
  (`turso_core-0.6.1/vdbe/execute.rs:10418`). The default
  `TransactionBehavior::Deferred` already satisfies this, so migrations
  need no special-cased transaction mode.
- **No savepoints.** Turso doesn't support savepoints, so a migration can't
  be partially rolled back internally — each migration must be exactly one
  flat transaction. `Connection::transaction()`'s roll-back-by-default-on-drop
  gives us that for free via a single `?` in the migration body.

## Sources

- Local crate source (`~/.cargo/registry/src/.../turso-0.6.1`,
  `turso_core-0.6.1`, version 0.6.1 as vendored in this project's
  `Cargo.lock`):
  - `turso_core-0.6.1/translate/pragma.rs` — `PRAGMA user_version` /
    `Cookie::UserVersion` read and write support.
  - `turso_core-0.6.1/vdbe/execute.rs:10418` — DDL requires an exclusive
    transaction, not `BEGIN CONCURRENT`.
  - `turso-0.6.1/src/transaction.rs` — `Transaction` defaults to
    `DropBehavior::Rollback`; `commit()` consumes and commits explicitly;
    `Transaction: Deref<Target = Connection>`.
  - `turso-0.6.1/src/connection.rs:261` — `Connection::transaction(&mut self)`.
- `docs/decisions/0001-turso-connection-design.md`,
  `docs/decisions/0002-turso-per-connection-pragmas.md` — prior decisions
  this one builds on (`Database` vs `Connection` ownership, per-connection
  setup).
