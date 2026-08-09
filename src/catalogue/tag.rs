//! Tag-level modification methods: creating/deleting a tag, and
//! attaching/detaching one on a score.
//!
//! A tag is just a key (e.g. "difficulty"); its value, if any, is a
//! per-score concept assigned when the tag is attached - not part of the
//! tag's own identity. This lets one score carry `difficulty:5` and
//! another `difficulty:easy` while both still refer to the same
//! "difficulty" tag, and lets a bare `#difficulty` search match any
//! attachment regardless of its value (including none).
//!
//! A single score may also carry the *same* tag with several different
//! values at once (e.g. a score tagged both `#laulupidu:2020` and
//! `#laulupidu:2025`) - so `score_tags` is keyed on the full
//! `(score_id, tag_id, value)` triple, not `(score_id, tag_id)`. Valueless
//! and valued attachments are mutually exclusive for a given `(score,
//! tag)` pair, though: attaching a value replaces a pre-existing valueless
//! attachment rather than coexisting with it (see [`Catalogue::attach_tag`]),
//! and attaching without a value is rejected once any valued attachment
//! exists.

use super::normalize::normalize_text;
use super::schema::{ScoreTags, Tags};
use super::score::score_exists;
use super::{Catalogue, DatabaseError, ScoreId, TagId, params, query_one};
use crate::query::TagItem;
use sea_query::{Expr, ExprTrait, Query, SqliteQueryBuilder};
use std::ops::Deref;

/// Errors from [`Catalogue::create_tag`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CreateTagError {
    #[error("a tag with this name already exists: {0}")]
    AlreadyExists(TagId),
    #[error(transparent)]
    Unexpected(#[from] DatabaseError),
}

impl From<turso::Error> for CreateTagError {
    fn from(error: turso::Error) -> Self {
        Self::Unexpected(DatabaseError::from(error))
    }
}

/// Errors from [`Catalogue::delete_tag`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DeleteTagError {
    #[error("tag {0} does not exist")]
    TagNotFound(TagId),
    #[error(transparent)]
    Unexpected(#[from] DatabaseError),
}

impl From<turso::Error> for DeleteTagError {
    fn from(error: turso::Error) -> Self {
        Self::Unexpected(DatabaseError::from(error))
    }
}

/// Errors from [`Catalogue::attach_tag`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AttachTagError {
    #[error("score {0} does not exist")]
    ScoreNotFound(ScoreId),
    #[error("tag {0} does not exist")]
    TagNotFound(TagId),
    /// The exact `(score, tag, value)` combination is already attached.
    /// Carries no data: the caller already has `score_id`, `tag_id`, and
    /// `value` at the call site, so echoing them back would be redundant.
    #[error("this tag is already attached to the score with this value")]
    AlreadyAttached,
    /// Attaching without a value was rejected because the score already
    /// has this tag attached with at least one real value - valueless and
    /// valued attachments are mutually exclusive for a given
    /// `(score, tag)` pair.
    #[error("tag {1} is attached to score {0} with a value; a value is required")]
    ValueRequired(ScoreId, TagId),
    #[error(transparent)]
    Unexpected(#[from] DatabaseError),
}

impl From<turso::Error> for AttachTagError {
    fn from(error: turso::Error) -> Self {
        Self::Unexpected(DatabaseError::from(error))
    }
}

/// Errors from [`Catalogue::detach_tag`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DetachTagError {
    #[error("score {0} does not exist")]
    ScoreNotFound(ScoreId),
    #[error("tag {0} does not exist")]
    TagNotFound(TagId),
    #[error("tag {1} with this value is not attached to score {0}")]
    NotAttached(ScoreId, TagId, Option<TagItem>),
    #[error(transparent)]
    Unexpected(#[from] DatabaseError),
}

impl From<turso::Error> for DetachTagError {
    fn from(error: turso::Error) -> Self {
        Self::Unexpected(DatabaseError::from(error))
    }
}

impl Catalogue {
    /// Creates a new tag from a name.
    ///
    /// `tags` is a shared, reusable vocabulary (attached to scores
    /// via `score_tags`'s many-to-many junction), so a name is never
    /// duplicated: if one already exists, this returns its id via
    /// [`CreateTagError::AlreadyExists`] instead of creating a second row.
    ///
    /// # Errors
    ///
    /// Returns [`CreateTagError::AlreadyExists`] if a tag with the same
    /// (normalized) name already exists, or [`CreateTagError::Unexpected`] on an
    /// underlying database failure.
    pub async fn create_tag(&self, name: TagItem) -> Result<TagId, CreateTagError> {
        let mut conn = self.connect().await?;
        let tx = conn.transaction().await?;

        let name_normalized = normalize_text(&name);

        if let Some(existing_id) = tag_lookup_by_name(&tx, &name_normalized).await? {
            return Err(CreateTagError::AlreadyExists(existing_id));
        }

        let (sql, values) = Query::insert()
            .into_table(Tags::Table)
            .columns([Tags::Name, Tags::NameNormalized])
            .values_panic([name.deref().into(), name_normalized.into()])
            .build(SqliteQueryBuilder);
        tx.execute(&sql, params::to_turso_values(values)).await?;
        let tag_id = TagId(tx.last_insert_rowid());

        tx.commit().await?;
        tracing::info!(%tag_id, name = %*name, "created tag");
        Ok(tag_id)
    }

    /// Deletes a tag and its `score_tags` attachments via
    /// `ON DELETE CASCADE`.
    ///
    /// # Errors
    ///
    /// Returns [`DeleteTagError::TagNotFound`] if `tag_id` doesn't exist,
    /// or [`DeleteTagError::Unexpected`] on an underlying database failure.
    pub async fn delete_tag(&self, tag_id: TagId) -> Result<TagId, DeleteTagError> {
        let mut conn = self.connect().await?;
        let tx = conn.transaction().await?;

        if !tag_exists(&tx, tag_id).await? {
            return Err(DeleteTagError::TagNotFound(tag_id));
        }

        let (sql, values) = Query::delete()
            .from_table(Tags::Table)
            .and_where(Expr::col(Tags::Id).eq(tag_id.0))
            .build(SqliteQueryBuilder);
        tx.execute(&sql, params::to_turso_values(values)).await?;

        tx.commit().await?;
        tracing::info!(%tag_id, "deleted tag");
        Ok(tag_id)
    }

    /// Attaches a tag to a score, optionally with a value.
    ///
    /// A score may carry the same tag with several different values at once
    /// (e.g. `#laulupidu:2020` and `#laulupidu:2025` together), so attaching a
    /// new, distinct value never fails just because other values already exist
    /// for this `(score, tag)` pair.
    ///
    /// Valueless and valued attachments are mutually exclusive, though:
    /// attaching *with* a value when a valueless attachment already exists
    /// implicitly replaces it (rather than leaving both), and attaching
    /// *without* a value when any valued attachment already exists is
    /// rejected with [`AttachTagError::ValueRequired`].
    ///
    /// # Errors
    ///
    /// Returns [`AttachTagError::ScoreNotFound`] if `score_id` doesn't
    /// exist, [`AttachTagError::TagNotFound`] if `tag_id` doesn't exist,
    /// [`AttachTagError::AlreadyAttached`] if this exact `(score, tag,
    /// value)` combination is already attached,
    /// [`AttachTagError::ValueRequired`] per the mutual-exclusion rule
    /// above, or [`AttachTagError::Unexpected`] on an underlying database failure.
    pub async fn attach_tag(
        &self,
        score_id: ScoreId,
        tag_id: TagId,
        value: Option<TagItem>,
    ) -> Result<(), AttachTagError> {
        let mut conn = self.connect().await?;
        let tx = conn.transaction().await?;

        if !score_exists(&tx, score_id).await? {
            return Err(AttachTagError::ScoreNotFound(score_id));
        }
        if !tag_exists(&tx, tag_id).await? {
            return Err(AttachTagError::TagNotFound(tag_id));
        }

        let value_normalized = value.as_ref().map(|v| normalize_text(v));

        if value.is_some() {
            if score_tag_row_exists(&tx, score_id, tag_id, value_normalized.as_deref()).await? {
                return Err(AttachTagError::AlreadyAttached);
            }
            // Adding a real value supersedes a pre-existing valueless
            // attachment rather than leaving both - see the module doc.
            if score_tag_row_exists(&tx, score_id, tag_id, None).await? {
                delete_score_tag_row(&tx, score_id, tag_id, None).await?;
            }
        } else {
            if score_tag_has_any_value(&tx, score_id, tag_id).await? {
                return Err(AttachTagError::ValueRequired(score_id, tag_id));
            }
            if score_tag_row_exists(&tx, score_id, tag_id, None).await? {
                return Err(AttachTagError::AlreadyAttached);
            }
        }

        let (sql, values) = Query::insert()
            .into_table(ScoreTags::Table)
            .columns([
                ScoreTags::ScoreId,
                ScoreTags::TagId,
                ScoreTags::Value,
                ScoreTags::ValueNormalized,
            ])
            .values_panic([
                score_id.0.into(),
                tag_id.0.into(),
                value.as_deref().into(),
                value_normalized.into(),
            ])
            .build(SqliteQueryBuilder);
        tx.execute(&sql, params::to_turso_values(values)).await?;

        tx.commit().await?;
        tracing::info!(%score_id, %tag_id, "attached tag to score");
        Ok(())
    }

    /// Detaches a tag with a specific value (or `None` for a valueless
    /// attachment) from a score.
    ///
    /// Since a score can carry the same tag with several different values,
    /// `value` identifies exactly which attachment to remove - the others, if
    /// any, are left in place.
    ///
    /// # Errors
    ///
    /// Returns [`DetachTagError::ScoreNotFound`] if `score_id` doesn't
    /// exist, [`DetachTagError::TagNotFound`] if `tag_id` doesn't exist,
    /// [`DetachTagError::NotAttached`] if no attachment matches `(score_id,
    /// tag_id, value)` exactly, or [`DetachTagError::Unexpected`] on an underlying
    /// database failure.
    pub async fn detach_tag(
        &self,
        score_id: ScoreId,
        tag_id: TagId,
        value: Option<TagItem>,
    ) -> Result<(), DetachTagError> {
        let mut conn = self.connect().await?;
        let tx = conn.transaction().await?;

        if !score_exists(&tx, score_id).await? {
            return Err(DetachTagError::ScoreNotFound(score_id));
        }
        if !tag_exists(&tx, tag_id).await? {
            return Err(DetachTagError::TagNotFound(tag_id));
        }

        let value_normalized = value.as_ref().map(|v| normalize_text(v));
        if !score_tag_row_exists(&tx, score_id, tag_id, value_normalized.as_deref()).await? {
            return Err(DetachTagError::NotAttached(score_id, tag_id, value));
        }

        delete_score_tag_row(&tx, score_id, tag_id, value_normalized.as_deref()).await?;

        tx.commit().await?;
        tracing::info!(%score_id, %tag_id, "detached tag from score");
        Ok(())
    }
}

/// `true` if `tag_id` exists. Used as an explicit pre-check for domain
/// "not found" errors rather than relying on `turso::Error::Constraint`'s
/// message text.
pub(super) async fn tag_exists(
    tx: &turso::transaction::Transaction<'_>,
    tag_id: TagId,
) -> Result<bool, DatabaseError> {
    let (sql, values) = Query::select()
        .expr(Expr::val(1))
        .from(Tags::Table)
        .and_where(Expr::col(Tags::Id).eq(tag_id.0))
        .build(SqliteQueryBuilder);
    let mut rows = tx.query(&sql, params::to_turso_values(values)).await?;
    Ok(query_one(&mut rows).await?.is_some())
}

/// `Some(tag_id)` of the tag whose `name_normalized` equals
/// `name_normalized`, `None` if none exists. Used by
/// [`Catalogue::create_tag`] to enforce the one-tag-per-name invariant
/// explicitly, rather than relying on the unique index's
/// constraint-violation text.
async fn tag_lookup_by_name(
    tx: &turso::transaction::Transaction<'_>,
    name_normalized: &str,
) -> Result<Option<TagId>, DatabaseError> {
    let (sql, values) = Query::select()
        .column(Tags::Id)
        .from(Tags::Table)
        .and_where(Expr::col(Tags::NameNormalized).eq(name_normalized))
        .build(SqliteQueryBuilder);
    let mut rows = tx.query(&sql, params::to_turso_values(values)).await?;
    let Some(row) = query_one(&mut rows).await? else {
        return Ok(None);
    };
    Ok(Some(TagId(row.get::<i64>(0)?)))
}

/// `true` if a `score_tags` row exists for `(score_id, tag_id)` with
/// exactly `value_normalized` (`None` means "no value", matched via `IS
/// NULL`, not a wildcard).
async fn score_tag_row_exists(
    tx: &turso::transaction::Transaction<'_>,
    score_id: ScoreId,
    tag_id: TagId,
    value_normalized: Option<&str>,
) -> Result<bool, DatabaseError> {
    let mut select = Query::select();
    select
        .expr(Expr::val(1))
        .from(ScoreTags::Table)
        .and_where(Expr::col(ScoreTags::ScoreId).eq(score_id.0))
        .and_where(Expr::col(ScoreTags::TagId).eq(tag_id.0));
    match value_normalized {
        Some(value) => {
            select.and_where(Expr::col(ScoreTags::ValueNormalized).eq(value));
        }
        None => {
            select.and_where(Expr::col(ScoreTags::ValueNormalized).is_null());
        }
    }
    let (sql, values) = select.build(SqliteQueryBuilder);
    let mut rows = tx.query(&sql, params::to_turso_values(values)).await?;
    Ok(query_one(&mut rows).await?.is_some())
}

/// `true` if `(score_id, tag_id)` has at least one `score_tags` row with a
/// non-null value. Used to enforce that valueless and valued attachments
/// are mutually exclusive for a given `(score, tag)` pair.
async fn score_tag_has_any_value(
    tx: &turso::transaction::Transaction<'_>,
    score_id: ScoreId,
    tag_id: TagId,
) -> Result<bool, DatabaseError> {
    let (sql, values) = Query::select()
        .expr(Expr::val(1))
        .from(ScoreTags::Table)
        .and_where(Expr::col(ScoreTags::ScoreId).eq(score_id.0))
        .and_where(Expr::col(ScoreTags::TagId).eq(tag_id.0))
        .and_where(Expr::col(ScoreTags::ValueNormalized).is_not_null())
        .build(SqliteQueryBuilder);
    let mut rows = tx.query(&sql, params::to_turso_values(values)).await?;
    Ok(query_one(&mut rows).await?.is_some())
}

/// Deletes the `score_tags` row for `(score_id, tag_id)` with exactly
/// `value_normalized` (`None` means "no value", matched via `IS NULL`).
async fn delete_score_tag_row(
    tx: &turso::transaction::Transaction<'_>,
    score_id: ScoreId,
    tag_id: TagId,
    value_normalized: Option<&str>,
) -> Result<(), DatabaseError> {
    let mut delete = Query::delete();
    delete
        .from_table(ScoreTags::Table)
        .and_where(Expr::col(ScoreTags::ScoreId).eq(score_id.0))
        .and_where(Expr::col(ScoreTags::TagId).eq(tag_id.0));
    match value_normalized {
        Some(value) => {
            delete.and_where(Expr::col(ScoreTags::ValueNormalized).eq(value));
        }
        None => {
            delete.and_where(Expr::col(ScoreTags::ValueNormalized).is_null());
        }
    }
    let (sql, values) = delete.build(SqliteQueryBuilder);
    tx.execute(&sql, params::to_turso_values(values)).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalogue::migration;
    use crate::catalogue::title::Title;

    async fn test_catalogue() -> Catalogue {
        let database = turso::Builder::new_local(":memory:").build().await.unwrap();
        let mut conn = database.connect().unwrap();
        conn.execute("PRAGMA foreign_keys = ON", ()).await.unwrap();
        migration::run(&mut conn, migration::ALL).await.unwrap();
        Catalogue { database }
    }

    fn item(value: &str) -> TagItem {
        TagItem::parse(value).unwrap()
    }

    fn title(value: &str) -> Title {
        value.parse().unwrap()
    }

    async fn value_of(
        catalogue: &Catalogue,
        score_id: ScoreId,
        tag_id: TagId,
    ) -> Vec<Option<String>> {
        let conn = catalogue.connect().await.unwrap();
        let mut rows = conn
            .query(
                "SELECT value FROM score_tags WHERE score_id = ? AND tag_id = ? ORDER BY value",
                (score_id.0, tag_id.0),
            )
            .await
            .unwrap();
        let mut found = Vec::new();
        while let Some(row) = rows.next().await.unwrap() {
            found.push(row.get::<Option<String>>(0).unwrap());
        }
        found
    }

    #[tokio::test]
    async fn create_tag_stores_name() {
        let catalogue = test_catalogue().await;
        let tag_id = catalogue.create_tag(item("key")).await.unwrap();

        let conn = catalogue.connect().await.unwrap();
        let mut rows = conn
            .query(
                "SELECT name, name_normalized FROM tags WHERE id = ?",
                (tag_id.0,),
            )
            .await
            .unwrap();
        let row = rows.next().await.unwrap().unwrap();
        assert_eq!(row.get::<String>(0).unwrap(), "key");
        assert_eq!(row.get::<String>(1).unwrap(), "key");
    }

    #[tokio::test]
    async fn create_tag_rejects_duplicate_name() {
        let catalogue = test_catalogue().await;
        let first_id = catalogue.create_tag(item("key")).await.unwrap();

        assert_eq!(
            catalogue.create_tag(item("key")).await.unwrap_err(),
            CreateTagError::AlreadyExists(first_id)
        );
    }

    #[tokio::test]
    async fn create_tag_rejects_duplicate_name_regardless_of_case() {
        let catalogue = test_catalogue().await;
        let first_id = catalogue.create_tag(item("key")).await.unwrap();

        assert_eq!(
            catalogue.create_tag(item("KEY")).await.unwrap_err(),
            CreateTagError::AlreadyExists(first_id)
        );
    }

    #[tokio::test]
    async fn delete_tag_cascades_to_attachments() {
        let catalogue = test_catalogue().await;
        let tag_id = catalogue.create_tag(item("sacred")).await.unwrap();
        let score_id = catalogue.create_score(title("Ave Maria")).await.unwrap();
        catalogue.attach_tag(score_id, tag_id, None).await.unwrap();

        catalogue.delete_tag(tag_id).await.unwrap();

        let conn = catalogue.connect().await.unwrap();
        let mut rows = conn
            .query("SELECT COUNT(*) FROM score_tags", ())
            .await
            .unwrap();
        assert_eq!(
            rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn delete_tag_rejects_missing_tag() {
        let catalogue = test_catalogue().await;
        assert_eq!(
            catalogue.delete_tag(TagId(1)).await.unwrap_err(),
            DeleteTagError::TagNotFound(TagId(1))
        );
    }

    #[tokio::test]
    async fn attach_tag_stores_value() {
        let catalogue = test_catalogue().await;
        let tag_id = catalogue.create_tag(item("difficulty")).await.unwrap();
        let score_id = catalogue.create_score(title("Ave Maria")).await.unwrap();

        catalogue
            .attach_tag(score_id, tag_id, Some(item("5")))
            .await
            .unwrap();

        assert_eq!(
            value_of(&catalogue, score_id, tag_id).await,
            vec![Some("5".to_string())]
        );
    }

    #[tokio::test]
    async fn attach_tag_without_value_stores_null() {
        let catalogue = test_catalogue().await;
        let tag_id = catalogue.create_tag(item("sacred")).await.unwrap();
        let score_id = catalogue.create_score(title("Ave Maria")).await.unwrap();

        catalogue.attach_tag(score_id, tag_id, None).await.unwrap();

        assert_eq!(value_of(&catalogue, score_id, tag_id).await, vec![None]);
    }

    #[tokio::test]
    async fn attach_tag_rejects_missing_score() {
        let catalogue = test_catalogue().await;
        let tag_id = catalogue.create_tag(item("sacred")).await.unwrap();
        assert_eq!(
            catalogue
                .attach_tag(ScoreId(1), tag_id, None)
                .await
                .unwrap_err(),
            AttachTagError::ScoreNotFound(ScoreId(1))
        );
    }

    #[tokio::test]
    async fn attach_tag_rejects_missing_tag() {
        let catalogue = test_catalogue().await;
        let score_id = catalogue.create_score(title("Ave Maria")).await.unwrap();
        assert_eq!(
            catalogue
                .attach_tag(score_id, TagId(1), None)
                .await
                .unwrap_err(),
            AttachTagError::TagNotFound(TagId(1))
        );
    }

    #[tokio::test]
    async fn attach_tag_rejects_exact_duplicate_value() {
        let catalogue = test_catalogue().await;
        let tag_id = catalogue.create_tag(item("key")).await.unwrap();
        let score_id = catalogue.create_score(title("Ave Maria")).await.unwrap();
        catalogue
            .attach_tag(score_id, tag_id, Some(item("CMajor")))
            .await
            .unwrap();

        assert_eq!(
            catalogue
                .attach_tag(score_id, tag_id, Some(item("CMajor")))
                .await
                .unwrap_err(),
            AttachTagError::AlreadyAttached
        );
    }

    #[tokio::test]
    async fn attach_tag_rejects_duplicate_valueless_attachment() {
        let catalogue = test_catalogue().await;
        let tag_id = catalogue.create_tag(item("sacred")).await.unwrap();
        let score_id = catalogue.create_score(title("Ave Maria")).await.unwrap();
        catalogue.attach_tag(score_id, tag_id, None).await.unwrap();

        assert_eq!(
            catalogue
                .attach_tag(score_id, tag_id, None)
                .await
                .unwrap_err(),
            AttachTagError::AlreadyAttached
        );
    }

    /// The whole point of the change: a score may carry the same tag with
    /// several different values at once.
    #[tokio::test]
    async fn attach_tag_allows_multiple_distinct_values_on_the_same_score() {
        let catalogue = test_catalogue().await;
        let tag_id = catalogue.create_tag(item("laulupidu")).await.unwrap();
        let score_id = catalogue.create_score(title("Ave Maria")).await.unwrap();

        catalogue
            .attach_tag(score_id, tag_id, Some(item("2020")))
            .await
            .unwrap();
        catalogue
            .attach_tag(score_id, tag_id, Some(item("2025")))
            .await
            .unwrap();

        assert_eq!(
            value_of(&catalogue, score_id, tag_id).await,
            vec![Some("2020".to_string()), Some("2025".to_string())]
        );
    }

    /// Valueless and valued attachments are mutually exclusive: adding a
    /// value supersedes a pre-existing valueless attachment rather than
    /// leaving both.
    #[tokio::test]
    async fn attach_tag_with_value_replaces_a_prior_valueless_attachment() {
        let catalogue = test_catalogue().await;
        let tag_id = catalogue.create_tag(item("difficulty")).await.unwrap();
        let score_id = catalogue.create_score(title("Ave Maria")).await.unwrap();
        catalogue.attach_tag(score_id, tag_id, None).await.unwrap();

        catalogue
            .attach_tag(score_id, tag_id, Some(item("5")))
            .await
            .unwrap();

        assert_eq!(
            value_of(&catalogue, score_id, tag_id).await,
            vec![Some("5".to_string())]
        );
    }

    #[tokio::test]
    async fn attach_tag_without_value_rejects_when_a_valued_attachment_exists() {
        let catalogue = test_catalogue().await;
        let tag_id = catalogue.create_tag(item("difficulty")).await.unwrap();
        let score_id = catalogue.create_score(title("Ave Maria")).await.unwrap();
        catalogue
            .attach_tag(score_id, tag_id, Some(item("5")))
            .await
            .unwrap();

        assert_eq!(
            catalogue
                .attach_tag(score_id, tag_id, None)
                .await
                .unwrap_err(),
            AttachTagError::ValueRequired(score_id, tag_id)
        );
    }

    #[tokio::test]
    async fn detach_tag_removes_only_the_matching_value() {
        let catalogue = test_catalogue().await;
        let tag_id = catalogue.create_tag(item("laulupidu")).await.unwrap();
        let score_id = catalogue.create_score(title("Ave Maria")).await.unwrap();
        catalogue
            .attach_tag(score_id, tag_id, Some(item("2020")))
            .await
            .unwrap();
        catalogue
            .attach_tag(score_id, tag_id, Some(item("2025")))
            .await
            .unwrap();

        catalogue
            .detach_tag(score_id, tag_id, Some(item("2020")))
            .await
            .unwrap();

        assert_eq!(
            value_of(&catalogue, score_id, tag_id).await,
            vec![Some("2025".to_string())]
        );
    }

    #[tokio::test]
    async fn detach_tag_removes_valueless_attachment() {
        let catalogue = test_catalogue().await;
        let tag_id = catalogue.create_tag(item("sacred")).await.unwrap();
        let score_id = catalogue.create_score(title("Ave Maria")).await.unwrap();
        catalogue.attach_tag(score_id, tag_id, None).await.unwrap();

        catalogue.detach_tag(score_id, tag_id, None).await.unwrap();

        let conn = catalogue.connect().await.unwrap();
        let mut rows = conn
            .query("SELECT COUNT(*) FROM score_tags", ())
            .await
            .unwrap();
        assert_eq!(
            rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn detach_tag_rejects_not_attached() {
        let catalogue = test_catalogue().await;
        let tag_id = catalogue.create_tag(item("sacred")).await.unwrap();
        let score_id = catalogue.create_score(title("Ave Maria")).await.unwrap();

        assert_eq!(
            catalogue
                .detach_tag(score_id, tag_id, None)
                .await
                .unwrap_err(),
            DetachTagError::NotAttached(score_id, tag_id, None)
        );
    }

    #[tokio::test]
    async fn detach_tag_rejects_mismatched_value() {
        let catalogue = test_catalogue().await;
        let tag_id = catalogue.create_tag(item("key")).await.unwrap();
        let score_id = catalogue.create_score(title("Ave Maria")).await.unwrap();
        catalogue
            .attach_tag(score_id, tag_id, Some(item("CMajor")))
            .await
            .unwrap();

        assert_eq!(
            catalogue
                .detach_tag(score_id, tag_id, Some(item("DMinor")))
                .await
                .unwrap_err(),
            DetachTagError::NotAttached(score_id, tag_id, Some(item("DMinor")))
        );
    }

    #[tokio::test]
    async fn detach_tag_rejects_missing_score() {
        let catalogue = test_catalogue().await;
        let tag_id = catalogue.create_tag(item("sacred")).await.unwrap();
        assert_eq!(
            catalogue
                .detach_tag(ScoreId(1), tag_id, None)
                .await
                .unwrap_err(),
            DetachTagError::ScoreNotFound(ScoreId(1))
        );
    }

    #[tokio::test]
    async fn detach_tag_rejects_missing_tag() {
        let catalogue = test_catalogue().await;
        let score_id = catalogue.create_score(title("Ave Maria")).await.unwrap();
        assert_eq!(
            catalogue
                .detach_tag(score_id, TagId(1), None)
                .await
                .unwrap_err(),
            DetachTagError::TagNotFound(TagId(1))
        );
    }
}
