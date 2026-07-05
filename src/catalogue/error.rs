//! Errors produced by the [`crate::catalogue`] module.

/// Errors that can occur while opening, connecting to, or migrating a
/// [`super::Catalogue`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, thiserror::Error)]
pub enum Error {
    /// An unhandled database error.
    #[error("unexpected database error: {0}")]
    Unexpected(String),
}

impl From<turso::Error> for Error {
    fn from(error: turso::Error) -> Self {
        Self::Unexpected(error.to_string())
    }
}
