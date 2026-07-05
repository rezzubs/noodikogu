//! A hand-rolled, programmatic schema-migration runner.

use super::Error;
use std::future::Future;
use std::pin::Pin;
use turso::Connection;

/// The boxed future returned by a [`Migration::apply`] function.
type MigrationFuture<'a> = Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>>;

/// A single schema change.
///
/// `apply` receives a `&Connection` — in practice always a `&Transaction`
/// (which derefs to `Connection`, see `docs/decisions/0003-migrations.md`)
/// — so a migration can run arbitrary Rust: read rows, transform them, and
/// write them back inside the same transaction, not just execute a fixed
/// SQL string.
///
/// A migration's schema version is its 1-based position in [`ALL`], not a
/// field on this type — there's only one place that could ever disagree
/// with itself.
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
    /// A short human-readable name, for debugging/logging only.
    pub name: &'static str,
    /// Runs the migration's schema change against `conn`.
    pub apply: fn(&Connection) -> MigrationFuture<'_>,
}

/// All migrations, in the order they should be applied.
pub(crate) const ALL: &[Migration] = &[];

/// Brings `conn`'s database up to date by applying every migration in
/// `migrations` whose position is greater than the current
/// `PRAGMA user_version`, in order.
///
/// Each migration runs inside its own transaction. If `apply` returns
/// `Err`, the `Transaction` guard rolls back on drop (no manual `ROLLBACK`
/// needed) and `user_version` is left unchanged for that migration.
pub(crate) async fn run(conn: &mut Connection, migrations: &[Migration]) -> Result<(), Error> {
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
async fn user_version(conn: &Connection) -> Result<i64, Error> {
    let mut rows = conn.query("PRAGMA user_version", ()).await?;
    let row = rows
        .next()
        .await?
        .expect("PRAGMA user_version always returns exactly one row");
    row.get::<i64>(0).map_err(Error::from)
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
