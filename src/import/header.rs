//! Parses a CSV header row into a typed [`Header`] describing which column
//! is which field, per the format documented on [`super::import_csv`].

use crate::catalogue::{EmptyRoleNameError, RoleName};
use crate::query::{TagItem, TagItemError};

/// A parsed header row: which column holds the primary title, which hold
/// alternate titles, and which are tag/person columns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Header {
    /// Total number of columns in the header row - every data row must
    /// have exactly this many cells.
    pub(super) column_count: usize,
    pub(super) primary_title_column: usize,
    pub(super) alternate_title_columns: Vec<usize>,
    pub(super) tag_columns: Vec<(usize, TagItem)>,
    /// `None` means a bare `person` column - the person is attached with
    /// no role.
    pub(super) person_columns: Vec<(usize, Option<RoleName>)>,
}

/// Error returned by [`parse_header`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HeaderError {
    #[error("no 'title' column present in the header row")]
    NoTitleColumn,
    #[error(
        "column {column}: unrecognized header '{header}' (expected 'title', 'tag:<name>', 'person', or 'person:<role>')"
    )]
    UnknownFieldType { column: usize, header: String },
    #[error("column {column}: invalid tag name: {source}")]
    InvalidTagName {
        column: usize,
        source: TagItemError,
    },
    #[error("column {column}: invalid person role: {source}")]
    InvalidRoleName {
        column: usize,
        source: EmptyRoleNameError,
    },
}

/// Classifies each cell of a header row (see the module doc) into a
/// [`Header`].
///
/// # Errors
///
/// Returns [`HeaderError::NoTitleColumn`] if no cell is exactly `"title"`,
/// [`HeaderError::UnknownFieldType`] if a cell matches none of the
/// recognized forms, or [`HeaderError::InvalidTagName`]/
/// [`HeaderError::InvalidRoleName`] if a `tag:`/`person:` cell's key fails
/// to parse.
pub(super) fn parse_header(cells: &[String]) -> Result<Header, HeaderError> {
    let mut primary_title_column = None;
    let mut alternate_title_columns = Vec::new();
    let mut tag_columns = Vec::new();
    let mut person_columns = Vec::new();

    for (column, cell) in cells.iter().enumerate() {
        if cell == "title" {
            match primary_title_column {
                None => primary_title_column = Some(column),
                Some(_) => alternate_title_columns.push(column),
            }
        } else if cell == "person" {
            person_columns.push((column, None));
        } else if let Some(role) = cell.strip_prefix("person:") {
            let role = role
                .parse::<RoleName>()
                .map_err(|source| HeaderError::InvalidRoleName { column, source })?;
            person_columns.push((column, Some(role)));
        } else if let Some(name) = cell.strip_prefix("tag:") {
            let name = TagItem::parse(name)
                .map_err(|source| HeaderError::InvalidTagName { column, source })?;
            tag_columns.push((column, name));
        } else {
            return Err(HeaderError::UnknownFieldType {
                column,
                header: cell.clone(),
            });
        }
    }

    let Some(primary_title_column) = primary_title_column else {
        return Err(HeaderError::NoTitleColumn);
    };

    Ok(Header {
        column_count: cells.len(),
        primary_title_column,
        alternate_title_columns,
        tag_columns,
        person_columns,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cells(values: &[&str]) -> Vec<String> {
        values.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn parses_a_mixed_valid_header() {
        let header = parse_header(&cells(&["title", "tag:difficulty", "person:composer"])).unwrap();

        assert_eq!(header.column_count, 3);
        assert_eq!(header.primary_title_column, 0);
        assert_eq!(header.alternate_title_columns, Vec::<usize>::new());
        assert_eq!(header.tag_columns, vec![(1, TagItem::parse("difficulty").unwrap())]);
        assert_eq!(
            header.person_columns,
            vec![(2, Some("composer".parse().unwrap()))]
        );
    }

    #[test]
    fn second_title_column_becomes_an_alternate() {
        let header = parse_header(&cells(&["title", "title"])).unwrap();

        assert_eq!(header.primary_title_column, 0);
        assert_eq!(header.alternate_title_columns, vec![1]);
    }

    #[test]
    fn bare_person_column_has_no_role() {
        let header = parse_header(&cells(&["title", "person"])).unwrap();

        assert_eq!(header.person_columns, vec![(1, None)]);
    }

    #[test]
    fn rejects_a_header_with_no_title_column() {
        assert_eq!(
            parse_header(&cells(&["tag:difficulty"])).unwrap_err(),
            HeaderError::NoTitleColumn
        );
    }

    #[test]
    fn rejects_an_unrecognized_header_cell() {
        assert_eq!(
            parse_header(&cells(&["title", "nonsense"])).unwrap_err(),
            HeaderError::UnknownFieldType {
                column: 1,
                header: "nonsense".to_string(),
            }
        );
    }

    #[test]
    fn rejects_a_tag_column_with_invalid_characters_in_the_name() {
        let err = parse_header(&cells(&["title", "tag:not valid"])).unwrap_err();
        assert!(matches!(err, HeaderError::InvalidTagName { column: 1, .. }));
    }

    #[test]
    fn rejects_a_person_column_with_an_empty_role() {
        assert_eq!(
            parse_header(&cells(&["title", "person:"])).unwrap_err(),
            HeaderError::InvalidRoleName {
                column: 1,
                source: EmptyRoleNameError,
            }
        );
    }
}
