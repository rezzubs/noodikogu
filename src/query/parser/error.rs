mod expected;

use crate::query::parser::DisplayToken;
pub use expected::{Expected, ExpectedValue, IntoExpected, IntoExpectedValue};
use std::fmt::Display;

pub type Result<T> = std::result::Result<T, Error>;

pub trait AddHelp<T> {
    fn with_help(self, help: impl Into<String>) -> Result<T>;
}

impl<T> AddHelp<T> for Result<T> {
    fn with_help(self, help: impl Into<String>) -> Result<T> {
        self.map_err(|e| e.with_help(help))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{kind}")]
pub struct Error {
    help: Option<String>,
    kind: ErrorKind,
}

impl Error {
    fn with_help(self, help: impl Into<String>) -> Error {
        Error {
            help: Some(help.into()),
            ..self
        }
    }
}

impl From<ErrorKind> for Error {
    fn from(value: ErrorKind) -> Self {
        Error {
            help: None,
            kind: value,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorKind {
    UnexpectedEof {
        expected: Expected,
    },
    UnexpectedToken {
        expected: Expected,
        found: DisplayToken,
    },
    InvalidTagName {
        invalid: char,
        name: String,
    },
    InvalidPersonName {
        invalid: char,
        name: String,
    },
    Empty,
}

impl Display for ErrorKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ErrorKind::UnexpectedEof { expected } => {
                write!(f, "Ran out of input, expected: {}", expected)
            }
            ErrorKind::UnexpectedToken { expected, found } => {
                write!(f, "expected: {}, found: {}", expected, found)
            }
            ErrorKind::Empty => write!(f, "The input is empty"),
            ErrorKind::InvalidTagName { invalid, name } => {
                write!(
                    f,
                    "tag name `{name}` contains an invalid character: `{invalid}`"
                )
            }
            ErrorKind::InvalidPersonName { invalid, name } => {
                write!(
                    f,
                    "person name `{name}` contains an invalid character: `{invalid}`"
                )
            }
        }
    }
}
