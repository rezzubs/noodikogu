//! The second schema migration: `people`/`person_words`/`score_people` for
//! person search, and `tags`/`score_tags` for tag search.

use super::Migration;

/// This migration's 1-based position in [`super::ALL`] - must match both
/// this module's `m0002_` file prefix and its actual index in `ALL`. Only
/// needs to be right once: history is forward-only and immutable, so
/// nothing can ever get inserted before this migration afterward.
#[cfg(test)]
pub(crate) const VERSION: usize = 2;

pub(crate) const MIGRATION: Migration = Migration {
    name: "create people, person_words, score_people, tags, and score_tags tables",
    apply: |conn| {
        Box::pin(async move {
            conn.execute_batch(include_str!("m0002_people_and_tags.sql"))
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

    async fn seed_score(conn: &Connection) {
        conn.execute(
            "INSERT INTO scores (id, created_at) VALUES (1, '2026-01-01T00:00:00Z')",
            (),
        )
        .await
        .unwrap();
    }

    async fn seed_person(conn: &Connection) {
        conn.execute(
            "INSERT INTO people (id, display_name, display_name_key)
             VALUES (1, 'Johann Bach', 'johann bach')",
            (),
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO person_words (person_id, word) VALUES (1, 'johann'), (1, 'bach')",
            (),
        )
        .await
        .unwrap();
    }

    async fn seed_tag(conn: &Connection) {
        conn.execute(
            "INSERT INTO tags (id, name, name_normalized) VALUES (1, 'key', 'key')",
            (),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn creates_people_person_words_and_score_people() {
        let conn = migrated_conn().await;
        seed_score(&conn).await;
        seed_person(&conn).await;
        conn.execute(
            "INSERT INTO score_people (score_id, person_id) VALUES (1, 1)",
            (),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn creates_tags_and_score_tags_with_a_per_attachment_value() {
        let conn = migrated_conn().await;
        seed_score(&conn).await;
        seed_tag(&conn).await;
        conn.execute(
            "INSERT INTO score_tags (score_id, tag_id, value, value_normalized)
             VALUES (1, 1, 'CMajor', 'cmajor')",
            (),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn score_tags_value_can_be_null() {
        let conn = migrated_conn().await;
        seed_score(&conn).await;
        seed_tag(&conn).await;
        conn.execute(
            "INSERT INTO score_tags (score_id, tag_id) VALUES (1, 1)",
            (),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn deleting_a_score_cascades_to_score_people_and_score_tags() {
        let conn = migrated_conn().await;
        seed_score(&conn).await;
        seed_person(&conn).await;
        seed_tag(&conn).await;
        conn.execute(
            "INSERT INTO score_people (score_id, person_id) VALUES (1, 1)",
            (),
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO score_tags (score_id, tag_id) VALUES (1, 1)",
            (),
        )
        .await
        .unwrap();

        conn.execute("DELETE FROM scores WHERE id = 1", ())
            .await
            .unwrap();

        let mut rows = conn
            .query("SELECT COUNT(*) FROM score_people", ())
            .await
            .unwrap();
        let row = rows.next().await.unwrap().unwrap();
        assert_eq!(row.get::<i64>(0).unwrap(), 0);

        let mut rows = conn
            .query("SELECT COUNT(*) FROM score_tags", ())
            .await
            .unwrap();
        let row = rows.next().await.unwrap().unwrap();
        assert_eq!(row.get::<i64>(0).unwrap(), 0);
    }

    #[tokio::test]
    async fn deleting_a_person_cascades_to_person_words_and_score_people() {
        let conn = migrated_conn().await;
        seed_score(&conn).await;
        seed_person(&conn).await;
        conn.execute(
            "INSERT INTO score_people (score_id, person_id) VALUES (1, 1)",
            (),
        )
        .await
        .unwrap();

        conn.execute("DELETE FROM people WHERE id = 1", ())
            .await
            .unwrap();

        let mut rows = conn
            .query("SELECT COUNT(*) FROM person_words", ())
            .await
            .unwrap();
        let row = rows.next().await.unwrap().unwrap();
        assert_eq!(row.get::<i64>(0).unwrap(), 0);

        let mut rows = conn
            .query("SELECT COUNT(*) FROM score_people", ())
            .await
            .unwrap();
        let row = rows.next().await.unwrap().unwrap();
        assert_eq!(row.get::<i64>(0).unwrap(), 0);
    }

    #[tokio::test]
    async fn deleting_a_tag_cascades_to_score_tags() {
        let conn = migrated_conn().await;
        seed_score(&conn).await;
        seed_tag(&conn).await;
        conn.execute(
            "INSERT INTO score_tags (score_id, tag_id) VALUES (1, 1)",
            (),
        )
        .await
        .unwrap();

        conn.execute("DELETE FROM tags WHERE id = 1", ())
            .await
            .unwrap();

        let mut rows = conn
            .query("SELECT COUNT(*) FROM score_tags", ())
            .await
            .unwrap();
        let row = rows.next().await.unwrap().unwrap();
        assert_eq!(row.get::<i64>(0).unwrap(), 0);
    }

    #[tokio::test]
    async fn only_one_person_allowed_per_display_name_key_case_insensitively() {
        let conn = migrated_conn().await;
        conn.execute(
            "INSERT INTO people (id, display_name, display_name_key)
             VALUES (1, 'Johann Bach', 'johann bach')",
            (),
        )
        .await
        .unwrap();

        let result = conn
            .execute(
                "INSERT INTO people (id, display_name, display_name_key)
                 VALUES (2, 'JOHANN BACH', 'johann bach')",
                (),
            )
            .await;
        assert!(result.is_err());
    }

    /// The whole reason `display_name_key` (case-folded, diacritics
    /// preserved) exists instead of a diacritic-stripped uniqueness key:
    /// two people whose names differ only by diacritics must both be
    /// insertable, even though search (`person_words`, tokenized from a
    /// diacritic-stripped normalization computed at write time - never
    /// persisted on `people` itself) intentionally treats them as
    /// equivalent.
    #[tokio::test]
    async fn people_with_different_diacritics_are_not_treated_as_duplicates() {
        let conn = migrated_conn().await;
        conn.execute(
            "INSERT INTO people (id, display_name, display_name_key)
             VALUES (1, 'Marten Roots', 'marten roots')",
            (),
        )
        .await
        .unwrap();

        conn.execute(
            "INSERT INTO people (id, display_name, display_name_key)
             VALUES (2, 'Märten Roots', 'märten roots')",
            (),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn only_one_tag_allowed_per_name() {
        let conn = migrated_conn().await;
        conn.execute(
            "INSERT INTO tags (id, name, name_normalized) VALUES (1, 'key', 'key')",
            (),
        )
        .await
        .unwrap();

        let result = conn
            .execute(
                "INSERT INTO tags (id, name, name_normalized) VALUES (2, 'Key', 'key')",
                (),
            )
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn same_tag_attached_to_different_scores_with_different_values_is_allowed() {
        let conn = migrated_conn().await;
        conn.execute(
            "INSERT INTO scores (id, created_at) VALUES (1, '2026-01-01T00:00:00Z'), (2, '2026-01-01T00:00:00Z')",
            (),
        )
        .await
        .unwrap();
        seed_tag(&conn).await;

        conn.execute(
            "INSERT INTO score_tags (score_id, tag_id, value, value_normalized)
             VALUES (1, 1, 'CMajor', 'cmajor')",
            (),
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO score_tags (score_id, tag_id, value, value_normalized)
             VALUES (2, 1, 'DMinor', 'dminor')",
            (),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn attaching_the_exact_same_tag_value_to_a_score_twice_is_rejected() {
        let conn = migrated_conn().await;
        seed_score(&conn).await;
        seed_tag(&conn).await;
        conn.execute(
            "INSERT INTO score_tags (score_id, tag_id, value, value_normalized)
             VALUES (1, 1, 'CMajor', 'cmajor')",
            (),
        )
        .await
        .unwrap();

        let result = conn
            .execute(
                "INSERT INTO score_tags (score_id, tag_id, value, value_normalized)
                 VALUES (1, 1, 'CMajor', 'cmajor')",
                (),
            )
            .await;
        assert!(result.is_err());
    }

    /// A score may carry the same tag with several different values at
    /// once (e.g. `#laulupidu:2020` and `#laulupidu:2025` on the same
    /// score) - the whole point of moving uniqueness from
    /// `(score_id, tag_id)` to `(score_id, tag_id, value)`.
    #[tokio::test]
    async fn attaching_different_values_of_the_same_tag_to_the_same_score_is_allowed() {
        let conn = migrated_conn().await;
        seed_score(&conn).await;
        seed_tag(&conn).await;
        conn.execute(
            "INSERT INTO score_tags (score_id, tag_id, value, value_normalized)
             VALUES (1, 1, 'CMajor', 'cmajor')",
            (),
        )
        .await
        .unwrap();

        conn.execute(
            "INSERT INTO score_tags (score_id, tag_id, value, value_normalized)
             VALUES (1, 1, 'DMinor', 'dminor')",
            (),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn attaching_the_same_tag_without_a_value_to_a_score_twice_is_rejected() {
        let conn = migrated_conn().await;
        seed_score(&conn).await;
        seed_tag(&conn).await;
        conn.execute(
            "INSERT INTO score_tags (score_id, tag_id) VALUES (1, 1)",
            (),
        )
        .await
        .unwrap();

        let result = conn
            .execute(
                "INSERT INTO score_tags (score_id, tag_id) VALUES (1, 1)",
                (),
            )
            .await;
        assert!(result.is_err());
    }
}
