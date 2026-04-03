use crate::query::{
    lexer::{Lexer, Token, TokenKind},
    Person, PersonError, PersonName, PersonNameError, Query, ScoreQuery, TagItemError,
};

type Result<T> = std::result::Result<T, ParseError>;

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

trait AddHelp<T> {
    fn with_help(self, help: impl Into<String>) -> Result<T>;
}

impl<T> AddHelp<T> for Result<T> {
    fn with_help(self, help: impl Into<String>) -> Result<T> {
        self.map_err(|e| e.with_help(help))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseErrorKind {
    UnexpectedEof { expected: String },
    UnexpectedToken { expected: String, found: String },
    InvalidTagName { invalid: char, name: String },
    InvalidPersonName { invalid: char, name: String },
    Empty,
}

impl std::fmt::Display for ParseErrorKind {
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

impl From<ParseErrorKind> for ParseError {
    fn from(value: ParseErrorKind) -> Self {
        ParseError {
            help: None,
            kind: value,
        }
    }
}

struct Completion {}

struct Parser<'a> {
    lexer: Lexer<'a>,
    cursor_pos: usize,
    completion: Option<Completion>,
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
            completion: None,
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

    fn expect(&mut self, kind: TokenKind, expected: &str) -> Result<()> {
        let Some(token) = self.next() else {
            return Err(ParseErrorKind::UnexpectedEof {
                expected: expected.into(),
            }
            .into());
        };
        if token.kind != kind {
            self.unexpected(token, expected)?;
        }
        Ok(())
    }

    fn expect_eof(&mut self, expected: &str) -> Result<()> {
        let Some(token) = self.next() else {
            return Ok(());
        };
        Err(ParseErrorKind::UnexpectedToken {
            expected: expected.into(),
            found: format!("`{}`", &self.lexer.input()[token.span.clone()]),
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

    fn token_str(&self, token: &Token) -> &str {
        &self.lexer.input()[token.span.clone()]
    }

    fn token_str_with_ticks(&self, token: &Token) -> String {
        format!("`{}`", self.token_str(token))
    }

    /// Helper for returning an unexpected token error. Always returns `Err`. Pretends to return `T` for `Ok`.
    fn unexpected_t<T>(&self, token: impl AsRef<Token>, expected: &str) -> Result<T> {
        Err(ParseErrorKind::UnexpectedToken {
            expected: expected.into(),
            found: self.token_str_with_ticks(token.as_ref()),
        }
        .into())
    }

    /// Helper for returning an unexpected token error. Always returns `Err`. Pretends to return `()` for `Ok`.
    fn unexpected(&self, token: impl AsRef<Token>, expected: &str) -> Result<()> {
        self.unexpected_t(token, expected)
    }

    /// Parses the section following a `##`.
    fn parse_tag_mode(&mut self) -> Result<Query> {
        let Some(first) = self.next() else {
            return Ok(Query::Tag { name: None });
        };

        let first = match first.kind {
            TokenKind::Word => first,
            TokenKind::Whitespace => {
                self.expect_eof("nothing")
                    .with_help("tag mode (`##`) can't have any content after it")?;

                return Ok(Query::Tag { name: None });
            }
            _ => return self.unexpected_t(first, "tag name or nothing"),
        };

        let tag_name_raw = self.token_str(&first);
        let tag_name = tag_name_raw.parse().map_err(|err| match err {
            TagItemError::Empty => unreachable!("The lexer should not return empty strings"),
            TagItemError::InvalidChar(invalid) => ParseErrorKind::InvalidTagName {
                invalid,
                name: tag_name_raw.to_owned(),
            },
        })?;

        self.skip_whitespace();

        self.expect_eof("nothing")
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
                let first_name_raw = self.token_str(&first);
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
                self.expect_eof("end of input")
                    .with_help("`@@` mode should not have any terms after the name part")?;

                return Ok(Query::Person(None));
            }
            _ => self.unexpected_t(first, "name or nothing")?,
        };

        let mut names = Vec::from([first_name]);
        while let Some(separator) = self.next() {
            match separator.kind {
                TokenKind::Dot => {}
                TokenKind::Whitespace => break,
                TokenKind::Word => {
                    unreachable!("The lexer should not return two words in sequence")
                }
                _ => self.unexpected(separator, "`.` or whitespace")?,
            }

            // Dot without a following word can be ignored
            let Some(next_word) = self.next() else { break };

            let name = match next_word.kind {
                TokenKind::Word => self.token_str(&next_word),
                TokenKind::Whitespace => break,
                _ => self.unexpected_t(next_word, "word or whitespace")?,
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

        self.expect_eof("end of input")
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
            TokenKind::TagStart => todo!(),
            TokenKind::TagMode => self.parse_tag_mode(),
            TokenKind::TagValueStart => todo!(),
            TokenKind::NameStart => todo!(),
            TokenKind::NameMode => self.parse_name_mode(),
            TokenKind::GroupStart => todo!(),
            TokenKind::GroupEnd => todo!(),
            TokenKind::Or => todo!(),
            TokenKind::Not => todo!(),
            TokenKind::Dot => todo!(),
            TokenKind::Whitespace => unreachable!("whitespace was already skipped"),
            TokenKind::Word => todo!(),
            TokenKind::RawText => todo!(),
        }
    }

    fn parse_any(&mut self) -> Result<Option<ScoreQuery>> {
        todo!()
    }
}
