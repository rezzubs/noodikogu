//! Database connection and schema-migration plumbing for the score
//! catalogue.

mod error;
mod id;
mod migration;
mod normalize;
mod params;
mod person;
mod schema;
mod score;
mod search;
mod tag;
mod title;

pub use error::DatabaseError;
pub use id::{PersonId, ScoreId, TagId, TitleId};
pub use person::{AttachPersonError, CreatePersonError, DeletePersonError, DetachPersonError};
pub use score::{
    DeleteScoreError, PersonDetail, ScoreDetail, ScoreDetailError, SetDescriptionError, TagDetail,
};
pub use search::{Pagination, ScoreSummary};
pub use tag::{AttachTagError, CreateTagError, DeleteTagError, DetachTagError};
pub use title::{
    AddTitleError, EmptyTitleError, RemoveTitleError, SetPrimaryTitleError, Title,
    UpdateTitleValueError,
};

use std::time::Duration;
use turso::{Connection, Database};

/// Busy timeout applied to every connection [`Catalogue::connect`] opens.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// A handle to the catalogue's backing database.
///
/// Cheap to clone. Obtain one with [`Catalogue::open`].
#[derive(Debug, Clone)]
pub struct Catalogue {
    database: Database,
}

impl Catalogue {
    /// Opens (creating if necessary) the catalogue database at `path`,
    /// enables WAL mode, and migrates the schema to the latest version.
    ///
    /// `path` may be `":memory:"` for a private in-memory database, used in
    /// tests.
    ///
    /// # Errors
    ///
    /// Returns [`DatabaseError`] if the database can't be opened, WAL mode
    /// can't be enabled, or migrating the schema fails.
    pub async fn open(path: &str) -> Result<Self, DatabaseError> {
        tracing::info!(path, "opening catalogue");
        let database = turso::Builder::new_local(path).build().await?;

        // WAL is stored in the file header, not per-connection, so it only
        // needs to be set once here, not in `connect()`. `pragma_update` is
        // used instead of `execute` because `PRAGMA journal_mode = ...`
        // returns the resulting mode as a row, which `execute` rejects.
        let wal_conn = database.connect()?;
        wal_conn.pragma_update("journal_mode", "WAL").await?;

        let catalogue = Self { database };
        let mut conn = catalogue.connect().await?;
        migration::run(&mut conn, migration::ALL).await?;

        Ok(catalogue)
    }

    /// Opens a fresh connection with per-connection setup applied: foreign key
    /// enforcement and a busy timeout. Every `Catalogue` method should get its
    /// connection through here, never via `self.database.connect()` directly.
    async fn connect(&self) -> Result<Connection, DatabaseError> {
        let conn = self.database.connect()?;
        conn.execute("PRAGMA foreign_keys = ON", ()).await?;
        conn.busy_timeout(BUSY_TIMEOUT)?;
        Ok(conn)
    }
}

/// Reads at most one row from `rows` (for queries that can only ever match
/// zero or one row, e.g. a lookup by primary key or an aggregate like
/// `COUNT(*)`), then fully drains the cursor.
///
/// The draining is not optional bookkeeping: `turso` 0.6.1 requires a
/// `Rows` cursor to be driven all the way to `None` before it considers the
/// underlying statement finished. Stopping after the first `Some(row)` and
/// just dropping the cursor leaves the statement in a state that can make
/// a *later* statement in the same transaction fail at `commit()` with
/// "no transaction is active" - reproduced empirically (not documented
/// turso behavior as of this version), so every single-row lookup in this
/// module goes through here rather than a bare `rows.next().await`.
pub(crate) async fn query_one(rows: &mut turso::Rows) -> Result<Option<turso::Row>, DatabaseError> {
    let first = rows.next().await?;
    while rows.next().await?.is_some() {}
    Ok(first)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn open_succeeds_on_a_fresh_in_memory_database() {
        Catalogue::open(":memory:").await.unwrap();
    }

    #[tokio::test]
    async fn connect_enables_foreign_keys() {
        let catalogue = Catalogue::open(":memory:").await.unwrap();
        let conn = catalogue.connect().await.unwrap();
        let mut rows = conn.query("PRAGMA foreign_keys", ()).await.unwrap();
        let row = rows.next().await.unwrap().unwrap();
        assert_eq!(row.get::<i64>(0).unwrap(), 1);
    }

    /// Regression test for the `query_one` drain requirement documented
    /// above. The bug only manifests when the looked-up row actually
    /// exists (`Some`, not `None` - an empty result already exhausts the
    /// cursor on its own on the first `.next()` call) *and* something else
    /// runs afterward in the same transaction. If `query_one` ever stopped
    /// draining (e.g. reverted to a bare `rows.next().await`), the
    /// `tx.commit()` below would fail with "no transaction is active".
    #[tokio::test]
    async fn query_one_drains_so_a_later_statement_can_still_commit() {
        let database = turso::Builder::new_local(":memory:").build().await.unwrap();
        let mut conn = database.connect().unwrap();
        conn.execute("PRAGMA foreign_keys = ON", ()).await.unwrap();
        migration::run(&mut conn, migration::ALL).await.unwrap();
        conn.execute(
            "INSERT INTO scores (created_at) VALUES ('2026-01-01T00:00:00Z')",
            (),
        )
        .await
        .unwrap();

        let tx = conn.transaction().await.unwrap();

        let mut rows = tx
            .query("SELECT 1 FROM scores WHERE id = 1", ())
            .await
            .unwrap();
        let found = query_one(&mut rows).await.unwrap();
        assert!(found.is_some(), "the seeded row should be found");
        drop(rows);

        // Any further statement in the same transaction is enough to
        // surface the bug - an INSERT here, but `set_description`'s
        // existence-check-then-UPDATE shape is what originally caught it.
        tx.execute(
            "INSERT INTO scores (created_at) VALUES ('2026-01-02T00:00:00Z')",
            (),
        )
        .await
        .unwrap();

        tx.commit().await.unwrap();
    }
}
