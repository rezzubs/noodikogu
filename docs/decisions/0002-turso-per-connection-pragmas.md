# Turso: per-connection setup options

**Status:** Accepted

Every `PRAGMA`/setting below either lives on the `Connection` struct (reset to
default on every new `turso::Connection`) or on the database file (persists
once set). Scope column says which. Decision: apply the "set" ones inside
`Catalogue`'s private `connect()` helper so no code path can skip them; leave
everything else at its engine default.

## Decision

| Setting | Scope | Default | Set? | Why |
|---|---|---|---|---|
| `foreign_keys` | connection | OFF (`fk_pragma: AtomicBool::new(false)`) | **Set ON** | FK enforcement is opt-in per connection and never persisted. A catalogue schema (pieces → composers, collections, etc.) needs this to catch bad references; skipping it silently disables referential integrity. |
| `busy_timeout` | connection | 0 / fail immediately (`BusyHandler::None`) | **Set** (e.g. 5s) | With one `Database` shared across axum handlers, each request opens its own `Connection`. Default behavior returns `SQLITE_BUSY` the instant another connection holds a write lock, instead of waiting. A short timeout absorbs normal write/write or write/checkpoint contention. |
| `journal_mode` (WAL) | **database file**, persists once set | rollback journal (not WAL) | **Set once** at db creation, not per-connection | WAL is stored in the file header, so it's not a per-connection concern — but it's what makes "many readers + one writer without blocking reads" work at all. Set it once when the catalogue file is first created. |
| `synchronous` | connection | `FULL` | Skip | `FULL` is the safe default and matches what you'd want for a personal catalogue where durability > raw write throughput. Only worth lowering to `NORMAL` if write latency becomes a measured problem (only safe to do so combined with WAL). |
| `require_where` (`IAmADummy`) | connection | OFF | Skip (optional) | Rejects `UPDATE`/`DELETE` with no `WHERE` clause — cheap insurance against an accidental full-table wipe from an app bug. Reasonable to turn on if you want the safety net; skipping because normal `Catalogue` methods should never construct unconditional deletes in the first place, so it mostly guards against a bug that would need fixing anyway. |
| `ignore_check_constraints` | connection | OFF (constraints enforced) | Skip | You want CHECK constraints enforced by default; there's no reason to disable them for a catalogue. |
| `query_only` | connection | OFF | Skip | No current need for a read-only connection mode. Could be useful later for a reporting/read-replica-style connection in the TUI, but not needed at initial design. |
| `cache_size` | connection (initialized from db header) | inherited from file header | Skip | No evidence of a performance problem to tune against; default is shared from the file's own header already. |
| `temp_store` | connection | `DEFAULT` (engine choice) | Skip | No large temp-table/sort workloads expected for a sheet music catalogue; default is fine. |
| `locking_mode` | connection | `NORMAL` | Skip | `NORMAL` is required for the TUI and web server (and multiple axum requests) to hold separate connections concurrently. `EXCLUSIVE` would lock everyone else out — actively wrong for this design. |
| `data_sync_retry` | connection | OFF (fsync error is fatal) | Skip | Guards against a rare fsync-error edge case by retrying instead of erroring. Not worth the complexity until it's an observed problem. |
| `capture_data_changes` (CDC) | connection | OFF | Skip (for now) | Turso-specific change-capture logging. No sync/audit-log feature planned yet; revisit if the catalogue ever needs an offline sync or activity log. |
| `full_column_names` / `short_column_names` | connection | deprecated legacy compat flags | Skip | Only affect column naming in raw result sets from ad-hoc SQL; irrelevant since `Catalogue` maps rows into its own domain types rather than relying on returned column names. |
| encryption (`key`/`cipher`) | database file | none | Skip | Single-user local file, not distributed or synced anywhere sensitive. Revisit only if the catalogue file needs to be encrypted at rest. |

## Net effect on `Catalogue::connect()`

```rust
async fn connect(&self) -> Result<Connection, Error> {
    let conn = self.database.connect()?;
    conn.execute("PRAGMA foreign_keys = ON", ()).await?;
    conn.busy_timeout(BUSY_TIMEOUT)?;
    Ok(conn)
}
```

`journal_mode = WAL` is set once, separately, when the database file is first created (in `Catalogue::open`), not in this per-connection helper — see `src/catalogue.rs`.

## Sources

- Local crate source (`~/.cargo/registry/src/.../turso-0.6.1`,
  `turso_core-0.6.1`, `turso_parser-0.6.1`, version 0.6.1 as vendored in
  this project's `Cargo.lock`):
  - `turso_core-0.6.1/lib.rs:1673-1730` — `Connection` struct fields and
    their per-connection defaults.
  - `turso_core-0.6.1/connection.rs`, `translate/pragma.rs` — pragma
    read/write implementations referenced in the table above.
  - `turso_parser-0.6.1/src/ast.rs` (`PragmaName` enum) — full list of
    supported pragmas.
- `docs/decisions/0001-turso-connection-design.md` — prior decision this
  one builds on (`Database` vs `Connection` ownership).
