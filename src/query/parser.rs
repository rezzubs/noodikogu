mod error;
mod lexer;

use crate::query::{
    Person, PersonError, PersonName, PersonNameError, Query, ScoreQuery, SearchAtom, TagItemError,
};
use error::{AddHelp, IntoExpected, IntoExpectedValue};
pub use error::{Context, Error, ErrorKind, Expected, ExpectedValue, Help, Result};
pub use lexer::DisplayToken;
use lexer::{Lexer, Token, TokenKind};

struct Parser<'a> {
    lexer: Lexer<'a>,
    cursor_pos: usize,
    peeked: (Option<Token>, Option<Token>),
}

macro_rules! build_unexpected {
    ($macro_name:ident, $self:ident, $token:ident, $expected:ident) => {
        macro_rules! $macro_name {
            () => {{
                let $token = $self.next_existing();
                return Err(Error::unexpected($expected, $token.display($self.input())));
            }};
            ($help:expr) => {{
                let $token = $self.next_existing();
                return Err(
                    Error::unexpected($expected, $token.display($self.input())).add_help($help)
                );
            }};
        }
    };
    ($macro_name:ident, $self:ident, $token:ident, $expected:ident, $context:ident) => {
        macro_rules! $macro_name {
            () => {{
                let $token = $self.next_existing();
                return Err(Error::unexpected($expected, $token.display($self.input()))
                    .add_context($context));
            }};
            ($help:expr) => {{
                let $token = $self.next_existing();
                return Err(Error::unexpected($expected, $token.display($self.input()))
                    .add_help($help)
                    .add_context($context));
            }};
        }
    };
}

impl<'a> Parser<'a> {
    pub fn new(input: &'a str, cursor_pos: usize) -> Self {
        Self::new_with_lexer(Lexer::new(input), cursor_pos)
    }

    pub fn new_with_lexer(lexer: Lexer<'a>, cursor_pos: usize) -> Self {
        Self {
            lexer,
            cursor_pos,
            peeked: (None, None),
        }
    }

    fn peek(&mut self) -> Option<&Token> {
        if self.peeked.0.is_none() {
            self.peeked.0 = self.lexer.next();
        }
        self.peeked.0.as_ref()
    }

    /// Peeks at the second next token without consuming either.
    fn peek2(&mut self) -> Option<&Token> {
        if self.peeked.0.is_none() {
            self.peeked.0 = self.lexer.next();
        }
        if self.peeked.1.is_none() {
            self.peeked.1 = self.lexer.next();
        }
        self.peeked.1.as_ref()
    }

    fn next(&mut self) -> Option<Token> {
        if let Some(token) = self.peeked.0.take() {
            self.peeked.0 = self.peeked.1.take();
            return Some(token);
        }

        self.lexer.next()
    }

    /// Call next when it's known to exist. Like after a peek.
    fn next_existing(&mut self) -> Token {
        self.next().expect("known to exist")
    }

    /// Like next but donesn't return anything.
    fn advance(&mut self) {
        self.next();
    }

    fn advance2(&mut self) {
        self.next();
        self.next();
    }

    /// Return the input of the lexer.
    fn input(&self) -> &'a str {
        self.lexer.input()
    }

    /// Check if the next token has the given kind and error if not.
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

    /// Check if the next token is EOF and error if not.
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
        if self.peek().is_some_and(|t| t.kind == TokenKind::Whitespace) {
            self.advance();
            debug_assert!(
                !self.peek().is_some_and(|t| t.kind == TokenKind::Whitespace),
                "consecutive whitespace tokens are impossible"
            );
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
                    ExpectedValue::NameSegment.or(ExpectedValue::Eof),
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
                        ExpectedValue::NameSegment.or(ExpectedValue::WhiteSpace),
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
        let first = self.peek().ok_or(Error::empty())?;

        match first.kind {
            TokenKind::TagModePrefix => {
                self.advance();
                self.parse_tag_mode()
            }
            TokenKind::NameModePrefix => {
                self.advance();
                self.parse_name_mode()
            }
            TokenKind::Whitespace => unreachable!("whitespace was already skipped"),
            _ => self.parse_any(0).map(Query::Score),
        }
    }

    fn parse_tag(&mut self, group_depth: usize) -> Result<ScoreQuery> {
        _ = group_depth;
        todo!()
    }

    fn parse_name(&mut self, group_depth: usize) -> Result<SearchAtom> {
        _ = group_depth;
        todo!()
    }

    fn parse_group(&mut self, group_depth: usize) -> Result<Option<ScoreQuery>> {
        _ = group_depth;
        todo!()
    }

    fn parse_title(&mut self, first: Token, group_depth: usize) -> Result<SearchAtom> {
        debug_assert!(matches!(
            first.kind,
            TokenKind::Word | TokenKind::QuotedText
        ));

        let text = first.content(self.input());
        debug_assert!(!text.is_empty(), "the lexer should not return empty text");

        let mut parts = Vec::from([text]);

        while let Some(separator) = self.peek() {
            let context = Context::EndOfTitle;

            let expected = ExpectedValue::WhiteSpace;
            let expected = if group_depth > 0 {
                expected.or(DisplayToken::GroupEnd)
            } else {
                expected.into_expected()
            };

            build_unexpected!(unexpected, self, separator, expected, context);

            match separator.kind {
                TokenKind::Whitespace => {}
                TokenKind::Word => unreachable!("The lexer should merge consecutive words."),
                TokenKind::GroupEnd if group_depth > 0 => break,

                TokenKind::TagPrefix => unexpected!(Help::SpaceBeforeTag),
                TokenKind::GroupStart => unexpected!(Help::SpaceBeforeGroup),
                TokenKind::NamePrefix => unexpected!(Help::SpaceBeforeName),
                TokenKind::QuotedText => unexpected!(Help::SpaceBeforeQuote),
                TokenKind::Or => unexpected!(Help::SpaceBeforeOr),
                TokenKind::Not => unexpected!(Help::SpaceBeforeNot),

                TokenKind::GroupEnd | TokenKind::TagValueSeparator | TokenKind::NameSeparator => {
                    unexpected!()
                }

                TokenKind::TagModePrefix => unexpected!(Help::TagModeAtStart),
                TokenKind::NameModePrefix => unexpected!(Help::NameModeAtStart),
            }

            let Some(next_token) = self.peek2() else {
                break;
            };

            let next_part = match next_token.kind {
                TokenKind::Word => {
                    self.advance();
                    self.next_existing().content(self.input())
                }
                TokenKind::QuotedText => {
                    self.advance();
                    self.next_existing().content(self.input())
                }

                TokenKind::GroupEnd if group_depth > 0 => {
                    // skip only the whitespace and leave the `)` for the group parser.
                    self.advance();
                    break;
                }
                TokenKind::NamePrefix
                | TokenKind::TagPrefix
                | TokenKind::GroupStart
                | TokenKind::Or
                | TokenKind::Not => {
                    // skip only the whitespace and leave the rest for other parsers.
                    self.advance();
                    break;
                }

                TokenKind::Whitespace => {
                    unreachable!("The lexer should merge consequtive whitespace.")
                }

                TokenKind::NameSeparator | TokenKind::TagValueSeparator | TokenKind::GroupEnd => {
                    unexpected!()
                }

                TokenKind::TagModePrefix => unexpected!(Help::TagModeAtStart),
                TokenKind::NameModePrefix => unexpected!(Help::NameModeAtStart),
            };

            parts.push(next_part);
        }

        let full_title = parts.join(" ");

        Ok(SearchAtom::Title(full_title))
    }

    fn parse_sequence(
        &mut self,
        first_query: ScoreQuery,
        group_depth: usize,
    ) -> Result<ScoreQuery> {
        _ = first_query;
        _ = group_depth;
        todo!()
    }

    /// Default parser when a more specific context doesn't exist.
    fn parse_any(&mut self, group_depth: usize) -> Result<ScoreQuery> {
        self.skip_whitespace();
        let expected_first = ExpectedValue::Title
            .or(ExpectedValue::TagExpression)
            .or(ExpectedValue::Group);

        let Some(first_token) = self.next() else {
            return Err(Error::unexpected_eof(expected_first));
        };

        build_unexpected!(unexpected1, self, first_token, expected_first);

        let first_query = match first_token.kind {
            TokenKind::TagPrefix => self.parse_tag(group_depth)?,
            TokenKind::NamePrefix => {
                let atom = self.parse_name(group_depth)?;
                ScoreQuery::Atom(atom)
            }
            TokenKind::GroupStart => {
                let Some(group) = self.parse_group(group_depth)? else {
                    return self.parse_any(group_depth);
                };
                group
            }
            TokenKind::Word | TokenKind::QuotedText => {
                let atom = self.parse_title(first_token, group_depth)?;
                ScoreQuery::Atom(atom)
            }

            TokenKind::Whitespace => unreachable!("we skipped whitespace"),

            TokenKind::GroupEnd if group_depth > 0 => unreachable!(
                "The group parser should check that the group isn't immediately closed"
            ),

            TokenKind::GroupEnd
            | TokenKind::TagValueSeparator
            | TokenKind::Or
            | TokenKind::Not
            | TokenKind::NameSeparator => unexpected1!(),

            TokenKind::TagModePrefix => unexpected1!(Help::TagModeAtStart),
            TokenKind::NameModePrefix => unexpected1!(Help::NameModeAtStart),
        };

        let Some(second_token) = self.peek() else {
            return Ok(first_query);
        };

        let expected_second = ExpectedValue::WhiteSpace;
        let expected_second = if group_depth > 0 {
            expected_second.or(DisplayToken::GroupEnd)
        } else {
            expected_second.into_expected()
        };

        build_unexpected!(unexpected2, self, second_token, expected_second);

        match second_token.kind {
            TokenKind::Whitespace => return self.parse_sequence(first_query, group_depth),

            TokenKind::GroupEnd if group_depth > 0 => return Ok(first_query),

            TokenKind::TagPrefix => unexpected2!(Help::SpaceBeforeTag),
            TokenKind::GroupStart => unexpected2!(Help::SpaceBeforeGroup),
            TokenKind::NamePrefix => unexpected2!(Help::SpaceBeforeName),
            TokenKind::QuotedText => unexpected2!(Help::SpaceBeforeQuote),
            TokenKind::Or => unexpected2!(Help::SpaceBeforeOr),
            TokenKind::Not => unexpected2!(Help::SpaceBeforeNot),

            TokenKind::GroupEnd
            | TokenKind::NameSeparator
            | TokenKind::Word
            | TokenKind::TagValueSeparator => unexpected2!(),

            TokenKind::TagModePrefix => unexpected2!(Help::TagModeAtStart),
            TokenKind::NameModePrefix => unexpected2!(Help::NameModeAtStart),
        }
    }
}
