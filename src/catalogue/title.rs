//! Title-level modification methods, and the [`Title`] value type shared
//! by every method that accepts a title string.

use super::normalize::{normalize_text, words};
use super::schema::{TitleWords, Titles};
use super::score::score_exists;
use super::{Catalogue, DatabaseError, ScoreId, TitleId, params, query_one};
use sea_query::{Expr, ExprTrait, OnConflict, Query, SqliteQueryBuilder};
use std::fmt;
use std::ops::Deref;
use std::str::FromStr;

/// A validated, trimmed, non-empty title string.
///
/// This is the only place a title's emptiness is ever checked -
/// [`Catalogue::create_score`], [`Catalogue::add_title`], and
/// [`Catalogue::update_title_value`] all take a `Title` instead of a raw
/// `String`, so an empty title can never reach them ("parse, don't
/// validate" - mirrors [`crate::query::PersonName`]/
/// [`crate::query::TagItem`]'s existing pattern).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Title(String);

/// Returned by [`Title::from_str`] if the value is empty after trimming.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("title cannot be empty")]
pub struct EmptyTitleError;

impl FromStr for Title {
    type Err = EmptyTitleError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(EmptyTitleError);
        }
        Ok(Self(trimmed.to_string()))
    }
}

impl Deref for Title {
    type Target = str;

    fn deref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Title {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Errors from [`Catalogue::add_title`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AddTitleError {
    #[error("score {0} does not exist")]
    ScoreNotFound(ScoreId),
    #[error(transparent)]
    Unexpected(#[from] DatabaseError),
}

impl From<turso::Error> for AddTitleError {
    fn from(error: turso::Error) -> Self {
        Self::Unexpected(DatabaseError::from(error))
    }
}

/// Errors from [`Catalogue::set_primary_title`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SetPrimaryTitleError {
    #[error("title {0} does not exist")]
    TitleNotFound(TitleId),
    #[error(transparent)]
    Unexpected(#[from] DatabaseError),
}

impl From<turso::Error> for SetPrimaryTitleError {
    fn from(error: turso::Error) -> Self {
        Self::Unexpected(DatabaseError::from(error))
    }
}

/// Errors from [`Catalogue::update_title_value`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UpdateTitleValueError {
    #[error("title {0} does not exist")]
    TitleNotFound(TitleId),
    #[error(transparent)]
    Unexpected(#[from] DatabaseError),
}

impl From<turso::Error> for UpdateTitleValueError {
    fn from(error: turso::Error) -> Self {
        Self::Unexpected(DatabaseError::from(error))
    }
}

/// Errors from [`Catalogue::remove_title`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RemoveTitleError {
    #[error("title {0} does not exist")]
    TitleNotFound(TitleId),
    #[error("cannot remove score {0}'s only remaining title")]
    CannotRemoveLastTitle(ScoreId),
    #[error("cannot remove score {0}'s primary title; set a different primary title first")]
    CannotRemovePrimaryTitle(ScoreId),
    #[error(transparent)]
    Unexpected(#[from] DatabaseError),
}

impl From<turso::Error> for RemoveTitleError {
    fn from(error: turso::Error) -> Self {
        Self::Unexpected(DatabaseError::from(error))
    }
}

impl Catalogue {
    /// Adds an alternate (non-primary) title to an existing score.
    ///
    /// # Errors
    ///
    /// Returns [`AddTitleError::ScoreNotFound`] if `score_id` doesn't
    /// exist, or [`AddTitleError::Unexpected`] on an underlying database failure.
    pub async fn add_title(
        &self,
        score_id: ScoreId,
        value: Title,
    ) -> Result<TitleId, AddTitleError> {
        let mut conn = self.connect().await?;
        let tx = conn.transaction().await?;

        // Explicit pre-check rather than relying on the `titles.score_id`
        // FK to reject this via `turso::Error::Constraint` - that variant
        // doesn't distinguish which constraint fired, only a message
        // string.
        if !score_exists(&tx, score_id).await? {
            return Err(AddTitleError::ScoreNotFound(score_id));
        }

        let title_id = insert_title(&tx, score_id, &value, false).await?;

        tx.commit().await?;
        tracing::info!(%score_id, %title_id, %value, "added title");
        Ok(title_id)
    }

    /// Makes `title_id` its score's primary title, demoting whichever
    /// title was previously primary. A title's own foreign key already
    /// determines its score, so there's no separate `score_id` parameter
    /// to keep in sync.
    ///
    /// # Errors
    ///
    /// Returns [`SetPrimaryTitleError::TitleNotFound`] if `title_id`
    /// doesn't exist, or [`SetPrimaryTitleError::Unexpected`] on an underlying
    /// database failure.
    pub async fn set_primary_title(
        &self,
        title_id: TitleId,
    ) -> Result<ScoreId, SetPrimaryTitleError> {
        let mut conn = self.connect().await?;
        let tx = conn.transaction().await?;

        let Some((score_id, _)) = title_lookup(&tx, title_id).await? else {
            return Err(SetPrimaryTitleError::TitleNotFound(title_id));
        };

        // Clear the old primary *first*: doing this in the other order
        // (or in one statement) would transiently have two primary titles
        // for the same score, tripping the partial unique index
        // `titles_primary_idx`.
        let (sql, values) = Query::update()
            .table(Titles::Table)
            .value(Titles::IsPrimary, 0)
            .and_where(Expr::col(Titles::ScoreId).eq(score_id.0))
            .and_where(Expr::col(Titles::IsPrimary).eq(1))
            .build(SqliteQueryBuilder);
        tx.execute(&sql, params::to_turso_values(values)).await?;

        let (sql, values) = Query::update()
            .table(Titles::Table)
            .value(Titles::IsPrimary, 1)
            .and_where(Expr::col(Titles::Id).eq(title_id.0))
            .build(SqliteQueryBuilder);
        tx.execute(&sql, params::to_turso_values(values)).await?;

        tx.commit().await?;
        tracing::info!(%score_id, %title_id, "set primary title");
        Ok(score_id)
    }

    /// Changes an existing title's text, recomputing its normalized value
    /// and `title_words` entries.
    ///
    /// # Errors
    ///
    /// Returns [`UpdateTitleValueError::TitleNotFound`] if `title_id`
    /// doesn't exist, or [`UpdateTitleValueError::Unexpected`] on an underlying
    /// database failure.
    pub async fn update_title_value(
        &self,
        title_id: TitleId,
        value: Title,
    ) -> Result<TitleId, UpdateTitleValueError> {
        let mut conn = self.connect().await?;
        let tx = conn.transaction().await?;

        if title_lookup(&tx, title_id).await?.is_none() {
            return Err(UpdateTitleValueError::TitleNotFound(title_id));
        }

        let normalized = normalize_text(&value);
        let (sql, values) = Query::update()
            .table(Titles::Table)
            .value(Titles::Value, value.to_string())
            .value(Titles::ValueNormalized, normalized.clone())
            .and_where(Expr::col(Titles::Id).eq(title_id.0))
            .build(SqliteQueryBuilder);
        tx.execute(&sql, params::to_turso_values(values)).await?;

        // Simpler and less error-prone to fully re-derive `title_words`
        // than to diff the old and new word sets.
        let (sql, values) = Query::delete()
            .from_table(TitleWords::Table)
            .and_where(Expr::col(TitleWords::TitleId).eq(title_id.0))
            .build(SqliteQueryBuilder);
        tx.execute(&sql, params::to_turso_values(values)).await?;
        insert_title_words(&tx, title_id, &normalized).await?;

        tx.commit().await?;
        tracing::info!(%title_id, value = %value, "updated title value");
        Ok(title_id)
    }

    /// Removes a title. Rejected if it's the score's only remaining title
    /// (a score can never have zero titles) or if it's currently the
    /// primary title (a different title must be made primary first via
    /// [`Catalogue::set_primary_title`] - this method never auto-promotes
    /// a replacement).
    ///
    /// # Errors
    ///
    /// Returns [`RemoveTitleError::TitleNotFound`] if `title_id` doesn't
    /// exist, [`RemoveTitleError::CannotRemoveLastTitle`] or
    /// [`RemoveTitleError::CannotRemovePrimaryTitle`] per the rules above,
    /// or [`RemoveTitleError::Unexpected`] on an underlying database failure.
    ///
    /// # Panics
    ///
    /// Never in practice: `COUNT(*)` always returns exactly one row.
    pub async fn remove_title(&self, title_id: TitleId) -> Result<ScoreId, RemoveTitleError> {
        let mut conn = self.connect().await?;
        let tx = conn.transaction().await?;

        let Some((score_id, is_primary)) = title_lookup(&tx, title_id).await? else {
            return Err(RemoveTitleError::TitleNotFound(title_id));
        };

        if title_count(&tx, score_id).await? <= 1 {
            return Err(RemoveTitleError::CannotRemoveLastTitle(score_id));
        }
        if is_primary {
            return Err(RemoveTitleError::CannotRemovePrimaryTitle(score_id));
        }

        let (sql, values) = Query::delete()
            .from_table(Titles::Table)
            .and_where(Expr::col(Titles::Id).eq(title_id.0))
            .build(SqliteQueryBuilder);
        tx.execute(&sql, params::to_turso_values(values)).await?;

        tx.commit().await?;
        tracing::info!(%score_id, %title_id, "removed title");
        Ok(score_id)
    }
}

/// Inserts a new `titles` row (and its `title_words`) and returns its id.
/// Shared by [`Catalogue::create_score`] (which needs a primary title) and
/// [`Catalogue::add_title`] (an alternate one).
pub(super) async fn insert_title(
    tx: &turso::transaction::Transaction<'_>,
    score_id: ScoreId,
    value: &Title,
    is_primary: bool,
) -> Result<TitleId, DatabaseError> {
    let normalized = normalize_text(value);
    let (sql, values) = Query::insert()
        .into_table(Titles::Table)
        .columns([
            Titles::ScoreId,
            Titles::Value,
            Titles::ValueNormalized,
            Titles::IsPrimary,
        ])
        .values_panic([
            score_id.0.into(),
            value.to_string().into(),
            normalized.clone().into(),
            i32::from(is_primary).into(),
        ])
        .build(SqliteQueryBuilder);
    tx.execute(&sql, params::to_turso_values(values)).await?;
    let title_id = TitleId(tx.last_insert_rowid());

    insert_title_words(tx, title_id, &normalized).await?;
    Ok(title_id)
}

/// Inserts one `title_words` row per distinct word in `normalized`,
/// ignoring words already indexed for this title.
async fn insert_title_words(
    tx: &turso::transaction::Transaction<'_>,
    title_id: TitleId,
    normalized: &str,
) -> Result<(), DatabaseError> {
    for word in words(normalized) {
        let (sql, values) = Query::insert()
            .into_table(TitleWords::Table)
            .columns([TitleWords::TitleId, TitleWords::Word])
            .values_panic([title_id.0.into(), word.into()])
            .on_conflict(OnConflict::new().do_nothing().to_owned())
            .build(SqliteQueryBuilder);
        tx.execute(&sql, params::to_turso_values(values)).await?;
    }
    Ok(())
}

/// `Some((score_id, is_primary))` if `title_id` exists, `None` otherwise.
async fn title_lookup(
    tx: &turso::transaction::Transaction<'_>,
    title_id: TitleId,
) -> Result<Option<(ScoreId, bool)>, DatabaseError> {
    let (sql, values) = Query::select()
        .columns([Titles::ScoreId, Titles::IsPrimary])
        .from(Titles::Table)
        .and_where(Expr::col(Titles::Id).eq(title_id.0))
        .build(SqliteQueryBuilder);
    let mut rows = tx.query(&sql, params::to_turso_values(values)).await?;
    let Some(row) = query_one(&mut rows).await? else {
        return Ok(None);
    };
    let score_id = ScoreId(row.get::<i64>(0)?);
    let is_primary = row.get::<i64>(1)? != 0;
    Ok(Some((score_id, is_primary)))
}

/// How many titles `score_id` currently has.
async fn title_count(
    tx: &turso::transaction::Transaction<'_>,
    score_id: ScoreId,
) -> Result<i64, DatabaseError> {
    let (sql, values) = Query::select()
        .expr(Expr::col(Titles::Id).count())
        .from(Titles::Table)
        .and_where(Expr::col(Titles::ScoreId).eq(score_id.0))
        .build(SqliteQueryBuilder);
    let mut rows = tx.query(&sql, params::to_turso_values(values)).await?;
    let row = query_one(&mut rows)
        .await?
        .expect("COUNT always returns one row");
    Ok(row.get::<i64>(0)?)
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

    async fn is_primary(catalogue: &Catalogue, title_id: TitleId) -> bool {
        let conn = catalogue.connect().await.unwrap();
        let mut rows = conn
            .query("SELECT is_primary FROM titles WHERE id = ?", (title_id.0,))
            .await
            .unwrap();
        rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap() != 0
    }

    #[tokio::test]
    async fn add_title_creates_alternate_title() {
        let catalogue = test_catalogue().await;
        let score_id = catalogue.create_score(title("Ave Maria")).await.unwrap();
        let title_id = catalogue
            .add_title(score_id, title("Ave Maria (alt)"))
            .await
            .unwrap();

        assert!(!is_primary(&catalogue, title_id).await);
    }

    #[tokio::test]
    async fn add_title_rejects_missing_score() {
        let catalogue = test_catalogue().await;
        assert_eq!(
            catalogue
                .add_title(ScoreId(1), title("Anything"))
                .await
                .unwrap_err(),
            AddTitleError::ScoreNotFound(ScoreId(1))
        );
    }

    #[test]
    fn title_rejects_empty_value() {
        assert_eq!("  ".parse::<Title>().unwrap_err(), EmptyTitleError);
    }

    #[tokio::test]
    async fn set_primary_title_swaps_primary_flag() {
        let catalogue = test_catalogue().await;
        let score_id = catalogue.create_score(title("Ave Maria")).await.unwrap();
        let alt_id = catalogue
            .add_title(score_id, title("Ave Maria (alt)"))
            .await
            .unwrap();

        catalogue.set_primary_title(alt_id).await.unwrap();

        assert!(is_primary(&catalogue, alt_id).await);
    }

    #[tokio::test]
    async fn set_primary_title_rejects_missing_title() {
        let catalogue = test_catalogue().await;
        assert_eq!(
            catalogue.set_primary_title(TitleId(1)).await.unwrap_err(),
            SetPrimaryTitleError::TitleNotFound(TitleId(1))
        );
    }

    #[tokio::test]
    async fn update_title_value_recomputes_title_words() {
        let catalogue = test_catalogue().await;
        let score_id = catalogue.create_score(title("Ave Maria")).await.unwrap();
        let conn = catalogue.connect().await.unwrap();
        let mut rows = conn
            .query("SELECT id FROM titles WHERE score_id = ?", (score_id.0,))
            .await
            .unwrap();
        let title_id = TitleId(rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap());
        drop(rows);

        catalogue
            .update_title_value(title_id, title("Total Praise"))
            .await
            .unwrap();

        let mut rows = conn
            .query("SELECT word FROM title_words", ())
            .await
            .unwrap();
        let mut found = Vec::new();
        while let Some(row) = rows.next().await.unwrap() {
            found.push(row.get::<String>(0).unwrap());
        }
        found.sort();
        assert_eq!(found, vec!["praise", "total"]);
    }

    #[tokio::test]
    async fn update_title_value_rejects_missing_title() {
        let catalogue = test_catalogue().await;
        assert_eq!(
            catalogue
                .update_title_value(TitleId(1), title("Anything"))
                .await
                .unwrap_err(),
            UpdateTitleValueError::TitleNotFound(TitleId(1))
        );
    }

    #[tokio::test]
    async fn remove_title_rejects_last_title() {
        let catalogue = test_catalogue().await;
        let score_id = catalogue.create_score(title("Ave Maria")).await.unwrap();
        let conn = catalogue.connect().await.unwrap();
        let mut rows = conn
            .query("SELECT id FROM titles WHERE score_id = ?", (score_id.0,))
            .await
            .unwrap();
        let title_id = TitleId(rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap());

        assert_eq!(
            catalogue.remove_title(title_id).await.unwrap_err(),
            RemoveTitleError::CannotRemoveLastTitle(score_id)
        );
    }

    #[tokio::test]
    async fn remove_title_rejects_removing_primary_title_when_alternates_exist() {
        let catalogue = test_catalogue().await;
        let score_id = catalogue.create_score(title("Ave Maria")).await.unwrap();
        catalogue
            .add_title(score_id, title("Ave Maria (alt)"))
            .await
            .unwrap();

        let conn = catalogue.connect().await.unwrap();
        let mut rows = conn
            .query(
                "SELECT id FROM titles WHERE score_id = ? AND is_primary = 1",
                (score_id.0,),
            )
            .await
            .unwrap();
        let primary_id = TitleId(rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap());

        assert_eq!(
            catalogue.remove_title(primary_id).await.unwrap_err(),
            RemoveTitleError::CannotRemovePrimaryTitle(score_id)
        );
    }

    #[tokio::test]
    async fn remove_title_deletes_non_primary_title() {
        let catalogue = test_catalogue().await;
        let score_id = catalogue.create_score(title("Ave Maria")).await.unwrap();
        let alt_id = catalogue
            .add_title(score_id, title("Ave Maria (alt)"))
            .await
            .unwrap();

        catalogue.remove_title(alt_id).await.unwrap();

        let conn = catalogue.connect().await.unwrap();
        let mut rows = conn
            .query(
                "SELECT COUNT(*) FROM titles WHERE score_id = ?",
                (score_id.0,),
            )
            .await
            .unwrap();
        let row = rows.next().await.unwrap().unwrap();
        assert_eq!(row.get::<i64>(0).unwrap(), 1);
    }

    #[tokio::test]
    async fn remove_title_rejects_missing_title() {
        let catalogue = test_catalogue().await;
        assert_eq!(
            catalogue.remove_title(TitleId(1)).await.unwrap_err(),
            RemoveTitleError::TitleNotFound(TitleId(1))
        );
    }
}
