//! The third schema migration: `roles` and `score_person_roles`, letting
//! each person attached to a score carry any number of roles (composer,
//! arranger, lyricist, ...).

use super::Migration;

/// This migration's 1-based position in [`super::ALL`] - must match both
/// this module's `m0003_` file prefix and its actual index in `ALL`. Only
/// needs to be right once: history is forward-only and immutable, so
/// nothing can ever get inserted before this migration afterward.
#[cfg(test)]
pub(crate) const VERSION: usize = 3;

pub(crate) const MIGRATION: Migration = Migration {
    name: "create roles and score_person_roles tables",
    apply: |conn| {
        Box::pin(async move {
            conn.execute_batch(include_str!("m0003_roles.sql")).await?;
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
    }

    async fn seed_score_people(conn: &Connection) {
        conn.execute(
            "INSERT INTO score_people (score_id, person_id) VALUES (1, 1)",
            (),
        )
        .await
        .unwrap();
    }

    async fn seed_role(conn: &Connection) {
        conn.execute(
            "INSERT INTO roles (id, name, name_key) VALUES (1, 'Composer', 'composer')",
            (),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn creates_roles_and_score_person_roles() {
        let conn = migrated_conn().await;
        seed_score(&conn).await;
        seed_person(&conn).await;
        seed_score_people(&conn).await;
        seed_role(&conn).await;
        conn.execute(
            "INSERT INTO score_person_roles (score_id, person_id, role_id) VALUES (1, 1, 1)",
            (),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn only_one_role_allowed_per_name_key() {
        let conn = migrated_conn().await;
        seed_role(&conn).await;

        let result = conn
            .execute(
                "INSERT INTO roles (id, name, name_key) VALUES (2, 'composer', 'composer')",
                (),
            )
            .await;
        assert!(result.is_err());
    }

    /// `name_key` is case-insensitive but diacritic-sensitive (mirrors
    /// `people.display_name_key`, not `tags.name_normalized`) - two roles
    /// differing only by diacritics must both be insertable.
    #[tokio::test]
    async fn roles_with_different_diacritics_are_not_treated_as_duplicates() {
        let conn = migrated_conn().await;
        conn.execute(
            "INSERT INTO roles (id, name, name_key) VALUES (1, 'arranger', 'arranger')",
            (),
        )
        .await
        .unwrap();

        conn.execute(
            "INSERT INTO roles (id, name, name_key) VALUES (2, 'arränger', 'arränger')",
            (),
        )
        .await
        .unwrap();
    }

    /// Verifies the composite `FOREIGN KEY (score_id, person_id)` against
    /// `score_people` is actually enforced by `turso` - a role can only
    /// attach to a person already attached to the score.
    #[tokio::test]
    async fn score_person_roles_requires_existing_score_people_pair() {
        let conn = migrated_conn().await;
        seed_score(&conn).await;
        seed_person(&conn).await;
        seed_role(&conn).await;
        // Deliberately not seeding `score_people` - (1, 1) is not attached.

        let result = conn
            .execute(
                "INSERT INTO score_person_roles (score_id, person_id, role_id) VALUES (1, 1, 1)",
                (),
            )
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn deleting_a_score_people_row_cascades_to_score_person_roles() {
        let conn = migrated_conn().await;
        seed_score(&conn).await;
        seed_person(&conn).await;
        seed_score_people(&conn).await;
        seed_role(&conn).await;
        conn.execute(
            "INSERT INTO score_person_roles (score_id, person_id, role_id) VALUES (1, 1, 1)",
            (),
        )
        .await
        .unwrap();

        conn.execute(
            "DELETE FROM score_people WHERE score_id = 1 AND person_id = 1",
            (),
        )
        .await
        .unwrap();

        let mut rows = conn
            .query("SELECT COUNT(*) FROM score_person_roles", ())
            .await
            .unwrap();
        let row = rows.next().await.unwrap().unwrap();
        assert_eq!(row.get::<i64>(0).unwrap(), 0);
    }

    #[tokio::test]
    async fn deleting_a_role_cascades_to_score_person_roles() {
        let conn = migrated_conn().await;
        seed_score(&conn).await;
        seed_person(&conn).await;
        seed_score_people(&conn).await;
        seed_role(&conn).await;
        conn.execute(
            "INSERT INTO score_person_roles (score_id, person_id, role_id) VALUES (1, 1, 1)",
            (),
        )
        .await
        .unwrap();

        conn.execute("DELETE FROM roles WHERE id = 1", ())
            .await
            .unwrap();

        let mut rows = conn
            .query("SELECT COUNT(*) FROM score_person_roles", ())
            .await
            .unwrap();
        let row = rows.next().await.unwrap().unwrap();
        assert_eq!(row.get::<i64>(0).unwrap(), 0);
    }

    /// Two-hop cascade: deleting `scores` cascades to `score_people`
    /// (established by `m0002`), which must in turn cascade to
    /// `score_person_roles` via the composite FK.
    #[tokio::test]
    async fn deleting_a_score_cascades_to_score_person_roles() {
        let conn = migrated_conn().await;
        seed_score(&conn).await;
        seed_person(&conn).await;
        seed_score_people(&conn).await;
        seed_role(&conn).await;
        conn.execute(
            "INSERT INTO score_person_roles (score_id, person_id, role_id) VALUES (1, 1, 1)",
            (),
        )
        .await
        .unwrap();

        conn.execute("DELETE FROM scores WHERE id = 1", ())
            .await
            .unwrap();

        let mut rows = conn
            .query("SELECT COUNT(*) FROM score_person_roles", ())
            .await
            .unwrap();
        let row = rows.next().await.unwrap().unwrap();
        assert_eq!(row.get::<i64>(0).unwrap(), 0);
    }

    /// Two-hop cascade: deleting `people` cascades to `score_people`, which
    /// must in turn cascade to `score_person_roles`.
    #[tokio::test]
    async fn deleting_a_person_cascades_to_score_person_roles() {
        let conn = migrated_conn().await;
        seed_score(&conn).await;
        seed_person(&conn).await;
        seed_score_people(&conn).await;
        seed_role(&conn).await;
        conn.execute(
            "INSERT INTO score_person_roles (score_id, person_id, role_id) VALUES (1, 1, 1)",
            (),
        )
        .await
        .unwrap();

        conn.execute("DELETE FROM people WHERE id = 1", ())
            .await
            .unwrap();

        let mut rows = conn
            .query("SELECT COUNT(*) FROM score_person_roles", ())
            .await
            .unwrap();
        let row = rows.next().await.unwrap().unwrap();
        assert_eq!(row.get::<i64>(0).unwrap(), 0);
    }
}
