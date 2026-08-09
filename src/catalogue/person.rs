//! Person-level modification methods: creating/deleting a person, and
//! attaching/detaching one to/from a score.

use super::normalize::{case_fold, normalize_text, words};
use super::schema::{People, PersonWords, ScorePeople};
use super::score::score_exists;
use super::{Catalogue, DatabaseError, PersonId, ScoreId, params, query_one};
use crate::query::PersonName;
use sea_query::{Expr, ExprTrait, OnConflict, Query, SqliteQueryBuilder};
use std::ops::Deref;

/// Errors from [`Catalogue::create_person`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CreatePersonError {
    #[error("person must have at least one name component")]
    Empty,
    #[error("a person with this name already exists: {0}")]
    AlreadyExists(PersonId),
    #[error(transparent)]
    Unexpected(#[from] DatabaseError),
}

impl From<turso::Error> for CreatePersonError {
    fn from(error: turso::Error) -> Self {
        Self::Unexpected(DatabaseError::from(error))
    }
}

/// Errors from [`Catalogue::delete_person`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DeletePersonError {
    #[error("person {0} does not exist")]
    PersonNotFound(PersonId),
    #[error(transparent)]
    Unexpected(#[from] DatabaseError),
}

impl From<turso::Error> for DeletePersonError {
    fn from(error: turso::Error) -> Self {
        Self::Unexpected(DatabaseError::from(error))
    }
}

/// Errors from [`Catalogue::attach_person`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AttachPersonError {
    #[error("score {0} does not exist")]
    ScoreNotFound(ScoreId),
    #[error("person {0} does not exist")]
    PersonNotFound(PersonId),
    #[error("person {1} is already attached to score {0}")]
    AlreadyAttached(ScoreId, PersonId),
    #[error(transparent)]
    Unexpected(#[from] DatabaseError),
}

impl From<turso::Error> for AttachPersonError {
    fn from(error: turso::Error) -> Self {
        Self::Unexpected(DatabaseError::from(error))
    }
}

/// Errors from [`Catalogue::detach_person`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DetachPersonError {
    #[error("score {0} does not exist")]
    ScoreNotFound(ScoreId),
    #[error("person {0} does not exist")]
    PersonNotFound(PersonId),
    #[error("person {1} is not attached to score {0}")]
    NotAttached(ScoreId, PersonId),
    #[error(transparent)]
    Unexpected(#[from] DatabaseError),
}

impl From<turso::Error> for DetachPersonError {
    fn from(error: turso::Error) -> Self {
        Self::Unexpected(DatabaseError::from(error))
    }
}

impl Catalogue {
    /// Creates a new person from one or more name components, joined with a
    /// single space to form the stored `display_name` (e.g. `["Johann",
    /// "Bach"]` -> `"Johann Bach"`).
    ///
    /// No two people may share the same full name, case-insensitively - if
    /// `names` case-folds to a `display_name` that already exists, this
    /// returns the existing person's id via
    /// [`CreatePersonError::AlreadyExists`] rather than creating a
    /// duplicate. Real people who share a name need a distinguishing name
    /// component added by whoever manages the catalogue. Names that differ
    /// only by diacritics (e.g. "Marten Roots" vs "Märten Roots") are
    /// *not* treated as duplicates - only the search index folds those
    /// together, not this uniqueness check.
    ///
    /// # Errors
    ///
    /// Returns [`CreatePersonError::Empty`] if `names` is empty,
    /// [`CreatePersonError::AlreadyExists`] if a person with the same
    /// (case-folded) full name already exists, or [`CreatePersonError::Unexpected`]
    /// on an underlying database failure.
    pub async fn create_person(&self, names: &[PersonName]) -> Result<PersonId, CreatePersonError> {
        if names.is_empty() {
            return Err(CreatePersonError::Empty);
        }

        let mut conn = self.connect().await?;
        let tx = conn.transaction().await?;

        let display_name = names
            .iter()
            .map(Deref::deref)
            .collect::<Vec<&str>>()
            .join(" ");
        let display_name_normalized = normalize_text(&display_name);
        let display_name_key = case_fold(&display_name);

        if let Some(existing_id) = person_lookup_by_display_name_key(&tx, &display_name_key).await?
        {
            return Err(CreatePersonError::AlreadyExists(existing_id));
        }

        let (sql, values) = Query::insert()
            .into_table(People::Table)
            .columns([People::DisplayName, People::DisplayNameKey])
            .values_panic([display_name.clone().into(), display_name_key.into()])
            .build(SqliteQueryBuilder);
        tx.execute(&sql, params::to_turso_values(values)).await?;
        let person_id = PersonId(tx.last_insert_rowid());

        insert_person_words(&tx, person_id, &display_name_normalized).await?;

        tx.commit().await?;
        tracing::info!(%person_id, %display_name, "created person");
        Ok(person_id)
    }

    /// Deletes a person and everything that belongs only to it (its
    /// `person_words`, and its `score_people` attachments) via
    /// `ON DELETE CASCADE`.
    ///
    /// # Errors
    ///
    /// Returns [`DeletePersonError::PersonNotFound`] if `person_id` doesn't
    /// exist, or [`DeletePersonError::Unexpected`] on an underlying database
    /// failure.
    pub async fn delete_person(&self, person_id: PersonId) -> Result<PersonId, DeletePersonError> {
        let mut conn = self.connect().await?;
        let tx = conn.transaction().await?;

        if !person_exists(&tx, person_id).await? {
            return Err(DeletePersonError::PersonNotFound(person_id));
        }

        let (sql, values) = Query::delete()
            .from_table(People::Table)
            .and_where(Expr::col(People::Id).eq(person_id.0))
            .build(SqliteQueryBuilder);
        tx.execute(&sql, params::to_turso_values(values)).await?;

        tx.commit().await?;
        tracing::info!(%person_id, "deleted person");
        Ok(person_id)
    }

    /// Attaches a person to a score.
    ///
    /// # Errors
    ///
    /// Returns [`AttachPersonError::ScoreNotFound`] if `score_id` doesn't
    /// exist, [`AttachPersonError::PersonNotFound`] if `person_id` doesn't
    /// exist, [`AttachPersonError::AlreadyAttached`] if the pair is already
    /// attached, or [`AttachPersonError::Unexpected`] on an underlying database
    /// failure.
    pub async fn attach_person(
        &self,
        score_id: ScoreId,
        person_id: PersonId,
    ) -> Result<(), AttachPersonError> {
        let mut conn = self.connect().await?;
        let tx = conn.transaction().await?;

        if !score_exists(&tx, score_id).await? {
            return Err(AttachPersonError::ScoreNotFound(score_id));
        }
        if !person_exists(&tx, person_id).await? {
            return Err(AttachPersonError::PersonNotFound(person_id));
        }
        if score_person_attached(&tx, score_id, person_id).await? {
            return Err(AttachPersonError::AlreadyAttached(score_id, person_id));
        }

        let (sql, values) = Query::insert()
            .into_table(ScorePeople::Table)
            .columns([ScorePeople::ScoreId, ScorePeople::PersonId])
            .values_panic([score_id.0.into(), person_id.0.into()])
            .build(SqliteQueryBuilder);
        tx.execute(&sql, params::to_turso_values(values)).await?;

        tx.commit().await?;
        tracing::info!(%score_id, %person_id, "attached person to score");
        Ok(())
    }

    /// Detaches a person from a score.
    ///
    /// # Errors
    ///
    /// Returns [`DetachPersonError::ScoreNotFound`] if `score_id` doesn't
    /// exist, [`DetachPersonError::PersonNotFound`] if `person_id` doesn't
    /// exist, [`DetachPersonError::NotAttached`] if the pair isn't
    /// attached, or [`DetachPersonError::Unexpected`] on an underlying database
    /// failure.
    pub async fn detach_person(
        &self,
        score_id: ScoreId,
        person_id: PersonId,
    ) -> Result<(), DetachPersonError> {
        let mut conn = self.connect().await?;
        let tx = conn.transaction().await?;

        if !score_exists(&tx, score_id).await? {
            return Err(DetachPersonError::ScoreNotFound(score_id));
        }
        if !person_exists(&tx, person_id).await? {
            return Err(DetachPersonError::PersonNotFound(person_id));
        }
        if !score_person_attached(&tx, score_id, person_id).await? {
            return Err(DetachPersonError::NotAttached(score_id, person_id));
        }

        let (sql, values) = Query::delete()
            .from_table(ScorePeople::Table)
            .and_where(Expr::col(ScorePeople::ScoreId).eq(score_id.0))
            .and_where(Expr::col(ScorePeople::PersonId).eq(person_id.0))
            .build(SqliteQueryBuilder);
        tx.execute(&sql, params::to_turso_values(values)).await?;

        tx.commit().await?;
        tracing::info!(%score_id, %person_id, "detached person from score");
        Ok(())
    }
}

/// `true` if `person_id` exists. Used as an explicit pre-check for domain
/// "not found" errors rather than relying on `turso::Error::Constraint`'s
/// message text. Shared with `search.rs`'s tests and any future module that
/// needs to validate a `PersonId` before using it.
pub(super) async fn person_exists(
    tx: &turso::transaction::Transaction<'_>,
    person_id: PersonId,
) -> Result<bool, DatabaseError> {
    let (sql, values) = Query::select()
        .expr(Expr::val(1))
        .from(People::Table)
        .and_where(Expr::col(People::Id).eq(person_id.0))
        .build(SqliteQueryBuilder);
    let mut rows = tx.query(&sql, params::to_turso_values(values)).await?;
    Ok(query_one(&mut rows).await?.is_some())
}

/// `Some(person_id)` of the person whose `display_name_key` equals
/// `display_name_key`, `None` if none exists. Used by
/// [`Catalogue::create_person`] to enforce one-person-per-full-name,
/// case-insensitively but diacritic-sensitively (see [`case_fold`]).
async fn person_lookup_by_display_name_key(
    tx: &turso::transaction::Transaction<'_>,
    display_name_key: &str,
) -> Result<Option<PersonId>, DatabaseError> {
    let (sql, values) = Query::select()
        .column(People::Id)
        .from(People::Table)
        .and_where(Expr::col(People::DisplayNameKey).eq(display_name_key))
        .build(SqliteQueryBuilder);
    let mut rows = tx.query(&sql, params::to_turso_values(values)).await?;
    let Some(row) = query_one(&mut rows).await? else {
        return Ok(None);
    };
    Ok(Some(PersonId(row.get::<i64>(0)?)))
}

/// `true` if `person_id` is attached to `score_id` via `score_people`.
/// `pub(super)` so `role.rs` can check this before attaching a role to a
/// `(score, person)` pair.
pub(super) async fn score_person_attached(
    tx: &turso::transaction::Transaction<'_>,
    score_id: ScoreId,
    person_id: PersonId,
) -> Result<bool, DatabaseError> {
    let (sql, values) = Query::select()
        .expr(Expr::val(1))
        .from(ScorePeople::Table)
        .and_where(Expr::col(ScorePeople::ScoreId).eq(score_id.0))
        .and_where(Expr::col(ScorePeople::PersonId).eq(person_id.0))
        .build(SqliteQueryBuilder);
    let mut rows = tx.query(&sql, params::to_turso_values(values)).await?;
    Ok(query_one(&mut rows).await?.is_some())
}

/// Inserts one `person_words` row per distinct word in `normalized`,
/// ignoring words already indexed for this person. Mirrors `title.rs`'s
/// `insert_title_words`.
async fn insert_person_words(
    tx: &turso::transaction::Transaction<'_>,
    person_id: PersonId,
    display_name_normalized: &str,
) -> Result<(), DatabaseError> {
    for word in words(display_name_normalized) {
        let (sql, values) = Query::insert()
            .into_table(PersonWords::Table)
            .columns([PersonWords::PersonId, PersonWords::Word])
            .values_panic([person_id.0.into(), word.into()])
            .on_conflict(OnConflict::new().do_nothing().to_owned())
            .build(SqliteQueryBuilder);
        tx.execute(&sql, params::to_turso_values(values)).await?;
    }
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

    fn name(value: &str) -> PersonName {
        PersonName::parse(value).unwrap()
    }

    fn title(value: &str) -> Title {
        value.parse().unwrap()
    }

    #[tokio::test]
    async fn create_person_stores_display_name_and_words() {
        let catalogue = test_catalogue().await;
        let person_id = catalogue
            .create_person(&[name("Johann"), name("Bach")])
            .await
            .unwrap();

        let conn = catalogue.connect().await.unwrap();
        let mut rows = conn
            .query(
                "SELECT display_name, display_name_key FROM people WHERE id = ?",
                (person_id.0,),
            )
            .await
            .unwrap();
        let row = rows.next().await.unwrap().unwrap();
        assert_eq!(row.get::<String>(0).unwrap(), "Johann Bach");
        assert_eq!(row.get::<String>(1).unwrap(), "johann bach");

        let mut rows = conn
            .query("SELECT word FROM person_words ORDER BY word", ())
            .await
            .unwrap();
        let mut found = Vec::new();
        while let Some(row) = rows.next().await.unwrap() {
            found.push(row.get::<String>(0).unwrap());
        }
        assert_eq!(found, &["bach", "johann"]);
    }

    #[tokio::test]
    async fn create_person_rejects_empty_names() {
        let catalogue = test_catalogue().await;
        assert_eq!(
            catalogue.create_person(&[]).await.unwrap_err(),
            CreatePersonError::Empty
        );
    }

    #[tokio::test]
    async fn create_person_rejects_duplicate_display_name_regardless_of_case() {
        let catalogue = test_catalogue().await;
        let first_id = catalogue
            .create_person(&[name("Johann"), name("Bach")])
            .await
            .unwrap();

        assert_eq!(
            catalogue
                .create_person(&[name("JOHANN"), name("BACH")])
                .await
                .unwrap_err(),
            CreatePersonError::AlreadyExists(first_id)
        );
    }

    /// "Marten Roots" and "Märten Roots" are different people - the
    /// uniqueness check must not fold diacritics away, even though search
    /// (via `person_words`/`normalize_text`) intentionally does.
    #[tokio::test]
    async fn create_person_allows_names_that_differ_only_by_diacritics() {
        let catalogue = test_catalogue().await;
        let without_diacritic_id = catalogue
            .create_person(&[name("Marten"), name("Roots")])
            .await
            .unwrap();
        let with_diacritic_id = catalogue
            .create_person(&[name("Märten"), name("Roots")])
            .await
            .unwrap();

        assert_ne!(without_diacritic_id, with_diacritic_id);
    }

    #[tokio::test]
    async fn delete_person_cascades_to_words_and_attachments() {
        let catalogue = test_catalogue().await;
        let person_id = catalogue.create_person(&[name("Anna")]).await.unwrap();
        let score_id = catalogue.create_score(title("Ave Maria")).await.unwrap();
        catalogue.attach_person(score_id, person_id).await.unwrap();

        catalogue.delete_person(person_id).await.unwrap();

        let conn = catalogue.connect().await.unwrap();
        let mut rows = conn
            .query("SELECT COUNT(*) FROM person_words", ())
            .await
            .unwrap();
        assert_eq!(
            rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap(),
            0
        );
        let mut rows = conn
            .query("SELECT COUNT(*) FROM score_people", ())
            .await
            .unwrap();
        assert_eq!(
            rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn delete_person_rejects_missing_person() {
        let catalogue = test_catalogue().await;
        assert_eq!(
            catalogue.delete_person(PersonId(1)).await.unwrap_err(),
            DeletePersonError::PersonNotFound(PersonId(1))
        );
    }

    #[tokio::test]
    async fn attach_person_creates_score_people_row() {
        let catalogue = test_catalogue().await;
        let person_id = catalogue.create_person(&[name("Anna")]).await.unwrap();
        let score_id = catalogue.create_score(title("Ave Maria")).await.unwrap();

        catalogue.attach_person(score_id, person_id).await.unwrap();

        let conn = catalogue.connect().await.unwrap();
        let mut rows = conn
            .query(
                "SELECT COUNT(*) FROM score_people WHERE score_id = ? AND person_id = ?",
                (score_id.0, person_id.0),
            )
            .await
            .unwrap();
        assert_eq!(
            rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap(),
            1
        );
    }

    #[tokio::test]
    async fn attach_person_rejects_missing_score() {
        let catalogue = test_catalogue().await;
        let person_id = catalogue.create_person(&[name("Anna")]).await.unwrap();
        assert_eq!(
            catalogue
                .attach_person(ScoreId(1), person_id)
                .await
                .unwrap_err(),
            AttachPersonError::ScoreNotFound(ScoreId(1))
        );
    }

    #[tokio::test]
    async fn attach_person_rejects_missing_person() {
        let catalogue = test_catalogue().await;
        let score_id = catalogue.create_score(title("Ave Maria")).await.unwrap();
        assert_eq!(
            catalogue
                .attach_person(score_id, PersonId(1))
                .await
                .unwrap_err(),
            AttachPersonError::PersonNotFound(PersonId(1))
        );
    }

    #[tokio::test]
    async fn attach_person_rejects_duplicate_attachment() {
        let catalogue = test_catalogue().await;
        let person_id = catalogue.create_person(&[name("Anna")]).await.unwrap();
        let score_id = catalogue.create_score(title("Ave Maria")).await.unwrap();
        catalogue.attach_person(score_id, person_id).await.unwrap();

        assert_eq!(
            catalogue
                .attach_person(score_id, person_id)
                .await
                .unwrap_err(),
            AttachPersonError::AlreadyAttached(score_id, person_id)
        );
    }

    #[tokio::test]
    async fn detach_person_removes_score_people_row() {
        let catalogue = test_catalogue().await;
        let person_id = catalogue.create_person(&[name("Anna")]).await.unwrap();
        let score_id = catalogue.create_score(title("Ave Maria")).await.unwrap();
        catalogue.attach_person(score_id, person_id).await.unwrap();

        catalogue.detach_person(score_id, person_id).await.unwrap();

        let conn = catalogue.connect().await.unwrap();
        let mut rows = conn
            .query("SELECT COUNT(*) FROM score_people", ())
            .await
            .unwrap();
        assert_eq!(
            rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn detach_person_rejects_not_attached() {
        let catalogue = test_catalogue().await;
        let person_id = catalogue.create_person(&[name("Anna")]).await.unwrap();
        let score_id = catalogue.create_score(title("Ave Maria")).await.unwrap();

        assert_eq!(
            catalogue
                .detach_person(score_id, person_id)
                .await
                .unwrap_err(),
            DetachPersonError::NotAttached(score_id, person_id)
        );
    }

    #[tokio::test]
    async fn detach_person_rejects_missing_score() {
        let catalogue = test_catalogue().await;
        let person_id = catalogue.create_person(&[name("Anna")]).await.unwrap();
        assert_eq!(
            catalogue
                .detach_person(ScoreId(1), person_id)
                .await
                .unwrap_err(),
            DetachPersonError::ScoreNotFound(ScoreId(1))
        );
    }

    #[tokio::test]
    async fn detach_person_rejects_missing_person() {
        let catalogue = test_catalogue().await;
        let score_id = catalogue.create_score(title("Ave Maria")).await.unwrap();
        assert_eq!(
            catalogue
                .detach_person(score_id, PersonId(1))
                .await
                .unwrap_err(),
            DetachPersonError::PersonNotFound(PersonId(1))
        );
    }
}
