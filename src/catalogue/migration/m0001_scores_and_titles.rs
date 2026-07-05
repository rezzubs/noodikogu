//! The first schema migration: `scores`, `titles`, and a hand-rolled
//! word-prefix search index over title text (`title_words`).

use super::Migration;

/// This migration's 1-based position in [`super::ALL`] - must match both
/// this module's `m0001_` file prefix and its actual index in `ALL`. Only
/// needs to be right once: history is forward-only and immutable, so
/// nothing can ever get inserted before this migration afterward.
#[cfg(test)]
pub(crate) const VERSION: usize = 1;

pub(crate) const MIGRATION: Migration = Migration {
    name: "create scores and titles tables",
    apply: |conn| {
        Box::pin(async move {
            conn.execute_batch(include_str!("m0001_scores_and_titles.sql"))
                .await?;
            Ok(())
        })
    },
};

#[cfg(test)]
mod tests {
    use super::VERSION;
    use crate::catalogue::migration;
    use turso::Connection;

    /// A fresh connection with history through this migration applied -
    /// nothing after it, so these tests reflect exactly the schema this
    /// migration establishes, regardless of what future migrations do.
    async fn migrated_conn() -> Connection {
        let mut conn = migration::test_connection().await;
        migration::run(&mut conn, migration::history_through(VERSION))
            .await
            .unwrap();
        conn
    }

    #[tokio::test]
    async fn creates_scores_titles_and_title_words() {
        let conn = migrated_conn().await;
        conn.execute(
            "INSERT INTO scores (id, created_at) VALUES (1, '2026-01-01T00:00:00Z')",
            (),
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO titles (id, score_id, value, value_normalized, is_primary)
             VALUES (1, 1, 'Ave Maria', 'ave maria', 1)",
            (),
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO title_words (title_id, word) VALUES (1, 'ave')",
            (),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn deleting_a_score_cascades_to_titles_and_title_words() {
        let conn = migrated_conn().await;
        conn.execute(
            "INSERT INTO scores (id, created_at) VALUES (1, '2026-01-01T00:00:00Z')",
            (),
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO titles (id, score_id, value, value_normalized, is_primary)
             VALUES (1, 1, 'Ave Maria', 'ave maria', 1)",
            (),
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO title_words (title_id, word) VALUES (1, 'ave')",
            (),
        )
        .await
        .unwrap();

        conn.execute("DELETE FROM scores WHERE id = 1", ())
            .await
            .unwrap();

        let mut rows = conn.query("SELECT COUNT(*) FROM titles", ()).await.unwrap();
        let row = rows.next().await.unwrap().unwrap();
        assert_eq!(row.get::<i64>(0).unwrap(), 0);

        let mut rows = conn
            .query("SELECT COUNT(*) FROM title_words", ())
            .await
            .unwrap();
        let row = rows.next().await.unwrap().unwrap();
        assert_eq!(row.get::<i64>(0).unwrap(), 0);
    }

    #[tokio::test]
    async fn only_one_primary_title_allowed_per_score() {
        let conn = migrated_conn().await;
        conn.execute(
            "INSERT INTO scores (id, created_at) VALUES (1, '2026-01-01T00:00:00Z')",
            (),
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO titles (id, score_id, value, value_normalized, is_primary)
             VALUES (1, 1, 'Ave Maria', 'ave maria', 1)",
            (),
        )
        .await
        .unwrap();

        let result = conn
            .execute(
                "INSERT INTO titles (id, score_id, value, value_normalized, is_primary)
                 VALUES (2, 1, 'Ave Maria (alt)', 'ave maria alt', 1)",
                (),
            )
            .await;
        assert!(result.is_err());
    }
}
