mod error;
mod lexer;

use crate::query::{
    Person, PersonError, PersonName, PersonNameError, Query, ScoreQuery, TagItemError,
};
use error::{AddHelp, IntoExpected, IntoExpectedValue};
pub use error::{Error, ErrorKind, Expected, ExpectedValue, Help, Result};
pub use lexer::DisplayToken;
use lexer::{Lexer, Token, TokenKind};

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
            return Err(Error::unexpected_eof(expected));
        };
        if token.kind != kind {
            return Err(Error::unexpected(expected, token.display(self.input())));
        }
        Ok(())
    }

    fn expect_eof(&mut self) -> Result<()> {
        let Some(token) = self.next() else {
            return Ok(());
        };
        Err(Error::unexpected(
            ExpectedValue::Eof,
            token.display(self.input()),
        ))
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

    /// Parses the section following a `##`.
    fn parse_tag_mode(&mut self) -> Result<Query> {
        let Some(first) = self.next() else {
            return Ok(Query::Tag { name: None });
        };

        let tag_name_raw = match first.kind {
            TokenKind::Word => first.content(self.input()),
            TokenKind::Whitespace => {
                self.expect_eof().add_help(Help::TagModeSingleComponent)?;

                return Ok(Query::Tag { name: None });
            }
            _ => {
                return Err(Error::unexpected(
                    ExpectedValue::TagName.or(ExpectedValue::Eof),
                    first.display(self.input()),
                ));
            }
        };

        let tag_name = tag_name_raw.parse().map_err(|err| match err {
            TagItemError::Empty => unreachable!("The lexer should not return empty strings"),
            TagItemError::InvalidChar(invalid) => Error::invalid_tag_name(invalid, tag_name_raw),
        })?;

        self.skip_whitespace();

        self.expect_eof().add_help(Help::TagModeSingleComponent)?;

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
                    PersonNameError::InvalidChar(invalid) => {
                        Error::invalid_person_name(invalid, first_name_raw)
                    }
                })?
            }
            TokenKind::Whitespace => {
                self.expect_eof().add_help(Help::NameModeSingleComponent)?;

                return Ok(Query::Person(None));
            }
            _ => {
                return Err(Error::unexpected(
                    ExpectedValue::Name.or(ExpectedValue::Eof),
                    first.display(self.input()),
                ));
            }
        };

        let mut names = Vec::from([first_name]);
        while let Some(separator) = self.next() {
            match separator.kind {
                TokenKind::NameSeparator => {}
                TokenKind::Whitespace => break,
                TokenKind::Word => {
                    unreachable!("The lexer should not return two words in sequence")
                }
                _ => {
                    return Err(Error::unexpected(
                        DisplayToken::NameSeparator.or(ExpectedValue::WhiteSpace),
                        separator.display(self.input()),
                    ));
                }
            }

            // Dot without a following word can be ignored
            let Some(next_word) = self.next() else { break };

            let name = match next_word.kind {
                TokenKind::Word => next_word.content(self.input()),
                TokenKind::Whitespace => break,
                _ => {
                    return Err(Error::unexpected(
                        ExpectedValue::Name.or(ExpectedValue::WhiteSpace),
                        next_word.display(self.input()),
                    ));
                }
            };

            let name = PersonName::parse(name).map_err(|err| match err {
                PersonNameError::Empty => unreachable!("The lexer should not return empty words"),
                PersonNameError::InvalidChar(invalid) => Error::invalid_person_name(invalid, name),
            })?;

            names.push(name);
        }

        self.skip_whitespace();

        self.expect_eof().add_help(Help::NameModeSingleComponent)?;

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
        let first = self.next().ok_or(Error::empty())?;

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
