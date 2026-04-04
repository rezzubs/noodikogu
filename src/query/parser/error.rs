mod expected;

use crate::query::parser::DisplayToken;
pub use expected::{Expected, ExpectedValue, IntoExpected, IntoExpectedValue};
use std::fmt::Display;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Error {
    pub help: Vec<Help>,
    pub context: Option<Context>,
    pub kind: ErrorKind,
}

impl Error {
    pub fn new(kind: ErrorKind) -> Self {
        Self {
            help: Vec::new(),
            context: None,
            kind,
        }
    }

    pub fn add_help(mut self, help: Help) -> Error {
        self.help.push(help);
        self
    }

    pub(crate) fn empty() -> Self {
        Self::new(ErrorKind::Empty)
    }

    pub(crate) fn unexpected(expected: impl IntoExpected, found: DisplayToken) -> Self {
        Self::new(ErrorKind::UnexpectedToken {
            expected: expected.into_expected(),
            found,
        })
    }

    pub(crate) fn unexpected_eof(expected: impl IntoExpected) -> Self {
        Self::new(ErrorKind::UnexpectedEof {
            expected: expected.into_expected(),
        })
    }

    pub(crate) fn invalid_person_name(invalid: char, name: impl Into<String>) -> Self {
        Self::new(ErrorKind::InvalidTagName {
            invalid,
            name: name.into(),
        })
    }

    pub(crate) fn invalid_tag_name(invalid: char, name: impl Into<String>) -> Self {
        Self::new(ErrorKind::InvalidTagName {
            invalid,
            name: name.into(),
        })
    }
}

impl Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.kind)?;
        if let Some(context) = &self.context {
            write!(f, " {}", context)?;
        }

        for h in &self.help {
            write!(f, "\nHelp: {}", h)?;
        }

        Ok(())
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
    /// When tag mode `##` appears anywhere but the start of input.
    TagModeAtStart,
    /// When name mode `@@` appears anywhere but the start of input.
    NameModeAtStart,

    SpaceBeforeTag,
    SpaceBeforeGroup,
    SpaceBeforeName,
    SpaceBeforeQuote,
    SpaceBeforeOr,
    SpaceBeforeNot,
}

impl Display for Help {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Help::TagModeSingleComponent => {
                write!(
                    f,
                    "Tag mode (`##`) queries can only have a single component."
                )
            }
            Help::NameModeSingleComponent => {
                write!(
                    f,
                    "Name mode (`##`) queries can only have a single component."
                )
            }
            Help::TagModeAtStart => write!(f, "Tag mode should be set at the beginning."),
            Help::NameModeAtStart => write!(f, "Name mode should be set at the beginning."),
            Help::SpaceBeforeTag => write!(f, "Add a space before the tag"),
            Help::SpaceBeforeGroup => write!(f, "Add a space before the group"),
            Help::SpaceBeforeName => write!(f, "Add a space before the name"),
            Help::SpaceBeforeQuote => write!(f, "Add a space before the quoted text"),
            Help::SpaceBeforeOr => write!(f, "Add a space before the 'or' operator"),
            Help::SpaceBeforeNot => write!(f, "Add a space before the 'not' operator"),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Context {
    EndOfTitle,
}

impl Display for Context {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Context::EndOfTitle => write!(f, "at the end of a title"),
        }
    }
}
