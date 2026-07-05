//! Strongly-typed row identifiers, so a `ScoreId` and a `TitleId` (or a
//! bare `i64`) can't be swapped for each other by accident.
//!
//! The inner value is `pub`: any `i64` is a syntactically valid id (there's
//! no invariant to protect beyond the type distinction itself), and code
//! outside this crate needs to construct one directly - e.g. a web layer
//! parsing a `/scores/:id` path segment, or htmx wiring up ids in markup.
//! Whether the id actually refers to an existing row is always checked
//! separately, on every use.

use std::fmt;

/// The id of a row in the `scores` table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ScoreId(pub i64);

impl fmt::Display for ScoreId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<ScoreId> for turso::Value {
    fn from(id: ScoreId) -> Self {
        turso::Value::Integer(id.0)
    }
}

/// The id of a row in the `titles` table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TitleId(pub i64);

impl fmt::Display for TitleId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<TitleId> for turso::Value {
    fn from(id: TitleId) -> Self {
        turso::Value::Integer(id.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn score_id_displays_as_raw_integer() {
        assert_eq!(ScoreId(42).to_string(), "42");
    }

    #[test]
    fn title_id_displays_as_raw_integer() {
        assert_eq!(TitleId(7).to_string(), "7");
    }
}
