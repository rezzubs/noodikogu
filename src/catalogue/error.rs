//! Errors produced by the [`crate::catalogue`] module.

/// An unexpected database-level failure, not tied to any specific
/// operation. `Catalogue::open`/`connect` can only ever fail this way;
/// every operation-specific error type elsewhere in this module (e.g.
#[derive(Debug, Clone, PartialEq, Eq, Hash, thiserror::Error)]
#[error("unexpected database error: {0}")]
pub struct DatabaseError(String);

impl From<turso::Error> for DatabaseError {
    fn from(error: turso::Error) -> Self {
        Self(error.to_string())
    }
}
