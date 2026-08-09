//! Score-level modification methods: creating, describing, and deleting a
//! score as a whole (as opposed to its titles - see `title.rs`).

use super::schema::{People, ScorePeople, ScoreTags, Scores, Tags, Titles};
use super::title::{Title, insert_title};
use super::{Catalogue, DatabaseError, PersonId, ScoreId, TagId, TitleId, params, query_one};
use sea_query::{Expr, ExprTrait, Order, Query, SqliteQueryBuilder};
use turso::Connection;

/// Errors from [`Catalogue::set_description`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SetDescriptionError {
    #[error("score {0} does not exist")]
    ScoreNotFound(ScoreId),
    #[error(transparent)]
    Unexpected(#[from] DatabaseError),
}

impl From<turso::Error> for SetDescriptionError {
    fn from(error: turso::Error) -> Self {
        Self::Unexpected(DatabaseError::from(error))
    }
}

/// Errors from [`Catalogue::delete_score`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DeleteScoreError {
    #[error("score {0} does not exist")]
    ScoreNotFound(ScoreId),
    #[error(transparent)]
    Unexpected(#[from] DatabaseError),
}

impl From<turso::Error> for DeleteScoreError {
    fn from(error: turso::Error) -> Self {
        Self::Unexpected(DatabaseError::from(error))
    }
}

/// Full detail for one score - its description, all titles (primary +
/// alternates), and its attached people/tags.
///
/// Returned by [`Catalogue::score_detail`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScoreDetail {
    pub id: ScoreId,
    pub description: Option<String>,
    pub primary_title_id: TitleId,
    pub primary_title: String,
    pub alternate_titles: Vec<(TitleId, String)>,
    pub people: Vec<PersonDetail>,
    pub tags: Vec<TagDetail>,
}

/// One person attached to a score, as returned in [`ScoreDetail::people`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersonDetail {
    pub id: PersonId,
    pub display_name: String,
}

/// One tag attached to a score, as returned in [`ScoreDetail::tags`].
///
/// A score may carry the same tag with several different values at once (see
/// `tag.rs`'s module doc), so the same `name` can appear more than once with a
/// different `value`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagDetail {
    pub id: TagId,
    pub name: String,
    pub value: Option<String>,
}

/// Errors from [`Catalogue::score_description`], [`Catalogue::score_titles`],
/// [`Catalogue::score_people`], [`Catalogue::score_tags`], and
/// [`Catalogue::score_detail`] - identical across all five since each is
/// just "does this score exist" plus a different `SELECT`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ScoreDetailError {
    #[error("score {0} does not exist")]
    ScoreNotFound(ScoreId),
    #[error(transparent)]
    Unexpected(#[from] DatabaseError),
}

impl From<turso::Error> for ScoreDetailError {
    fn from(error: turso::Error) -> Self {
        Self::Unexpected(DatabaseError::from(error))
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
    /// exist, or [`SetDescriptionError::Unexpected`] on an underlying database
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
    /// exist, or [`DeleteScoreError::Unexpected`] on an underlying database
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

    /// A score's description (`None` if it has none set).
    ///
    /// Read-only - doesn't open a transaction.
    ///
    /// # Errors
    ///
    /// Returns [`ScoreDetailError::ScoreNotFound`] if `score_id` doesn't
    /// exist, or [`ScoreDetailError::Unexpected`] on an underlying database
    /// failure.
    pub async fn score_description(
        &self,
        score_id: ScoreId,
    ) -> Result<Option<String>, ScoreDetailError> {
        let conn = self.connect().await?;
        let (sql, values) = Query::select()
            .column(Scores::Description)
            .from(Scores::Table)
            .and_where(Expr::col(Scores::Id).eq(score_id.0))
            .build(SqliteQueryBuilder);
        let mut rows = conn.query(&sql, params::to_turso_values(values)).await?;
        let Some(row) = query_one(&mut rows).await? else {
            return Err(ScoreDetailError::ScoreNotFound(score_id));
        };
        Ok(row.get::<Option<String>>(0)?)
    }

    /// A score's primary title (id + value) and its alternate titles.
    ///
    /// Read-only, doesn't open a transaction.
    ///
    /// # Errors
    ///
    /// Returns [`ScoreDetailError::ScoreNotFound`] if `score_id` doesn't
    /// exist, or [`ScoreDetailError::Unexpected`] on an underlying database
    /// failure.
    pub async fn score_titles(
        &self,
        score_id: ScoreId,
    ) -> Result<(TitleId, String, Vec<(TitleId, String)>), ScoreDetailError> {
        let conn = self.connect().await?;
        let (sql, values) = Query::select()
            .columns([Titles::Id, Titles::Value, Titles::IsPrimary])
            .from(Titles::Table)
            .and_where(Expr::col(Titles::ScoreId).eq(score_id.0))
            .build(SqliteQueryBuilder);
        let mut rows = conn.query(&sql, params::to_turso_values(values)).await?;

        let mut primary = None;
        let mut alternates = Vec::new();
        while let Some(row) = rows.next().await? {
            let title_id = TitleId(row.get::<i64>(0)?);
            let value = row.get::<String>(1)?;
            let is_primary = row.get::<i64>(2)? != 0;
            if is_primary {
                primary = Some((title_id, value));
            } else {
                alternates.push((title_id, value));
            }
        }

        // No titles at all means the score doesn't exist - every existing
        // score has exactly one primary title (title.rs's invariants), so
        // this doubles as the existence check without a separate query.
        let Some((primary_title_id, primary_title)) = primary else {
            return Err(ScoreDetailError::ScoreNotFound(score_id));
        };
        Ok((primary_title_id, primary_title, alternates))
    }

    /// The people attached to a score, ordered by display name.
    ///
    /// Read-only, doesn't open a transaction.
    ///
    /// # Errors
    ///
    /// Returns [`ScoreDetailError::ScoreNotFound`] if `score_id` doesn't
    /// exist, or [`ScoreDetailError::Unexpected`] on an underlying database
    /// failure.
    pub async fn score_people(
        &self,
        score_id: ScoreId,
    ) -> Result<Vec<PersonDetail>, ScoreDetailError> {
        let conn = self.connect().await?;

        // Unlike `score_titles`, an empty result here doesn't imply the
        // score is missing - a score may legitimately have zero attached
        // people - so this needs its own explicit existence check.
        if !score_exists_conn(&conn, score_id).await? {
            return Err(ScoreDetailError::ScoreNotFound(score_id));
        }

        let (sql, values) = Query::select()
            .column((People::Table, People::Id))
            .column((People::Table, People::DisplayName))
            .from(ScorePeople::Table)
            .inner_join(
                People::Table,
                Expr::col((People::Table, People::Id))
                    .equals((ScorePeople::Table, ScorePeople::PersonId)),
            )
            .and_where(Expr::col((ScorePeople::Table, ScorePeople::ScoreId)).eq(score_id.0))
            .order_by((People::Table, People::DisplayName), Order::Asc)
            .build(SqliteQueryBuilder);
        let mut rows = conn.query(&sql, params::to_turso_values(values)).await?;

        let mut people = Vec::new();
        while let Some(row) = rows.next().await? {
            people.push(PersonDetail {
                id: PersonId(row.get::<i64>(0)?),
                display_name: row.get::<String>(1)?,
            });
        }
        Ok(people)
    }

    /// The tags attached to a score, ordered by name then value.
    ///
    /// A single tag name can appear more than once if several distinct values
    /// are attached (see `tag.rs`'s module doc).
    ///
    /// Read-only, same rationale as [`Catalogue::score_description`].
    ///
    /// # Errors
    ///
    /// Returns [`ScoreDetailError::ScoreNotFound`] if `score_id` doesn't
    /// exist, or [`ScoreDetailError::Unexpected`] on an underlying database
    /// failure.
    pub async fn score_tags(&self, score_id: ScoreId) -> Result<Vec<TagDetail>, ScoreDetailError> {
        let conn = self.connect().await?;

        // Same reasoning as `score_people`: zero attached tags is a valid
        // state for an existing score, so it can't double as the
        // existence check the way `score_titles`'s emptiness can.
        if !score_exists_conn(&conn, score_id).await? {
            return Err(ScoreDetailError::ScoreNotFound(score_id));
        }

        let (sql, values) = Query::select()
            .column((Tags::Table, Tags::Id))
            .column((Tags::Table, Tags::Name))
            .column((ScoreTags::Table, ScoreTags::Value))
            .from(ScoreTags::Table)
            .inner_join(
                Tags::Table,
                Expr::col((Tags::Table, Tags::Id)).equals((ScoreTags::Table, ScoreTags::TagId)),
            )
            .and_where(Expr::col((ScoreTags::Table, ScoreTags::ScoreId)).eq(score_id.0))
            .order_by((Tags::Table, Tags::Name), Order::Asc)
            .order_by((ScoreTags::Table, ScoreTags::Value), Order::Asc)
            .build(SqliteQueryBuilder);
        let mut rows = conn.query(&sql, params::to_turso_values(values)).await?;

        let mut tags = Vec::new();
        while let Some(row) = rows.next().await? {
            tags.push(TagDetail {
                id: TagId(row.get::<i64>(0)?),
                name: row.get::<String>(1)?,
                value: row.get::<Option<String>>(2)?,
            });
        }
        Ok(tags)
    }

    /// Fetches all detail about a score in one call.
    ///
    /// Equivalent to calling [`Catalogue::score_description`],
    /// [`Catalogue::score_titles`], [`Catalogue::score_people`], and
    /// [`Catalogue::score_tags`] separately and bundling the results. Use those
    /// directly instead if only some of this data is needed.
    ///
    /// # Errors
    ///
    /// Returns [`ScoreDetailError::ScoreNotFound`] if `score_id` doesn't
    /// exist, or [`ScoreDetailError::Unexpected`] on an underlying database
    /// failure.
    pub async fn score_detail(&self, score_id: ScoreId) -> Result<ScoreDetail, ScoreDetailError> {
        let description = self.score_description(score_id).await?;
        let (primary_title_id, primary_title, alternate_titles) =
            self.score_titles(score_id).await?;
        let people = self.score_people(score_id).await?;
        let tags = self.score_tags(score_id).await?;

        Ok(ScoreDetail {
            id: score_id,
            description,
            primary_title_id,
            primary_title,
            alternate_titles,
            people,
            tags,
        })
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

/// `true` if `score_id` exists. Connection-based counterpart to
/// [`score_exists`], for the read-only `Catalogue::score_*` methods that
/// deliberately don't open a transaction.
async fn score_exists_conn(conn: &Connection, score_id: ScoreId) -> Result<bool, DatabaseError> {
    let (sql, values) = Query::select()
        .expr(Expr::val(1))
        .from(Scores::Table)
        .and_where(Expr::col(Scores::Id).eq(score_id.0))
        .build(SqliteQueryBuilder);
    let mut rows = conn.query(&sql, params::to_turso_values(values)).await?;
    Ok(query_one(&mut rows).await?.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalogue::migration;
    use crate::query::{PersonName, TagItem};

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

    fn name(value: &str) -> PersonName {
        PersonName::parse(value).unwrap()
    }

    fn item(value: &str) -> TagItem {
        TagItem::parse(value).unwrap()
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

    #[tokio::test]
    async fn score_description_rejects_missing_score() {
        let catalogue = test_catalogue().await;
        assert_eq!(
            catalogue.score_description(ScoreId(1)).await.unwrap_err(),
            ScoreDetailError::ScoreNotFound(ScoreId(1))
        );
    }

    #[tokio::test]
    async fn score_titles_returns_primary_and_alternates() {
        let catalogue = test_catalogue().await;
        let score_id = catalogue.create_score(title("Ave Maria")).await.unwrap();
        let alt_id = catalogue
            .add_title(score_id, title("Ave Maria (alt)"))
            .await
            .unwrap();

        let (primary_title_id, primary_title, alternate_titles) =
            catalogue.score_titles(score_id).await.unwrap();

        assert_eq!(primary_title, "Ave Maria");
        assert_ne!(primary_title_id, alt_id);
        assert_eq!(
            alternate_titles,
            vec![(alt_id, "Ave Maria (alt)".to_string())]
        );
    }

    #[tokio::test]
    async fn score_titles_rejects_missing_score() {
        let catalogue = test_catalogue().await;
        assert_eq!(
            catalogue.score_titles(ScoreId(1)).await.unwrap_err(),
            ScoreDetailError::ScoreNotFound(ScoreId(1))
        );
    }

    /// Unlike `score_titles`, an empty `score_people` result is a valid
    /// state for an existing score (no attached people yet), so this needs
    /// its own explicit existence check - this test is what would fail if
    /// that check were ever removed and replaced with "empty means
    /// missing" reasoning.
    #[tokio::test]
    async fn score_people_returns_empty_for_an_existing_score_with_no_people() {
        let catalogue = test_catalogue().await;
        let score_id = catalogue.create_score(title("Ave Maria")).await.unwrap();

        assert_eq!(catalogue.score_people(score_id).await.unwrap(), Vec::new());
    }

    #[tokio::test]
    async fn score_people_rejects_missing_score() {
        let catalogue = test_catalogue().await;
        assert_eq!(
            catalogue.score_people(ScoreId(1)).await.unwrap_err(),
            ScoreDetailError::ScoreNotFound(ScoreId(1))
        );
    }

    /// Same reasoning as `score_people_returns_empty_for_an_existing_score_with_no_people`.
    #[tokio::test]
    async fn score_tags_returns_empty_for_an_existing_score_with_no_tags() {
        let catalogue = test_catalogue().await;
        let score_id = catalogue.create_score(title("Ave Maria")).await.unwrap();

        assert_eq!(catalogue.score_tags(score_id).await.unwrap(), Vec::new());
    }

    #[tokio::test]
    async fn score_tags_rejects_missing_score() {
        let catalogue = test_catalogue().await;
        assert_eq!(
            catalogue.score_tags(ScoreId(1)).await.unwrap_err(),
            ScoreDetailError::ScoreNotFound(ScoreId(1))
        );
    }

    #[tokio::test]
    async fn score_detail_returns_not_found_for_a_missing_score() {
        let catalogue = test_catalogue().await;
        assert_eq!(
            catalogue.score_detail(ScoreId(1)).await.unwrap_err(),
            ScoreDetailError::ScoreNotFound(ScoreId(1))
        );
    }

    #[tokio::test]
    async fn score_detail_returns_description_and_primary_title_for_a_freshly_created_score() {
        let catalogue = test_catalogue().await;
        let score_id = catalogue.create_score(title("Ave Maria")).await.unwrap();

        let detail = catalogue.score_detail(score_id).await.unwrap();

        assert_eq!(detail.id, score_id);
        assert_eq!(detail.description, None);
        assert_eq!(detail.primary_title, "Ave Maria");
        assert_eq!(detail.alternate_titles, Vec::new());
        assert_eq!(detail.people, Vec::new());
        assert_eq!(detail.tags, Vec::new());
    }

    #[tokio::test]
    async fn score_detail_includes_alternate_titles() {
        let catalogue = test_catalogue().await;
        let score_id = catalogue.create_score(title("Ave Maria")).await.unwrap();
        let alt_id = catalogue
            .add_title(score_id, title("Ave Maria (alt)"))
            .await
            .unwrap();

        let detail = catalogue.score_detail(score_id).await.unwrap();

        assert_eq!(
            detail.alternate_titles,
            vec![(alt_id, "Ave Maria (alt)".to_string())]
        );
    }

    #[tokio::test]
    async fn score_detail_includes_attached_people() {
        let catalogue = test_catalogue().await;
        let score_id = catalogue.create_score(title("Ave Maria")).await.unwrap();
        let person_id = catalogue.create_person(&[name("Anna")]).await.unwrap();
        catalogue.attach_person(score_id, person_id).await.unwrap();

        let detail = catalogue.score_detail(score_id).await.unwrap();

        assert_eq!(
            detail.people,
            vec![PersonDetail {
                id: person_id,
                display_name: "Anna".to_string(),
            }]
        );
    }

    #[tokio::test]
    async fn score_detail_includes_attached_tags_with_their_values() {
        let catalogue = test_catalogue().await;
        let score_id = catalogue.create_score(title("Ave Maria")).await.unwrap();
        let tag_id = catalogue.create_tag(item("difficulty")).await.unwrap();
        catalogue
            .attach_tag(score_id, tag_id, Some(item("5")))
            .await
            .unwrap();

        let detail = catalogue.score_detail(score_id).await.unwrap();

        assert_eq!(
            detail.tags,
            vec![TagDetail {
                id: tag_id,
                name: "difficulty".to_string(),
                value: Some("5".to_string()),
            }]
        );
    }

    /// A score may carry the same tag with several different values at
    /// once - `score_detail` must list each attachment separately rather
    /// than collapsing them (see `tag.rs`'s module doc).
    #[tokio::test]
    async fn score_detail_lists_a_tag_multiple_times_when_multiple_values_are_attached() {
        let catalogue = test_catalogue().await;
        let score_id = catalogue.create_score(title("Ave Maria")).await.unwrap();
        let tag_id = catalogue.create_tag(item("laulupidu")).await.unwrap();
        catalogue
            .attach_tag(score_id, tag_id, Some(item("2020")))
            .await
            .unwrap();
        catalogue
            .attach_tag(score_id, tag_id, Some(item("2025")))
            .await
            .unwrap();

        let detail = catalogue.score_detail(score_id).await.unwrap();

        assert_eq!(
            detail.tags,
            vec![
                TagDetail {
                    id: tag_id,
                    name: "laulupidu".to_string(),
                    value: Some("2020".to_string()),
                },
                TagDetail {
                    id: tag_id,
                    name: "laulupidu".to_string(),
                    value: Some("2025".to_string()),
                },
            ]
        );
    }

    #[tokio::test]
    async fn score_detail_lists_a_valueless_tag_attachment_as_none() {
        let catalogue = test_catalogue().await;
        let score_id = catalogue.create_score(title("Ave Maria")).await.unwrap();
        let tag_id = catalogue.create_tag(item("sacred")).await.unwrap();
        catalogue.attach_tag(score_id, tag_id, None).await.unwrap();

        let detail = catalogue.score_detail(score_id).await.unwrap();

        assert_eq!(
            detail.tags,
            vec![TagDetail {
                id: tag_id,
                name: "sacred".to_string(),
                value: None,
            }]
        );
    }
}
