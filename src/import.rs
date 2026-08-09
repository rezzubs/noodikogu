//! Bulk-imports scores from a CSV file into a [`Catalogue`], for bootstrapping
//! a fresh database. A one-time tool, not a repeatable "safely extend an
//! existing database" workflow - scores are never deduplicated, since titles
//! are not a unique namespace in this catalogue.
//!
//! # CSV format
//!
//! The first row is a header row; every following row is one score. Each
//! header cell is one of:
//!
//! - `title` - the *first* such column is the score's primary title
//!   (required per row); every later one is an alternate title, added via
//!   [`Catalogue::add_title`] (an empty cell means no alternate title from
//!   that column for that row).
//! - `tag:<name>` - `<name>` is the tag's name. A non-empty cell is that
//!   tag's value, attached via [`Catalogue::create_tag`] +
//!   [`Catalogue::attach_tag`]. An empty cell means the tag is not attached
//!   for that row.
//! - `person` or `person:<role>` - a non-empty cell holds one person's full
//!   name, split into components on any combination of `,`, `.`, and
//!   whitespace, attached via [`Catalogue::create_person`] +
//!   [`Catalogue::attach_person`]. With the `:<role>` suffix, the role is
//!   additionally attached via [`Catalogue::create_role`] +
//!   [`Catalogue::attach_role`]; a bare `person` column attaches the person
//!   with no role. The same name may appear in multiple `person:*` columns
//!   in one row - that's one person gaining multiple roles, not multiple
//!   people.
//!
//! Each [`Catalogue`] call commits its own transaction independently (true
//! of every method on `Catalogue`, not something this module introduces),
//! so a failure partway through a row leaves whatever already committed in
//! place rather than rolling back. Since this is a one-time bootstrap tool,
//! that's an accepted tradeoff: a failed import needs the partial row
//! cleaned up by hand before re-running, rather than being automatically
//! recovered.

mod csv;
mod header;

pub use csv::CsvError;
pub use header::HeaderError;

use crate::catalogue::{
    AddTitleError, AttachPersonError, AttachRoleError, AttachTagError, Catalogue,
    CreatePersonError, CreateRoleError, CreateTagError, DatabaseError, PersonId, RoleId, RoleName,
    ScoreId, TagId, Title,
};
use crate::query::{PersonName, PersonNameError, TagItem, TagItemError};
use header::Header;

/// Error returned by [`import_csv`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ImportError {
    #[error("csv file has no header row")]
    EmptyCsv,
    #[error(transparent)]
    Csv(#[from] CsvError),
    #[error(transparent)]
    Header(#[from] HeaderError),
    #[error("row {row}: {source}")]
    Row {
        row: usize,
        #[source]
        source: RowError,
    },
}

/// Error processing a single data row, wrapped by [`ImportError::Row`] with
/// the offending row's line number.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RowError {
    #[error("{expected} columns expected, found {found}")]
    ColumnCountMismatch { expected: usize, found: usize },
    #[error("primary title cell is empty")]
    MissingPrimaryTitle,
    #[error("column {column}: {source}")]
    InvalidTagValue { column: usize, source: TagItemError },
    #[error("column {column}: {source}")]
    InvalidPersonName {
        column: usize,
        source: PersonNameError,
    },
    #[error(transparent)]
    Database(#[from] DatabaseError),
    #[error(transparent)]
    CreatePerson(#[from] CreatePersonError),
    #[error(transparent)]
    CreateTag(#[from] CreateTagError),
    #[error(transparent)]
    CreateRole(#[from] CreateRoleError),
    #[error(transparent)]
    AttachPerson(#[from] AttachPersonError),
    #[error(transparent)]
    AttachRole(#[from] AttachRoleError),
    #[error(transparent)]
    AttachTag(#[from] AttachTagError),
    #[error(transparent)]
    AddTitle(#[from] AddTitleError),
}

/// Imports every row of `csv_text` into `catalogue`, returning the number
/// of scores created. See the module doc for the expected CSV format.
///
/// # Errors
///
/// Returns [`ImportError::EmptyCsv`] if `csv_text` has no header row,
/// [`ImportError::Csv`]/[`ImportError::Header`] if the file or its header
/// row is malformed, or [`ImportError::Row`] if a data row fails to
/// process - the whole import aborts on the first such failure, leaving
/// every row imported before it in place (see the module doc).
pub async fn import_csv(catalogue: &Catalogue, csv_text: &str) -> Result<usize, ImportError> {
    let rows = csv::parse_rows(csv_text)?;
    let mut rows = rows.into_iter();

    let Some(header_cells) = rows.next() else {
        return Err(ImportError::EmptyCsv);
    };
    let header = header::parse_header(&header_cells)?;

    let mut imported = 0;
    for (offset, row) in rows.enumerate() {
        process_row(catalogue, &header, &row)
            .await
            .map_err(|source| ImportError::Row {
                row: offset + 2,
                source,
            })?;
        imported += 1;
    }

    Ok(imported)
}

/// Processes one data row: creates its score, then attaches its alternate
/// titles, tags, and people/roles.
async fn process_row(
    catalogue: &Catalogue,
    header: &Header,
    row: &[String],
) -> Result<ScoreId, RowError> {
    if row.len() != header.column_count {
        return Err(RowError::ColumnCountMismatch {
            expected: header.column_count,
            found: row.len(),
        });
    }

    let primary_title: Title = row[header.primary_title_column]
        .parse()
        .map_err(|_| RowError::MissingPrimaryTitle)?;
    let score_id = catalogue.create_score(primary_title).await?;

    for &column in &header.alternate_title_columns {
        let cell = row[column].trim();
        let Ok(title) = cell.parse() else {
            debug_assert!(cell.is_empty());
            continue;
        };

        catalogue.add_title(score_id, title).await?;
    }

    for (column, tag_name) in &header.tag_columns {
        let cell = row[*column].trim();
        if cell.is_empty() {
            continue;
        }
        let value = TagItem::parse(cell).map_err(|source| RowError::InvalidTagValue {
            column: *column,
            source,
        })?;
        let tag_id = get_or_create_tag(catalogue, tag_name.clone()).await?;
        catalogue.attach_tag(score_id, tag_id, Some(value)).await?;
    }

    for (column, role_name) in &header.person_columns {
        let cell = row[*column].trim();
        if cell.is_empty() {
            continue;
        }

        let mut names = Vec::new();
        for component in split_person_name(cell) {
            let name =
                PersonName::parse(component).map_err(|source| RowError::InvalidPersonName {
                    column: *column,
                    source,
                })?;
            names.push(name);
        }

        let person_id = get_or_create_person(catalogue, &names).await?;
        attach_person_idempotent(catalogue, score_id, person_id).await?;

        if let Some(role_name) = role_name {
            let role_id = get_or_create_role(catalogue, role_name.clone()).await?;
            attach_role_idempotent(catalogue, score_id, person_id, role_id).await?;
        }
    }

    Ok(score_id)
}

/// Splits a person-name cell into components on any combination of `,`,
/// `.`, and whitespace (e.g. `"Bach, Johann Sebastian"` and
/// `"Bach.Johann.Sebastian"` both yield `["Bach", "Johann", "Sebastian"]`).
fn split_person_name(cell: &str) -> impl Iterator<Item = &str> {
    cell.split(|c: char| c == ',' || c == '.' || c.is_whitespace())
        .filter(|component| !component.is_empty())
}

/// Creates `name`, or reuses the existing tag if one with this name already
/// exists - a pre-existing tag (from an earlier row, or from before the
/// import ran at all) is expected, not an error.
async fn get_or_create_tag(catalogue: &Catalogue, name: TagItem) -> Result<TagId, RowError> {
    match catalogue.create_tag(name).await {
        Ok(id) | Err(CreateTagError::AlreadyExists(id)) => Ok(id),
        Err(e) => Err(e.into()),
    }
}

/// Creates a person from `names`, or reuses the existing person if one with
/// this full name already exists. See [`get_or_create_tag`]'s doc for why
/// this is expected rather than an error.
async fn get_or_create_person(
    catalogue: &Catalogue,
    names: &[PersonName],
) -> Result<PersonId, RowError> {
    match catalogue.create_person(names).await {
        Ok(id) | Err(CreatePersonError::AlreadyExists(id)) => Ok(id),
        Err(e) => Err(e.into()),
    }
}

/// Creates `name`, or reuses the existing role if one with this name
/// already exists. See [`get_or_create_tag`]'s doc for why this is expected
/// rather than an error.
async fn get_or_create_role(catalogue: &Catalogue, name: RoleName) -> Result<RoleId, RowError> {
    match catalogue.create_role(name).await {
        Ok(id) | Err(CreateRoleError::AlreadyExists(id)) => Ok(id),
        Err(e) => Err(e.into()),
    }
}

/// Attaches `person_id` to `score_id`, treating "already attached" as
/// success - expected when the same person's name appears in more than one
/// `person:*` column for this row.
async fn attach_person_idempotent(
    catalogue: &Catalogue,
    score_id: ScoreId,
    person_id: PersonId,
) -> Result<(), RowError> {
    match catalogue.attach_person(score_id, person_id).await {
        Ok(()) | Err(AttachPersonError::AlreadyAttached(_, _)) => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// Attaches `role_id` to `person_id` on `score_id`, treating "already
/// attached" as success. See [`attach_person_idempotent`]'s doc.
async fn attach_role_idempotent(
    catalogue: &Catalogue,
    score_id: ScoreId,
    person_id: PersonId,
    role_id: RoleId,
) -> Result<(), RowError> {
    match catalogue.attach_role(score_id, person_id, role_id).await {
        Ok(()) | Err(AttachRoleError::AlreadyAttached(_, _, _)) => Ok(()),
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_catalogue() -> Catalogue {
        Catalogue::open(":memory:")
            .await
            .expect("in-memory catalogue should always open successfully")
    }

    #[test]
    fn split_person_name_splits_on_commas_dots_and_whitespace() {
        assert_eq!(
            split_person_name("Bach, Johann.Sebastian").collect::<Vec<_>>(),
            vec!["Bach", "Johann", "Sebastian"]
        );
    }

    #[tokio::test]
    async fn imports_a_row_with_primary_and_alternate_titles_a_tag_and_a_person_with_a_role() {
        let catalogue = test_catalogue().await;
        let csv = "title,title,tag:difficulty,person:composer\n\
                    Ave Maria,Ave Maria (alt),5,Johann Bach\n";

        let imported = import_csv(&catalogue, csv).await.unwrap();

        assert_eq!(imported, 1);
        let detail = catalogue.score_detail(ScoreId(1)).await.unwrap();
        assert_eq!(detail.primary_title, "Ave Maria");
        assert_eq!(detail.alternate_titles.len(), 1);
        assert_eq!(detail.alternate_titles[0].1, "Ave Maria (alt)");
        assert_eq!(detail.tags.len(), 1);
        assert_eq!(detail.tags[0].name, "difficulty");
        assert_eq!(detail.tags[0].value.as_deref(), Some("5"));
        assert_eq!(detail.people.len(), 1);
        assert_eq!(detail.people[0].display_name, "Johann Bach");
        assert_eq!(detail.people[0].roles.len(), 1);
        assert_eq!(detail.people[0].roles[0].name, "composer");
    }

    #[tokio::test]
    async fn empty_alternate_title_cell_adds_no_alternate_title() {
        let catalogue = test_catalogue().await;
        let csv = "title,title\nAve Maria,\n";

        import_csv(&catalogue, csv).await.unwrap();

        let detail = catalogue.score_detail(ScoreId(1)).await.unwrap();
        assert_eq!(detail.alternate_titles, Vec::new());
    }

    #[tokio::test]
    async fn same_name_in_two_person_columns_gets_both_roles_on_one_person() {
        let catalogue = test_catalogue().await;
        let csv = "title,person:composer,person:arranger\n\
                    Ave Maria,Johann Bach,Johann Bach\n";

        import_csv(&catalogue, csv).await.unwrap();

        let detail = catalogue.score_detail(ScoreId(1)).await.unwrap();
        assert_eq!(detail.people.len(), 1);
        assert_eq!(detail.people[0].roles.len(), 2);
    }

    #[tokio::test]
    async fn bare_person_column_attaches_with_no_role() {
        let catalogue = test_catalogue().await;
        let csv = "title,person\nAve Maria,Anna\n";

        import_csv(&catalogue, csv).await.unwrap();

        let detail = catalogue.score_detail(ScoreId(1)).await.unwrap();
        assert_eq!(detail.people.len(), 1);
        assert_eq!(detail.people[0].roles, Vec::new());
    }

    #[tokio::test]
    async fn person_name_cell_splits_on_dots_and_whitespace() {
        let catalogue = test_catalogue().await;
        let csv = "title,person\nAve Maria,Bach.Johann Sebastian\n";

        import_csv(&catalogue, csv).await.unwrap();

        let detail = catalogue.score_detail(ScoreId(1)).await.unwrap();
        assert_eq!(detail.people[0].display_name, "Bach Johann Sebastian");
    }

    #[tokio::test]
    async fn tag_column_with_empty_cell_leaves_the_tag_unattached_for_that_row() {
        let catalogue = test_catalogue().await;
        let csv = "title,tag:difficulty\nAve Maria,5\nRequiem,\n";

        import_csv(&catalogue, csv).await.unwrap();

        let first = catalogue.score_detail(ScoreId(1)).await.unwrap();
        assert_eq!(first.tags.len(), 1);
        let second = catalogue.score_detail(ScoreId(2)).await.unwrap();
        assert_eq!(second.tags, Vec::new());
    }

    #[tokio::test]
    async fn a_tag_and_person_reused_across_rows_are_not_duplicated() {
        let catalogue = test_catalogue().await;
        let csv = "title,tag:difficulty,person:composer\n\
                    Ave Maria,5,Johann Bach\n\
                    Requiem,5,Johann Bach\n";

        import_csv(&catalogue, csv).await.unwrap();

        let first = catalogue.score_detail(ScoreId(1)).await.unwrap();
        let second = catalogue.score_detail(ScoreId(2)).await.unwrap();
        assert_eq!(first.people[0].id, second.people[0].id);
        assert_eq!(first.tags[0].id, second.tags[0].id);
    }

    #[tokio::test]
    async fn duplicate_primary_titles_both_import_as_separate_scores() {
        let catalogue = test_catalogue().await;
        let csv = "title\nAve Maria\nAve Maria\n";

        let imported = import_csv(&catalogue, csv).await.unwrap();

        assert_eq!(imported, 2);
        let first = catalogue.score_detail(ScoreId(1)).await.unwrap();
        let second = catalogue.score_detail(ScoreId(2)).await.unwrap();
        assert_eq!(first.primary_title, "Ave Maria");
        assert_eq!(second.primary_title, "Ave Maria");
    }

    #[tokio::test]
    async fn invalid_tag_value_characters_abort_the_import() {
        let catalogue = test_catalogue().await;
        let csv = "title,tag:difficulty\nAve Maria,not valid!\n";

        let err = import_csv(&catalogue, csv).await.unwrap_err();

        assert!(matches!(
            err,
            ImportError::Row {
                row: 2,
                source: RowError::InvalidTagValue { column: 1, .. }
            }
        ));
    }

    #[tokio::test]
    async fn invalid_person_name_characters_abort_the_import() {
        let catalogue = test_catalogue().await;
        let csv = "title,person\nAve Maria,Anna123\n";

        let err = import_csv(&catalogue, csv).await.unwrap_err();

        assert!(matches!(
            err,
            ImportError::Row {
                row: 2,
                source: RowError::InvalidPersonName { column: 1, .. }
            }
        ));
    }

    #[tokio::test]
    async fn column_count_mismatch_aborts_the_import() {
        let catalogue = test_catalogue().await;
        let csv = "title,tag:difficulty\nAve Maria\n";

        let err = import_csv(&catalogue, csv).await.unwrap_err();

        assert_eq!(
            err,
            ImportError::Row {
                row: 2,
                source: RowError::ColumnCountMismatch {
                    expected: 2,
                    found: 1,
                },
            }
        );
    }

    #[tokio::test]
    async fn empty_primary_title_cell_aborts_the_import() {
        let catalogue = test_catalogue().await;
        let csv = "title\n\n";

        let err = import_csv(&catalogue, csv).await.unwrap_err();

        assert_eq!(
            err,
            ImportError::Row {
                row: 2,
                source: RowError::MissingPrimaryTitle,
            }
        );
    }

    #[tokio::test]
    async fn empty_csv_is_an_error() {
        let catalogue = test_catalogue().await;

        assert_eq!(
            import_csv(&catalogue, "").await.unwrap_err(),
            ImportError::EmptyCsv
        );
    }
}
