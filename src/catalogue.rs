//! Database connection and schema-migration plumbing for the score
//! catalogue.

mod error;
mod migration;
mod normalize;

pub use error::Error;

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
    pub async fn open(path: &str) -> Result<Self, Error> {
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
    async fn connect(&self) -> Result<Connection, Error> {
        let conn = self.database.connect()?;
        conn.execute("PRAGMA foreign_keys = ON", ()).await?;
        conn.busy_timeout(BUSY_TIMEOUT)?;
        Ok(conn)
    }
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
}
