//! Score-level modification methods: creating, describing, and deleting a
//! score as a whole (as opposed to its titles - see `title.rs`).

use super::schema::Scores;
use super::title::{Title, insert_title};
use super::{Catalogue, DatabaseError, ScoreId, params, query_one};
use sea_query::{Expr, ExprTrait, Query, SqliteQueryBuilder};

/// Errors from [`Catalogue::set_description`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SetDescriptionError {
    #[error("score {0} does not exist")]
    ScoreNotFound(ScoreId),
    #[error(transparent)]
    Db(#[from] DatabaseError),
}

impl From<turso::Error> for SetDescriptionError {
    fn from(error: turso::Error) -> Self {
        Self::Db(DatabaseError::from(error))
    }
}

/// Errors from [`Catalogue::delete_score`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DeleteScoreError {
    #[error("score {0} does not exist")]
    ScoreNotFound(ScoreId),
    #[error(transparent)]
    Db(#[from] DatabaseError),
}

impl From<turso::Error> for DeleteScoreError {
    fn from(error: turso::Error) -> Self {
        Self::Db(DatabaseError::from(error))
    }
}

impl Catalogue {
    /// Creates a new score with `title` as its primary (and, at creation
    /// time, only) title. A score can never exist without a title - see
    /// `remove_title` in `title.rs`, which enforces this for the rest of
    /// the score's lifetime too.
    ///
    /// # Errors
    ///
    /// Returns [`DatabaseError`] on an underlying database failure.
    pub async fn create_score(&self, title: Title) -> Result<ScoreId, DatabaseError> {
        let mut conn = self.connect().await?;
        let tx = conn.transaction().await?;

        let created_at = chrono::Utc::now().to_rfc3339();
        let (sql, values) = Query::insert()
            .into_table(Scores::Table)
            .columns([Scores::CreatedAt])
            .values_panic([created_at.into()])
            .build(SqliteQueryBuilder);
        tx.execute(&sql, params::to_turso_values(values)).await?;
        let score_id = ScoreId(tx.last_insert_rowid());

        insert_title(&tx, score_id, &title, true).await?;

        tx.commit().await?;
        tracing::info!(%score_id, %title, "created score");
        Ok(score_id)
    }

    /// Sets (or clears, with `None`) a score's description.
    ///
    /// # Errors
    ///
    /// Returns [`SetDescriptionError::ScoreNotFound`] if `score_id` doesn't
    /// exist, or [`SetDescriptionError::Db`] on an underlying database
    /// failure.
    pub async fn set_description(
        &self,
        score_id: ScoreId,
        description: Option<String>,
    ) -> Result<ScoreId, SetDescriptionError> {
        let mut conn = self.connect().await?;
        let tx = conn.transaction().await?;

        if !score_exists(&tx, score_id).await? {
            return Err(SetDescriptionError::ScoreNotFound(score_id));
        }

        let has_description = description.is_some();
        let (sql, values) = Query::update()
            .table(Scores::Table)
            .value(Scores::Description, description)
            .and_where(Expr::col(Scores::Id).eq(score_id.0))
            .build(SqliteQueryBuilder);
        tx.execute(&sql, params::to_turso_values(values)).await?;

        tx.commit().await?;
        tracing::info!(%score_id, has_description, "set score description");
        Ok(score_id)
    }

    /// Deletes a score and everything that belongs only to it (its
    /// titles, and thence their `title_words`) via `ON DELETE CASCADE`.
    ///
    /// # Errors
    ///
    /// Returns [`DeleteScoreError::ScoreNotFound`] if `score_id` doesn't
    /// exist, or [`DeleteScoreError::Db`] on an underlying database
    /// failure.
    pub async fn delete_score(&self, score_id: ScoreId) -> Result<ScoreId, DeleteScoreError> {
        let mut conn = self.connect().await?;
        let tx = conn.transaction().await?;

        if !score_exists(&tx, score_id).await? {
            return Err(DeleteScoreError::ScoreNotFound(score_id));
        }

        let (sql, values) = Query::delete()
            .from_table(Scores::Table)
            .and_where(Expr::col(Scores::Id).eq(score_id.0))
            .build(SqliteQueryBuilder);
        tx.execute(&sql, params::to_turso_values(values)).await?;

        tx.commit().await?;
        tracing::info!(%score_id, "deleted score");
        Ok(score_id)
    }
}

/// `true` if `score_id` exists. Used as an explicit pre-check for domain
/// "not found" errors rather than relying on `turso::Error::Constraint`'s
/// message text, which doesn't distinguish FK/unique/check violations from
/// each other. Shared with `title.rs`'s `add_title`, which needs the same
/// check before inserting a title against a `score_id`.
pub(super) async fn score_exists(
    tx: &turso::transaction::Transaction<'_>,
    score_id: ScoreId,
) -> Result<bool, DatabaseError> {
    let (sql, values) = Query::select()
        .expr(Expr::val(1))
        .from(Scores::Table)
        .and_where(Expr::col(Scores::Id).eq(score_id.0))
        .build(SqliteQueryBuilder);
    let mut rows = tx.query(&sql, params::to_turso_values(values)).await?;
    Ok(query_one(&mut rows).await?.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalogue::migration;

    async fn test_catalogue() -> Catalogue {
        let database = turso::Builder::new_local(":memory:").build().await.unwrap();
        let mut conn = database.connect().unwrap();
        conn.execute("PRAGMA foreign_keys = ON", ()).await.unwrap();
        migration::run(&mut conn, migration::ALL).await.unwrap();
        Catalogue { database }
    }

    fn title(value: &str) -> Title {
        value.parse().unwrap()
    }

    #[tokio::test]
    async fn create_score_inserts_score_and_primary_title() {
        let catalogue = test_catalogue().await;
        let score_id = catalogue.create_score(title("Ave Maria")).await.unwrap();

        let conn = catalogue.connect().await.unwrap();
        let mut rows = conn
            .query(
                "SELECT value, is_primary FROM titles WHERE score_id = ?",
                (score_id.0,),
            )
            .await
            .unwrap();
        let row = rows.next().await.unwrap().unwrap();
        assert_eq!(row.get::<String>(0).unwrap(), "Ave Maria");
        assert_eq!(row.get::<i64>(1).unwrap(), 1);
    }

    #[tokio::test]
    async fn create_score_indexes_title_words() {
        let catalogue = test_catalogue().await;
        catalogue.create_score(title("Ave Maria")).await.unwrap();

        let conn = catalogue.connect().await.unwrap();
        let mut rows = conn
            .query("SELECT word FROM title_words", ())
            .await
            .unwrap();
        let mut found = Vec::new();
        while let Some(row) = rows.next().await.unwrap() {
            found.push(row.get::<String>(0).unwrap());
        }
        found.sort();
        assert_eq!(found, vec!["ave", "maria"]);
    }

    #[tokio::test]
    async fn set_description_updates_existing_score() {
        let catalogue = test_catalogue().await;
        let score_id = catalogue.create_score(title("Ave Maria")).await.unwrap();
        catalogue
            .set_description(score_id, Some("updated".to_string()))
            .await
            .unwrap();

        let conn = catalogue.connect().await.unwrap();
        let mut rows = conn
            .query("SELECT description FROM scores WHERE id = ?", (score_id.0,))
            .await
            .unwrap();
        let row = rows.next().await.unwrap().unwrap();
        assert_eq!(row.get::<String>(0).unwrap(), "updated");
    }

    #[tokio::test]
    async fn set_description_rejects_missing_score() {
        let catalogue = test_catalogue().await;
        assert_eq!(
            catalogue
                .set_description(ScoreId(1), None)
                .await
                .unwrap_err(),
            SetDescriptionError::ScoreNotFound(ScoreId(1))
        );
    }

    #[tokio::test]
    async fn delete_score_cascades_to_titles_and_title_words() {
        let catalogue = test_catalogue().await;
        let score_id = catalogue.create_score(title("Ave Maria")).await.unwrap();
        catalogue.delete_score(score_id).await.unwrap();

        let conn = catalogue.connect().await.unwrap();
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
    async fn delete_score_rejects_missing_score() {
        let catalogue = test_catalogue().await;
        assert_eq!(
            catalogue.delete_score(ScoreId(1)).await.unwrap_err(),
            DeleteScoreError::ScoreNotFound(ScoreId(1))
        );
    }
}
