mod expected;

use crate::query::parser::DisplayToken;
pub use expected::{Expected, ExpectedValue, IntoExpected, IntoExpectedValue};
use std::fmt::Display;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, PartialEq, Eq, Hash, thiserror::Error)]
#[error("{kind}")]
pub struct Error {
    pub help: Vec<Help>,
    pub kind: ErrorKind,
}

impl Error {
    fn add_help(mut self, help: Help) -> Error {
        self.help.push(help);
        self
    }
}

impl From<ErrorKind> for Error {
    fn from(value: ErrorKind) -> Self {
        Error {
            help: Vec::new(),
            kind: value,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Help {
    /// When there is content after a complete tag mode `##` item.
    TagModeSingleComponent,
    /// When there is content after a complete tag mode `@@` item.
    NameModeSingleComponent,
}

impl Display for Help {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Help::TagModeSingleComponent => {
                write!(
                    f,
                    "tag mode (`##`) queries can only have a single component"
                )
            }
            Help::NameModeSingleComponent => {
                write!(
                    f,
                    "name mode (`##`) queries can only have a single component"
                )
            }
        }
    }
}

pub trait AddHelp<T> {
    fn add_help(self, help: Help) -> Result<T>;
}

impl<T> AddHelp<T> for Result<T> {
    fn add_help(self, help: Help) -> Result<T> {
        self.map_err(|e| e.add_help(help))
    }
}
