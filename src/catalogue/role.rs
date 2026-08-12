//! Role-level modification methods: creating/renaming/deleting a role, and
//! attaching/detaching one to/from a person already attached to a score.
//!
//! A role (e.g. "composer", "arranger") is a shared, reusable label - not
//! per-attachment free text - so it lives in its own `roles` table and is
//! referenced by id from `score_person_roles`. Unlike `tags`, a role can
//! only ever attach to a person that's already attached to the score (see
//! `score_person_roles`'s composite foreign key onto `score_people` in
//! `m0003_roles.sql`), and carries no per-attachment value: renaming a role
//! is a single `UPDATE` on `roles` and is visible everywhere that role is
//! used.

use super::normalize::case_fold;
use super::person::{person_exists, score_person_attached};
use super::schema::{Roles, ScorePersonRoles};
use super::score::score_exists;
use super::{Catalogue, DatabaseError, PersonId, RoleId, ScoreId, params, query_one};
use sea_query::{Expr, ExprTrait, Query, SqliteQueryBuilder};
use std::fmt;
use std::ops::Deref;
use std::str::FromStr;

/// A validated, trimmed, non-empty role name (e.g. "composer", "second
/// violin"). Mirrors `crate::catalogue::title::Title`'s validation, not
/// `crate::query::TagItem`/`PersonName`'s restrictive character class -
/// roles aren't part of the search-query grammar, so there's no delimiter
/// safety to protect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoleName(String);

/// Returned by [`RoleName::from_str`] if the value is empty after trimming.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("role name cannot be empty")]
pub struct EmptyRoleNameError;

impl FromStr for RoleName {
    type Err = EmptyRoleNameError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(EmptyRoleNameError);
        }
        Ok(Self(trimmed.to_string()))
    }
}

impl Deref for RoleName {
    type Target = str;

    fn deref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RoleName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Errors from [`Catalogue::create_role`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CreateRoleError {
    #[error("a role with this name already exists: {0}")]
    AlreadyExists(RoleId),
    #[error(transparent)]
    Unexpected(#[from] DatabaseError),
}

impl From<turso::Error> for CreateRoleError {
    fn from(error: turso::Error) -> Self {
        Self::Unexpected(DatabaseError::from(error))
    }
}

/// Errors from [`Catalogue::rename_role`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RenameRoleError {
    #[error("role {0} does not exist")]
    RoleNotFound(RoleId),
    #[error("a role with this name already exists: {0}")]
    AlreadyExists(RoleId),
    #[error(transparent)]
    Unexpected(#[from] DatabaseError),
}

impl From<turso::Error> for RenameRoleError {
    fn from(error: turso::Error) -> Self {
        Self::Unexpected(DatabaseError::from(error))
    }
}

/// Errors from [`Catalogue::delete_role`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DeleteRoleError {
    #[error("role {0} does not exist")]
    RoleNotFound(RoleId),
    #[error(transparent)]
    Unexpected(#[from] DatabaseError),
}

impl From<turso::Error> for DeleteRoleError {
    fn from(error: turso::Error) -> Self {
        Self::Unexpected(DatabaseError::from(error))
    }
}

/// Errors from [`Catalogue::attach_role`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AttachRoleError {
    #[error("score {0} does not exist")]
    ScoreNotFound(ScoreId),
    #[error("person {0} does not exist")]
    PersonNotFound(PersonId),
    #[error("role {0} does not exist")]
    RoleNotFound(RoleId),
    #[error("person {1} is not attached to score {0}")]
    PersonNotAttachedToScore(ScoreId, PersonId),
    #[error("role {2} is already attached to person {1} on score {0}")]
    AlreadyAttached(ScoreId, PersonId, RoleId),
    #[error(transparent)]
    Unexpected(#[from] DatabaseError),
}

impl From<turso::Error> for AttachRoleError {
    fn from(error: turso::Error) -> Self {
        Self::Unexpected(DatabaseError::from(error))
    }
}

/// Errors from [`Catalogue::detach_role`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DetachRoleError {
    #[error("score {0} does not exist")]
    ScoreNotFound(ScoreId),
    #[error("person {0} does not exist")]
    PersonNotFound(PersonId),
    #[error("role {0} does not exist")]
    RoleNotFound(RoleId),
    #[error("role {2} is not attached to person {1} on score {0}")]
    NotAttached(ScoreId, PersonId, RoleId),
    #[error(transparent)]
    Unexpected(#[from] DatabaseError),
}

impl From<turso::Error> for DetachRoleError {
    fn from(error: turso::Error) -> Self {
        Self::Unexpected(DatabaseError::from(error))
    }
}

impl Catalogue {
    /// Creates a new role from a name.
    ///
    /// `roles` is a shared, reusable vocabulary (attached to people on
    /// scores via `score_person_roles`'s junction), so a name is never
    /// duplicated: if one already exists (case-insensitively, but
    /// diacritic-sensitively - see `docs/decisions/0008-role-name-uniqueness-key.md`),
    /// this returns its id via [`CreateRoleError::AlreadyExists`] instead of
    /// creating a second row.
    ///
    /// # Errors
    ///
    /// Returns [`CreateRoleError::AlreadyExists`] if a role with the same
    /// (case-folded) name already exists, or [`CreateRoleError::Unexpected`]
    /// on an underlying database failure.
    pub async fn create_role(&self, name: RoleName) -> Result<RoleId, CreateRoleError> {
        let mut conn = self.connect().await?;
        let tx = conn.transaction().await?;

        let name_key = case_fold(&name);

        if let Some(existing_id) = role_lookup_by_name_key(&tx, &name_key).await? {
            return Err(CreateRoleError::AlreadyExists(existing_id));
        }

        let (sql, values) = Query::insert()
            .into_table(Roles::Table)
            .columns([Roles::Name, Roles::NameKey])
            .values_panic([name.deref().into(), name_key.into()])
            .build(SqliteQueryBuilder);
        tx.execute(&sql, params::to_turso_values(values)).await?;
        let role_id = RoleId(tx.last_insert_rowid());

        tx.commit().await?;
        tracing::info!(%role_id, name = %*name, "created role");
        Ok(role_id)
    }

    /// Renames an existing role. Since a role's name lives in exactly one
    /// row, this is visible everywhere that role is attached - no other
    /// data needs to change.
    ///
    /// Renaming to the role's own current name (or a case variant of it) is
    /// allowed and is a no-op beyond rewriting `name`/`name_key`.
    ///
    /// # Errors
    ///
    /// Returns [`RenameRoleError::RoleNotFound`] if `role_id` doesn't
    /// exist, [`RenameRoleError::AlreadyExists`] if a *different* role
    /// already has this (case-folded) name, or
    /// [`RenameRoleError::Unexpected`] on an underlying database failure.
    pub async fn rename_role(
        &self,
        role_id: RoleId,
        name: RoleName,
    ) -> Result<RoleId, RenameRoleError> {
        let mut conn = self.connect().await?;
        let tx = conn.transaction().await?;

        if !role_exists(&tx, role_id).await? {
            return Err(RenameRoleError::RoleNotFound(role_id));
        }

        let name_key = case_fold(&name);
        if let Some(existing_id) = role_lookup_by_name_key(&tx, &name_key).await?
            && existing_id != role_id
        {
            return Err(RenameRoleError::AlreadyExists(existing_id));
        }

        let (sql, values) = Query::update()
            .table(Roles::Table)
            .value(Roles::Name, name.to_string())
            .value(Roles::NameKey, name_key)
            .and_where(Expr::col(Roles::Id).eq(role_id.0))
            .build(SqliteQueryBuilder);
        tx.execute(&sql, params::to_turso_values(values)).await?;

        tx.commit().await?;
        tracing::info!(%role_id, name = %*name, "renamed role");
        Ok(role_id)
    }

    /// Deletes a role and its `score_person_roles` attachments via
    /// `ON DELETE CASCADE`.
    ///
    /// # Errors
    ///
    /// Returns [`DeleteRoleError::RoleNotFound`] if `role_id` doesn't
    /// exist, or [`DeleteRoleError::Unexpected`] on an underlying database
    /// failure.
    pub async fn delete_role(&self, role_id: RoleId) -> Result<RoleId, DeleteRoleError> {
        let mut conn = self.connect().await?;
        let tx = conn.transaction().await?;

        if !role_exists(&tx, role_id).await? {
            return Err(DeleteRoleError::RoleNotFound(role_id));
        }

        let (sql, values) = Query::delete()
            .from_table(Roles::Table)
            .and_where(Expr::col(Roles::Id).eq(role_id.0))
            .build(SqliteQueryBuilder);
        tx.execute(&sql, params::to_turso_values(values)).await?;

        tx.commit().await?;
        tracing::info!(%role_id, "deleted role");
        Ok(role_id)
    }

    /// Attaches a role to a person on a score. The person must already be
    /// attached to the score via [`Catalogue::attach_person`] - a role
    /// can't attach to a person the score doesn't have.
    ///
    /// # Errors
    ///
    /// Returns [`AttachRoleError::ScoreNotFound`] if `score_id` doesn't
    /// exist, [`AttachRoleError::PersonNotFound`] if `person_id` doesn't
    /// exist, [`AttachRoleError::RoleNotFound`] if `role_id` doesn't exist,
    /// [`AttachRoleError::PersonNotAttachedToScore`] if `person_id` isn't
    /// attached to `score_id`, [`AttachRoleError::AlreadyAttached`] if this
    /// exact `(score, person, role)` combination is already attached, or
    /// [`AttachRoleError::Unexpected`] on an underlying database failure.
    pub async fn attach_role(
        &self,
        score_id: ScoreId,
        person_id: PersonId,
        role_id: RoleId,
    ) -> Result<(), AttachRoleError> {
        let mut conn = self.connect().await?;
        let tx = conn.transaction().await?;

        if !score_exists(&tx, score_id).await? {
            return Err(AttachRoleError::ScoreNotFound(score_id));
        }
        if !person_exists(&tx, person_id).await? {
            return Err(AttachRoleError::PersonNotFound(person_id));
        }
        if !role_exists(&tx, role_id).await? {
            return Err(AttachRoleError::RoleNotFound(role_id));
        }
        if !score_person_attached(&tx, score_id, person_id).await? {
            return Err(AttachRoleError::PersonNotAttachedToScore(
                score_id, person_id,
            ));
        }
        if score_person_role_attached(&tx, score_id, person_id, role_id).await? {
            return Err(AttachRoleError::AlreadyAttached(
                score_id, person_id, role_id,
            ));
        }

        let (sql, values) = Query::insert()
            .into_table(ScorePersonRoles::Table)
            .columns([
                ScorePersonRoles::ScoreId,
                ScorePersonRoles::PersonId,
                ScorePersonRoles::RoleId,
            ])
            .values_panic([score_id.0.into(), person_id.0.into(), role_id.0.into()])
            .build(SqliteQueryBuilder);
        tx.execute(&sql, params::to_turso_values(values)).await?;

        tx.commit().await?;
        tracing::info!(%score_id, %person_id, %role_id, "attached role");
        Ok(())
    }

    /// Detaches a role from a person on a score.
    ///
    /// # Errors
    ///
    /// Returns [`DetachRoleError::ScoreNotFound`] if `score_id` doesn't
    /// exist, [`DetachRoleError::PersonNotFound`] if `person_id` doesn't
    /// exist, [`DetachRoleError::RoleNotFound`] if `role_id` doesn't exist,
    /// [`DetachRoleError::NotAttached`] if this `(score, person, role)`
    /// combination isn't attached, or [`DetachRoleError::Unexpected`] on an
    /// underlying database failure.
    pub async fn detach_role(
        &self,
        score_id: ScoreId,
        person_id: PersonId,
        role_id: RoleId,
    ) -> Result<(), DetachRoleError> {
        let mut conn = self.connect().await?;
        let tx = conn.transaction().await?;

        if !score_exists(&tx, score_id).await? {
            return Err(DetachRoleError::ScoreNotFound(score_id));
        }
        if !person_exists(&tx, person_id).await? {
            return Err(DetachRoleError::PersonNotFound(person_id));
        }
        if !role_exists(&tx, role_id).await? {
            return Err(DetachRoleError::RoleNotFound(role_id));
        }
        if !score_person_role_attached(&tx, score_id, person_id, role_id).await? {
            return Err(DetachRoleError::NotAttached(score_id, person_id, role_id));
        }

        let (sql, values) = Query::delete()
            .from_table(ScorePersonRoles::Table)
            .and_where(Expr::col(ScorePersonRoles::ScoreId).eq(score_id.0))
            .and_where(Expr::col(ScorePersonRoles::PersonId).eq(person_id.0))
            .and_where(Expr::col(ScorePersonRoles::RoleId).eq(role_id.0))
            .build(SqliteQueryBuilder);
        tx.execute(&sql, params::to_turso_values(values)).await?;

        tx.commit().await?;
        tracing::info!(%score_id, %person_id, %role_id, "detached role");
        Ok(())
    }
}

/// `true` if `role_id` exists. Used as an explicit pre-check for domain
/// "not found" errors rather than relying on `turso::Error::Constraint`'s
/// message text.
pub(super) async fn role_exists(
    tx: &turso::transaction::Transaction<'_>,
    role_id: RoleId,
) -> Result<bool, DatabaseError> {
    let (sql, values) = Query::select()
        .expr(Expr::val(1))
        .from(Roles::Table)
        .and_where(Expr::col(Roles::Id).eq(role_id.0))
        .build(SqliteQueryBuilder);
    let mut rows = tx.query(&sql, params::to_turso_values(values)).await?;
    Ok(query_one(&mut rows).await?.is_some())
}

/// `Some(role_id)` of the role whose `name_key` equals `name_key`, `None`
/// if none exists. Used by [`Catalogue::create_role`] and
/// [`Catalogue::rename_role`] to enforce one-role-per-name,
/// case-insensitively but diacritic-sensitively (see [`case_fold`]).
async fn role_lookup_by_name_key(
    tx: &turso::transaction::Transaction<'_>,
    name_key: &str,
) -> Result<Option<RoleId>, DatabaseError> {
    let (sql, values) = Query::select()
        .column(Roles::Id)
        .from(Roles::Table)
        .and_where(Expr::col(Roles::NameKey).eq(name_key))
        .build(SqliteQueryBuilder);
    let mut rows = tx.query(&sql, params::to_turso_values(values)).await?;
    let Some(row) = query_one(&mut rows).await? else {
        return Ok(None);
    };
    Ok(Some(RoleId(row.get::<i64>(0)?)))
}

/// `true` if `role_id` is attached to `person_id` on `score_id` via
/// `score_person_roles`.
async fn score_person_role_attached(
    tx: &turso::transaction::Transaction<'_>,
    score_id: ScoreId,
    person_id: PersonId,
    role_id: RoleId,
) -> Result<bool, DatabaseError> {
    let (sql, values) = Query::select()
        .expr(Expr::val(1))
        .from(ScorePersonRoles::Table)
        .and_where(Expr::col(ScorePersonRoles::ScoreId).eq(score_id.0))
        .and_where(Expr::col(ScorePersonRoles::PersonId).eq(person_id.0))
        .and_where(Expr::col(ScorePersonRoles::RoleId).eq(role_id.0))
        .build(SqliteQueryBuilder);
    let mut rows = tx.query(&sql, params::to_turso_values(values)).await?;
    Ok(query_one(&mut rows).await?.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalogue::migration;
    use crate::catalogue::title::Title;
    use crate::query::PersonName;

    async fn test_catalogue() -> Catalogue {
        let database = turso::Builder::new_local(":memory:").build().await.unwrap();
        let mut conn = database.connect().unwrap();
        conn.execute("PRAGMA foreign_keys = ON", ()).await.unwrap();
        migration::run(&mut conn, migration::ALL).await.unwrap();
        Catalogue { database }
    }

    fn role_name(value: &str) -> RoleName {
        value.parse().unwrap()
    }

    fn title(value: &str) -> Title {
        value.parse().unwrap()
    }

    fn person_name(value: &str) -> PersonName {
        PersonName::parse(value).unwrap()
    }

    #[tokio::test]
    async fn create_role_stores_name_and_key() {
        let catalogue = test_catalogue().await;
        let role_id = catalogue.create_role(role_name("Composer")).await.unwrap();

        let conn = catalogue.connect().await.unwrap();
        let mut rows = conn
            .query(
                "SELECT name, name_key FROM roles WHERE id = ?",
                (role_id.0,),
            )
            .await
            .unwrap();
        let row = rows.next().await.unwrap().unwrap();
        assert_eq!(row.get::<String>(0).unwrap(), "Composer");
        assert_eq!(row.get::<String>(1).unwrap(), "composer");
    }

    #[tokio::test]
    async fn create_role_rejects_duplicate_name_regardless_of_case() {
        let catalogue = test_catalogue().await;
        let first_id = catalogue.create_role(role_name("Composer")).await.unwrap();

        assert_eq!(
            catalogue
                .create_role(role_name("COMPOSER"))
                .await
                .unwrap_err(),
            CreateRoleError::AlreadyExists(first_id)
        );
    }

    /// Roles differing only by diacritics are distinct - uniqueness must
    /// not fold them together, matching `people.display_name_key`'s model
    /// rather than `tags.name_normalized`'s (see ADR 0008).
    #[tokio::test]
    async fn create_role_allows_names_that_differ_only_by_diacritics() {
        let catalogue = test_catalogue().await;
        let without_diacritic_id = catalogue.create_role(role_name("arranger")).await.unwrap();
        let with_diacritic_id = catalogue.create_role(role_name("arränger")).await.unwrap();

        assert_ne!(without_diacritic_id, with_diacritic_id);
    }

    #[tokio::test]
    async fn rename_role_updates_name_and_key() {
        let catalogue = test_catalogue().await;
        let role_id = catalogue.create_role(role_name("Compser")).await.unwrap();

        catalogue
            .rename_role(role_id, role_name("Composer"))
            .await
            .unwrap();

        let conn = catalogue.connect().await.unwrap();
        let mut rows = conn
            .query(
                "SELECT name, name_key FROM roles WHERE id = ?",
                (role_id.0,),
            )
            .await
            .unwrap();
        let row = rows.next().await.unwrap().unwrap();
        assert_eq!(row.get::<String>(0).unwrap(), "Composer");
        assert_eq!(row.get::<String>(1).unwrap(), "composer");
    }

    #[tokio::test]
    async fn rename_role_allows_renaming_to_its_own_current_name() {
        let catalogue = test_catalogue().await;
        let role_id = catalogue.create_role(role_name("Composer")).await.unwrap();

        catalogue
            .rename_role(role_id, role_name("COMPOSER"))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn rename_role_rejects_missing_role() {
        let catalogue = test_catalogue().await;
        assert_eq!(
            catalogue
                .rename_role(RoleId(1), role_name("Composer"))
                .await
                .unwrap_err(),
            RenameRoleError::RoleNotFound(RoleId(1))
        );
    }

    #[tokio::test]
    async fn rename_role_rejects_conflicting_name() {
        let catalogue = test_catalogue().await;
        let composer_id = catalogue.create_role(role_name("Composer")).await.unwrap();
        let arranger_id = catalogue.create_role(role_name("Arranger")).await.unwrap();

        assert_eq!(
            catalogue
                .rename_role(arranger_id, role_name("composer"))
                .await
                .unwrap_err(),
            RenameRoleError::AlreadyExists(composer_id)
        );
    }

    #[tokio::test]
    async fn delete_role_cascades_to_attachments() {
        let catalogue = test_catalogue().await;
        let role_id = catalogue.create_role(role_name("Composer")).await.unwrap();
        let score_id = catalogue.create_score(title("Ave Maria")).await.unwrap();
        let person_id = catalogue
            .create_person(&[person_name("Anna")])
            .await
            .unwrap();
        catalogue.attach_person(score_id, person_id).await.unwrap();
        catalogue
            .attach_role(score_id, person_id, role_id)
            .await
            .unwrap();

        catalogue.delete_role(role_id).await.unwrap();

        let conn = catalogue.connect().await.unwrap();
        let mut rows = conn
            .query("SELECT COUNT(*) FROM score_person_roles", ())
            .await
            .unwrap();
        assert_eq!(
            rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn delete_role_rejects_missing_role() {
        let catalogue = test_catalogue().await;
        assert_eq!(
            catalogue.delete_role(RoleId(1)).await.unwrap_err(),
            DeleteRoleError::RoleNotFound(RoleId(1))
        );
    }

    #[tokio::test]
    async fn attach_role_creates_score_person_roles_row() {
        let catalogue = test_catalogue().await;
        let role_id = catalogue.create_role(role_name("Composer")).await.unwrap();
        let score_id = catalogue.create_score(title("Ave Maria")).await.unwrap();
        let person_id = catalogue
            .create_person(&[person_name("Anna")])
            .await
            .unwrap();
        catalogue.attach_person(score_id, person_id).await.unwrap();

        catalogue
            .attach_role(score_id, person_id, role_id)
            .await
            .unwrap();

        let conn = catalogue.connect().await.unwrap();
        let mut rows = conn
            .query(
                "SELECT COUNT(*) FROM score_person_roles
                 WHERE score_id = ? AND person_id = ? AND role_id = ?",
                (score_id.0, person_id.0, role_id.0),
            )
            .await
            .unwrap();
        assert_eq!(
            rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap(),
            1
        );
    }

    #[tokio::test]
    async fn attach_role_allows_multiple_roles_for_the_same_person_on_a_score() {
        let catalogue = test_catalogue().await;
        let composer_id = catalogue.create_role(role_name("Composer")).await.unwrap();
        let arranger_id = catalogue.create_role(role_name("Arranger")).await.unwrap();
        let score_id = catalogue.create_score(title("Ave Maria")).await.unwrap();
        let person_id = catalogue
            .create_person(&[person_name("Anna")])
            .await
            .unwrap();
        catalogue.attach_person(score_id, person_id).await.unwrap();

        catalogue
            .attach_role(score_id, person_id, composer_id)
            .await
            .unwrap();
        catalogue
            .attach_role(score_id, person_id, arranger_id)
            .await
            .unwrap();

        let conn = catalogue.connect().await.unwrap();
        let mut rows = conn
            .query(
                "SELECT COUNT(*) FROM score_person_roles WHERE score_id = ? AND person_id = ?",
                (score_id.0, person_id.0),
            )
            .await
            .unwrap();
        assert_eq!(
            rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap(),
            2
        );
    }

    #[tokio::test]
    async fn attach_role_rejects_missing_score() {
        let catalogue = test_catalogue().await;
        let role_id = catalogue.create_role(role_name("Composer")).await.unwrap();
        let person_id = catalogue
            .create_person(&[person_name("Anna")])
            .await
            .unwrap();

        assert_eq!(
            catalogue
                .attach_role(ScoreId(1), person_id, role_id)
                .await
                .unwrap_err(),
            AttachRoleError::ScoreNotFound(ScoreId(1))
        );
    }

    #[tokio::test]
    async fn attach_role_rejects_missing_person() {
        let catalogue = test_catalogue().await;
        let role_id = catalogue.create_role(role_name("Composer")).await.unwrap();
        let score_id = catalogue.create_score(title("Ave Maria")).await.unwrap();

        assert_eq!(
            catalogue
                .attach_role(score_id, PersonId(1), role_id)
                .await
                .unwrap_err(),
            AttachRoleError::PersonNotFound(PersonId(1))
        );
    }

    #[tokio::test]
    async fn attach_role_rejects_missing_role() {
        let catalogue = test_catalogue().await;
        let score_id = catalogue.create_score(title("Ave Maria")).await.unwrap();
        let person_id = catalogue
            .create_person(&[person_name("Anna")])
            .await
            .unwrap();
        catalogue.attach_person(score_id, person_id).await.unwrap();

        assert_eq!(
            catalogue
                .attach_role(score_id, person_id, RoleId(1))
                .await
                .unwrap_err(),
            AttachRoleError::RoleNotFound(RoleId(1))
        );
    }

    #[tokio::test]
    async fn attach_role_rejects_person_not_attached_to_score() {
        let catalogue = test_catalogue().await;
        let role_id = catalogue.create_role(role_name("Composer")).await.unwrap();
        let score_id = catalogue.create_score(title("Ave Maria")).await.unwrap();
        let person_id = catalogue
            .create_person(&[person_name("Anna")])
            .await
            .unwrap();
        // Deliberately not attaching `person_id` to `score_id`.

        assert_eq!(
            catalogue
                .attach_role(score_id, person_id, role_id)
                .await
                .unwrap_err(),
            AttachRoleError::PersonNotAttachedToScore(score_id, person_id)
        );
    }

    #[tokio::test]
    async fn attach_role_rejects_duplicate_attachment() {
        let catalogue = test_catalogue().await;
        let role_id = catalogue.create_role(role_name("Composer")).await.unwrap();
        let score_id = catalogue.create_score(title("Ave Maria")).await.unwrap();
        let person_id = catalogue
            .create_person(&[person_name("Anna")])
            .await
            .unwrap();
        catalogue.attach_person(score_id, person_id).await.unwrap();
        catalogue
            .attach_role(score_id, person_id, role_id)
            .await
            .unwrap();

        assert_eq!(
            catalogue
                .attach_role(score_id, person_id, role_id)
                .await
                .unwrap_err(),
            AttachRoleError::AlreadyAttached(score_id, person_id, role_id)
        );
    }

    #[tokio::test]
    async fn detach_role_removes_score_person_roles_row() {
        let catalogue = test_catalogue().await;
        let role_id = catalogue.create_role(role_name("Composer")).await.unwrap();
        let score_id = catalogue.create_score(title("Ave Maria")).await.unwrap();
        let person_id = catalogue
            .create_person(&[person_name("Anna")])
            .await
            .unwrap();
        catalogue.attach_person(score_id, person_id).await.unwrap();
        catalogue
            .attach_role(score_id, person_id, role_id)
            .await
            .unwrap();

        catalogue
            .detach_role(score_id, person_id, role_id)
            .await
            .unwrap();

        let conn = catalogue.connect().await.unwrap();
        let mut rows = conn
            .query("SELECT COUNT(*) FROM score_person_roles", ())
            .await
            .unwrap();
        assert_eq!(
            rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn detach_role_rejects_not_attached() {
        let catalogue = test_catalogue().await;
        let role_id = catalogue.create_role(role_name("Composer")).await.unwrap();
        let score_id = catalogue.create_score(title("Ave Maria")).await.unwrap();
        let person_id = catalogue
            .create_person(&[person_name("Anna")])
            .await
            .unwrap();
        catalogue.attach_person(score_id, person_id).await.unwrap();

        assert_eq!(
            catalogue
                .detach_role(score_id, person_id, role_id)
                .await
                .unwrap_err(),
            DetachRoleError::NotAttached(score_id, person_id, role_id)
        );
    }

    #[tokio::test]
    async fn detach_role_rejects_missing_score() {
        let catalogue = test_catalogue().await;
        let role_id = catalogue.create_role(role_name("Composer")).await.unwrap();
        let person_id = catalogue
            .create_person(&[person_name("Anna")])
            .await
            .unwrap();

        assert_eq!(
            catalogue
                .detach_role(ScoreId(1), person_id, role_id)
                .await
                .unwrap_err(),
            DetachRoleError::ScoreNotFound(ScoreId(1))
        );
    }

    #[tokio::test]
    async fn detach_role_rejects_missing_person() {
        let catalogue = test_catalogue().await;
        let role_id = catalogue.create_role(role_name("Composer")).await.unwrap();
        let score_id = catalogue.create_score(title("Ave Maria")).await.unwrap();

        assert_eq!(
            catalogue
                .detach_role(score_id, PersonId(1), role_id)
                .await
                .unwrap_err(),
            DetachRoleError::PersonNotFound(PersonId(1))
        );
    }

    #[tokio::test]
    async fn detach_role_rejects_missing_role() {
        let catalogue = test_catalogue().await;
        let score_id = catalogue.create_score(title("Ave Maria")).await.unwrap();
        let person_id = catalogue
            .create_person(&[person_name("Anna")])
            .await
            .unwrap();
        catalogue.attach_person(score_id, person_id).await.unwrap();

        assert_eq!(
            catalogue
                .detach_role(score_id, person_id, RoleId(1))
                .await
                .unwrap_err(),
            DetachRoleError::RoleNotFound(RoleId(1))
        );
    }

    #[test]
    fn role_name_rejects_empty_value() {
        assert_eq!("  ".parse::<RoleName>().unwrap_err(), EmptyRoleNameError);
    }
}
