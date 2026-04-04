use std::fmt::Display;

use crate::query::{
    Person, PersonError, PersonName, PersonNameError, Query, ScoreQuery, TagItemError,
    lexer::{DisplayToken, Lexer, Token, TokenKind},
};

type Result<T> = std::result::Result<T, ParseError>;

trait AddHelp<T> {
    fn with_help(self, help: impl Into<String>) -> Result<T>;
}

impl<T> AddHelp<T> for Result<T> {
    fn with_help(self, help: impl Into<String>) -> Result<T> {
        self.map_err(|e| e.with_help(help))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{kind}")]
pub struct ParseError {
    help: Option<String>,
    kind: ParseErrorKind,
}

impl ParseError {
    fn with_help(self, help: impl Into<String>) -> ParseError {
        ParseError {
            help: Some(help.into()),
            ..self
        }
    }
}

impl From<ParseErrorKind> for ParseError {
    fn from(value: ParseErrorKind) -> Self {
        ParseError {
            help: None,
            kind: value,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseErrorKind {
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

impl Display for ParseErrorKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseErrorKind::UnexpectedEof { expected } => {
                write!(f, "Ran out of input, expected: {}", expected)
            }
            ParseErrorKind::UnexpectedToken { expected, found } => {
                write!(f, "expected: {}, found: {}", expected, found)
            }
            ParseErrorKind::Empty => write!(f, "The input is empty"),
            ParseErrorKind::InvalidTagName { invalid, name } => {
                write!(
                    f,
                    "tag name `{name}` contains an invalid character: `{invalid}`"
                )
            }
            ParseErrorKind::InvalidPersonName { invalid, name } => {
                write!(
                    f,
                    "person name `{name}` contains an invalid character: `{invalid}`"
                )
            }
        }
    }
}

trait IntoExpected {
    fn into_expected(self) -> Expected;
}

impl<T> IntoExpected for T
where
    T: IntoExpectedValue,
{
    fn into_expected(self) -> Expected {
        Expected::One(self.into_expected_value())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expected {
    /// One of multiple expected values.
    OneOf { options: Vec<ExpectedValue> },
    /// Exactly one expected value.
    One(ExpectedValue),
}

impl Expected {
    /// Chain another expected value
    fn or(self, other: impl IntoExpectedValue) -> Self {
        let options = match self {
            Expected::OneOf { mut options } => {
                let other_value = other.into_expected_value();
                options.push(other_value);
                options
            }
            Expected::One(value) => {
                let other_value = other.into_expected_value();
                Vec::from([value, other_value])
            }
        };

        Self::OneOf { options }
    }
}

impl IntoExpected for Expected {
    fn into_expected(self) -> Expected {
        self
    }
}

impl Display for Expected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Expected::OneOf { options } => {
                write!(
                    f,
                    "one of: {}",
                    options
                        .iter()
                        .map(|option| option.to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
            Expected::One(option) => {
                write!(f, "one: {:?}", option)
            }
        }
    }
}

trait IntoExpectedValue {
    fn into_expected_value(self) -> ExpectedValue;

    fn or(self, other: ExpectedValue) -> Expected
    where
        Self: Sized,
    {
        self.into_expected_value().or(other)
    }
}

impl IntoExpectedValue for DisplayToken {
    fn into_expected_value(self) -> ExpectedValue {
        ExpectedValue::Token(self)
    }
}

/// A single
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExpectedValue {
    /// A specific token.
    Token(DisplayToken),
    /// End of input.
    Eof,
    /// A tag name.
    TagName,
    /// A name of a person
    Name,
    /// Any whitespace
    WhiteSpace,
}

impl ExpectedValue {
    /// Chain another expected value
    pub fn or(self, other: impl Into<ExpectedValue>) -> Expected {
        Expected::OneOf {
            options: Vec::from([self.into(), other.into()]),
        }
    }
}

impl IntoExpectedValue for ExpectedValue {
    fn into_expected_value(self) -> ExpectedValue {
        self
    }
}

impl Display for ExpectedValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExpectedValue::Token(token) => {
                write!(f, "{}", token)
            }
            ExpectedValue::Eof => write!(f, "end of input"),
            ExpectedValue::TagName => write!(f, "a tag name"),
            ExpectedValue::Name => write!(f, "a name"),
            ExpectedValue::WhiteSpace => write!(f, "whitespace"),
        }
    }
}

struct Parser<'a> {
    lexer: Lexer<'a>,
    cursor_pos: usize,
    peeked: Option<Token>,
}

impl<'a> Parser<'a> {
    pub fn new(input: &'a str, cursor_pos: usize) -> Self {
        Self::new_with_lexer(Lexer::new(input), cursor_pos)
    }

    pub fn new_with_lexer(lexer: Lexer<'a>, cursor_pos: usize) -> Self {
        Self {
            lexer,
            cursor_pos,
            peeked: None,
        }
    }

    fn peek(&mut self) -> Option<&Token> {
        if self.peeked.is_none() {
            self.peeked = self.lexer.next();
        }
        self.peeked.as_ref()
    }

    fn next(&mut self) -> Option<Token> {
        if let Some(peeked) = self.peeked.take() {
            return Some(peeked);
        };

        self.lexer.next()
    }

    fn advance(&mut self) {
        self.peeked.take().or_else(|| self.lexer.next());
    }

    fn input(&self) -> &str {
        self.lexer.input()
    }

    fn expect(&mut self, kind: TokenKind, expected: impl IntoExpected) -> Result<()> {
        let expected = expected.into_expected();
        let Some(token) = self.next() else {
            return Err(ParseErrorKind::UnexpectedEof { expected }.into());
        };
        if token.kind != kind {
            self.unexpected(token, expected)?;
        }
        Ok(())
    }

    fn expect_eof(&mut self) -> Result<()> {
        let Some(token) = self.next() else {
            return Ok(());
        };
        Err(ParseErrorKind::UnexpectedToken {
            expected: ExpectedValue::Eof.into_expected(),
            found: token.display(self.input()),
        }
        .into())
    }

    fn skip_whitespace(&mut self) {
        while let Some(token) = self.peek() {
            if token.kind == TokenKind::Whitespace {
                self.next();
            } else {
                break;
            }
        }
    }

    /// Helper for returning an unexpected token error. Always returns `Err`. Pretends to return `T` for `Ok`.
    fn unexpected_t<T>(&self, token: impl AsRef<Token>, expected: impl IntoExpected) -> Result<T> {
        Err(ParseErrorKind::UnexpectedToken {
            expected: expected.into_expected(),
            found: token.as_ref().display(self.input()),
        }
        .into())
    }

    /// Helper for returning an unexpected token error. Always returns `Err`. Pretends to return `()` for `Ok`.
    fn unexpected(&self, token: impl AsRef<Token>, expected: impl IntoExpected) -> Result<()> {
        self.unexpected_t(token, expected)
    }

    /// Parses the section following a `##`.
    fn parse_tag_mode(&mut self) -> Result<Query> {
        let Some(first) = self.next() else {
            return Ok(Query::Tag { name: None });
        };

        let tag_name_raw = match first.kind {
            TokenKind::Word => first.content(self.input()),
            TokenKind::Whitespace => {
                self.expect_eof()
                    .with_help("tag mode (`##`) can't have any content after it")?;

                return Ok(Query::Tag { name: None });
            }
            _ => {
                return self.unexpected_t(first, ExpectedValue::TagName.or(ExpectedValue::Eof));
            }
        };

        let tag_name = tag_name_raw.parse().map_err(|err| match err {
            TagItemError::Empty => unreachable!("The lexer should not return empty strings"),
            TagItemError::InvalidChar(invalid) => ParseErrorKind::InvalidTagName {
                invalid,
                name: tag_name_raw.to_owned(),
            },
        })?;

        self.skip_whitespace();

        self.expect_eof()
            .with_help("tag mode (`##`) can't have any content after it")?;

        Ok(Query::Tag {
            name: Some(tag_name),
        })
    }

    /// Parses the section after `@@`.
    fn parse_name_mode(&mut self) -> Result<Query> {
        let Some(first) = self.next() else {
            return Ok(Query::Person(None));
        };

        let first_name = match first.kind {
            TokenKind::Word => {
                let first_name_raw = first.content(self.input());
                PersonName::parse(first_name_raw).map_err(|err| match err {
                    PersonNameError::Empty => {
                        unreachable!("The lexer should not return empty words")
                    }
                    PersonNameError::InvalidChar(invalid) => ParseErrorKind::InvalidPersonName {
                        invalid,
                        name: first_name_raw.into(),
                    },
                })?
            }
            TokenKind::Whitespace => {
                self.expect_eof()
                    .with_help("`@@` mode should not have any terms after the name part")?;

                return Ok(Query::Person(None));
            }
            _ => self.unexpected_t(first, ExpectedValue::Name.or(ExpectedValue::Eof))?,
        };

        let mut names = Vec::from([first_name]);
        while let Some(separator) = self.next() {
            match separator.kind {
                TokenKind::NameSeparator => {}
                TokenKind::Whitespace => break,
                TokenKind::Word => {
                    unreachable!("The lexer should not return two words in sequence")
                }
                _ => self.unexpected(
                    separator,
                    DisplayToken::NameSeparator.or(ExpectedValue::WhiteSpace),
                )?,
            }

            // Dot without a following word can be ignored
            let Some(next_word) = self.next() else { break };

            let name = match next_word.kind {
                TokenKind::Word => next_word.content(self.input()),
                TokenKind::Whitespace => break,
                _ => {
                    self.unexpected_t(next_word, ExpectedValue::Name.or(ExpectedValue::WhiteSpace))?
                }
            };

            let name = PersonName::parse(name).map_err(|err| match err {
                PersonNameError::Empty => unreachable!("The lexer should not return empty words"),
                PersonNameError::InvalidChar(invalid) => ParseErrorKind::InvalidPersonName {
                    invalid,
                    name: name.into(),
                },
            })?;

            names.push(name);
        }

        self.skip_whitespace();

        self.expect_eof()
            .with_help("`@@` mode should not have any terms after the name part")?;

        let person = match Person::new(names) {
            Ok(person) => person,
            Err(PersonError::Empty) => {
                unreachable!("We always have at least one name component at this point")
            }
        };

        Ok(Query::Person(Some(person)))
    }

    fn parse_top(&mut self) -> Result<Query> {
        self.skip_whitespace();
        let first = self.next().ok_or(ParseErrorKind::Empty)?;

        match first.kind {
            TokenKind::TagPrefix => todo!(),
            TokenKind::TagModePrefix => self.parse_tag_mode(),
            TokenKind::TagValueSeparator => todo!(),
            TokenKind::NamePrefix => todo!(),
            TokenKind::NameModePrefix => self.parse_name_mode(),
            TokenKind::GroupStart => todo!(),
            TokenKind::GroupEnd => todo!(),
            TokenKind::Or => todo!(),
            TokenKind::Not => todo!(),
            TokenKind::NameSeparator => todo!(),
            TokenKind::Whitespace => unreachable!("whitespace was already skipped"),
            TokenKind::Word => todo!(),
            TokenKind::QuotedText => todo!(),
        }
    }

    fn parse_any(&mut self) -> Result<Option<ScoreQuery>> {
        todo!()
    }
}
