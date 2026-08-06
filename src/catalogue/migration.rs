//! A hand-rolled, programmatic schema-migration runner.

mod m0001_scores_and_titles;

use super::{DatabaseError, query_one};
use std::future::Future;
use std::pin::Pin;
use turso::Connection;

/// The boxed future returned by a [`Migration::apply`] function.
type MigrationFuture<'a> = Pin<Box<dyn Future<Output = Result<(), DatabaseError>> + Send + 'a>>;

/// A single schema change.
///
/// `apply` receives a `&Connection` - in practice always a `&Transaction`
/// (which derefs to `Connection`, see `docs/decisions/0003-migrations.md`) -
/// so a migration can run arbitrary Rust: read rows, transform them, and
/// write them back inside the same transaction, not just execute a fixed
/// SQL string.
///
/// A migration's schema version is its 1-based position in [`ALL`], not a field
/// on this type - there's only one place that could ever disagree with itself.
/// Each migration submodule is named and numbered to match
/// (`m0001_scores_and_titles`, `m0002_...`, ...), the module name/file prefix
/// is where the version actually lives; `name` below is just a description.
///
/// Migrations are forward-only: there's no `down`/revert. This project's
/// recovery mechanism for a bad state is restoring a backup, not undoing a
/// migration (see `docs/decisions/0003-migrations.md`).
///
/// # Example
///
/// ```ignore
/// const ALL: &[Migration] = &[Migration {
///     name: "create_scores_table",
///     apply: |conn| Box::pin(async move {
///         conn.execute_batch("CREATE TABLE scores (id INTEGER PRIMARY KEY)")
///             .await?;
///         Ok(())
///     }),
/// }];
/// ```
#[derive(Debug, Clone, Copy)]
pub(crate) struct Migration {
    /// A short human-readable description, for debugging/logging only -
    /// not an identifier. A migration's actual identity is its numeric
    /// position in [`ALL`] (see the submodule naming convention above).
    pub name: &'static str,
    /// Runs the migration's schema change against `conn`.
    pub apply: fn(&Connection) -> MigrationFuture<'_>,
}

/// All migrations, in the order they should be applied.
pub(crate) const ALL: &[Migration] = &[m0001_scores_and_titles::MIGRATION];

/// Brings `conn`'s database up to date by applying every migration in
/// `migrations` whose position is greater than the current
/// `PRAGMA user_version`, in order.
///
/// Each migration runs inside its own transaction. If `apply` returns
/// `Err`, the `Transaction` guard rolls back on drop (no manual `ROLLBACK`
/// needed) and `user_version` is left unchanged for that migration.
pub(crate) async fn run(
    conn: &mut Connection,
    migrations: &[Migration],
) -> Result<(), DatabaseError> {
    let current = user_version(conn).await?;
    tracing::debug!(current, "checking for pending migrations");

    for (index, migration) in migrations.iter().enumerate() {
        let version = i64::try_from(index).expect("migration version out of bounds") + 1;
        if version <= current {
            continue;
        }

        tracing::info!(version, name = migration.name, "applying migration");

        let tx = conn.transaction().await?;
        if let Err(error) = (migration.apply)(&tx).await {
            tracing::error!(version, name = migration.name, %error, "migration failed");
            return Err(error);
        }
        tx.execute(&format!("PRAGMA user_version = {version}"), ())
            .await?;
        tx.commit().await?;
    }

    Ok(())
}

/// Reads the current `PRAGMA user_version`.
async fn user_version(conn: &Connection) -> Result<i64, DatabaseError> {
    let mut rows = conn.query("PRAGMA user_version", ()).await?;
    let row = query_one(&mut rows)
        .await?
        .expect("PRAGMA user_version always returns exactly one row");
    row.get::<i64>(0).map_err(DatabaseError::from)
}

/// Test-only fixture: a fresh in-memory connection with foreign keys
/// enabled, and nothing else migrated yet.
#[cfg(test)]
pub(crate) async fn test_connection() -> Connection {
    let db = turso::Builder::new_local(":memory:").build().await.unwrap();
    let conn = db.connect().unwrap();
    conn.execute("PRAGMA foreign_keys = ON", ()).await.unwrap();
    conn
}

/// Test-only: the first `version` entries of [`ALL`] - i.e. the schema
/// exactly as migration `version` leaves it, unaffected by anything after
/// it in history.
///
/// A migration's own tests should always fetch their fixture through this
/// (passing their own submodule's `VERSION`), never `ALL` directly: `ALL`
/// always reflects the *current tip* of history, so if a later migration
/// ever alters something an earlier one established, using `ALL` would
/// silently start running the earlier migration's tests against the wrong
/// schema instead of the one they actually claim to test. `version` only
/// needs to be right once, at the point a migration is written - history
/// is forward-only and immutable (see `docs/decisions/0003-migrations.md`),
/// so nothing before it can ever change position afterward.
///
/// (Testing that the *entire* history replays cleanly end to end is a
/// separate concern, already covered by
/// `catalogue::tests::open_succeeds_on_a_fresh_in_memory_database`.)
#[cfg(test)]
pub(crate) fn history_through(version: usize) -> &'static [Migration] {
    &ALL[..version]
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn fresh_conn() -> Connection {
        let db = turso::Builder::new_local(":memory:").build().await.unwrap();
        db.connect().unwrap()
    }

    #[tokio::test]
    async fn fresh_database_starts_at_version_zero() {
        let conn = fresh_conn().await;
        assert_eq!(user_version(&conn).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn migrations_apply_in_order() {
        let mut conn = fresh_conn().await;
        let migrations = [
            Migration {
                name: "create_log",
                apply: |c| {
                    Box::pin(async move {
                        c.execute_batch("CREATE TABLE log (step INTEGER)").await?;
                        c.execute("INSERT INTO log (step) VALUES (1)", ()).await?;
                        Ok(())
                    })
                },
            },
            Migration {
                name: "append_two",
                apply: |c| {
                    Box::pin(async move {
                        c.execute("INSERT INTO log (step) VALUES (2)", ()).await?;
                        Ok(())
                    })
                },
            },
        ];

        run(&mut conn, &migrations).await.unwrap();

        assert_eq!(user_version(&conn).await.unwrap(), 2);
        let mut rows = conn
            .query("SELECT step FROM log ORDER BY rowid", ())
            .await
            .unwrap();
        let mut steps = Vec::new();
        while let Some(row) = rows.next().await.unwrap() {
            steps.push(row.get::<i64>(0).unwrap());
        }
        assert_eq!(steps, vec![1, 2]);
    }

    #[tokio::test]
    async fn rerunning_already_applied_migrations_is_a_no_op() {
        let mut conn = fresh_conn().await;
        let migrations = [Migration {
            name: "create_log",
            apply: |c| {
                Box::pin(async move {
                    c.execute_batch("CREATE TABLE log (step INTEGER)").await?;
                    c.execute("INSERT INTO log (step) VALUES (1)", ()).await?;
                    Ok(())
                })
            },
        }];

        run(&mut conn, &migrations).await.unwrap();
        run(&mut conn, &migrations).await.unwrap();

        assert_eq!(user_version(&conn).await.unwrap(), 1);
        let mut rows = conn.query("SELECT COUNT(*) FROM log", ()).await.unwrap();
        let row = rows.next().await.unwrap().unwrap();
        assert_eq!(row.get::<i64>(0).unwrap(), 1);
    }

    #[tokio::test]
    async fn failing_migration_rolls_back_and_does_not_bump_version() {
        let mut conn = fresh_conn().await;
        let migrations = [
            Migration {
                name: "create_log",
                apply: |c| {
                    Box::pin(async move {
                        c.execute_batch("CREATE TABLE log (step INTEGER)").await?;
                        Ok(())
                    })
                },
            },
            Migration {
                name: "insert_then_fail",
                apply: |c| {
                    Box::pin(async move {
                        c.execute("INSERT INTO log (step) VALUES (2)", ()).await?;
                        c.execute("this is not valid sql", ()).await?;
                        Ok(())
                    })
                },
            },
        ];

        assert!(run(&mut conn, &migrations).await.is_err());

        assert_eq!(user_version(&conn).await.unwrap(), 1);
        let mut rows = conn.query("SELECT COUNT(*) FROM log", ()).await.unwrap();
        let row = rows.next().await.unwrap().unwrap();
        assert_eq!(row.get::<i64>(0).unwrap(), 0);
    }
}
