mod expected;

use super::lexer::LexError;
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

    pub fn add_context(mut self, context: Context) -> Error {
        self.context = Some(context);
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
        Self::new(ErrorKind::InvalidPersonName {
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

impl From<LexError> for Error {
    fn from(e: LexError) -> Self {
        match e {
            LexError::EmptyQuotedString { .. } => Error::new(ErrorKind::EmptyQuotedString),
        }
    }
}

impl Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.kind)?;
        if let Some(context) = &self.context {
            write!(f, " {}", context)?;
        }

        for h in &self.help {
            write!(f, "\n-> Help: {}", h)?;
        }

        Ok(())
    }
}

impl std::error::Error for Error {}

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
    /// An empty quoted string `""` appeared in the input.
    EmptyQuotedString,
    Empty,
}

impl Display for ErrorKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ErrorKind::UnexpectedEof { expected } => {
                write!(f, "Ran out of input, expected {}", expected)
            }
            ErrorKind::UnexpectedToken { expected, found } => {
                write!(f, "expected {}, found {}", expected, found)
            }
            ErrorKind::EmptyQuotedString => write!(f, "empty quoted strings are not allowed"),
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
    SpaceAfterQuote,
    SpaceBeforeOr,
    SpaceAfterOr,
    SpaceBeforeNot,
    SpaceBeforeWord,

    DoubleNegation,
    OrMissingItem,
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
            Help::SpaceAfterQuote => write!(f, "Add a space after the quoted text"),
            Help::SpaceBeforeOr => write!(f, "Add a space before the `|` operator"),
            Help::SpaceAfterOr => write!(f, "Add a space after the `|` operator"),
            Help::SpaceBeforeNot => write!(f, "Add a space before the '!' operator"),
            Help::SpaceBeforeWord => write!(f, "Add a space before the word"),
            Help::DoubleNegation => write!(f, "Double negation `!!` is not allowed"),
            Help::OrMissingItem => write!(f, "The last item of the `|` sequence is missing"),
        }
    }
}

pub trait AddHelp<T> {
    fn add_help(self, help: Help) -> Result<T>;

    fn add_context(self, context: Context) -> Result<T>;
}

impl<T> AddHelp<T> for Result<T> {
    fn add_help(self, help: Help) -> Result<T> {
        self.map_err(|e| e.add_help(help))
    }

    fn add_context(self, context: Context) -> Result<T> {
        self.map_err(|e| e.add_context(context))
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
