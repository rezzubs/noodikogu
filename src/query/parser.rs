mod error;
mod lexer;

use crate::query::{
    AndQuery, Person, PersonError, PersonName, PersonNameError, Query, ScoreQuery, SearchAtom,
    TagItemError,
};
use error::{AddHelp, IntoExpected, IntoExpectedValue};
pub use error::{Context, Error, ErrorKind, Expected, ExpectedValue, Help, Result};
pub use lexer::DisplayToken;
use lexer::{Lexer, Token, TokenKind};

pub(crate) struct Parser<'a> {
    lexer: Lexer<'a>,
    cursor_pos: usize,
    peeked: (Option<Result<Token>>, Option<Result<Token>>),
}

macro_rules! build_unexpected {
    ($macro_name:ident, $self:ident, $token:ident, $expected:ident, peek) => {
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
    ($macro_name:ident, $self:ident, $token:ident, $expected:ident, $context:ident, peek) => {
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
    ($macro_name:ident, $self:ident, $token:ident, $expected:ident, nopeek) => {
        macro_rules! $macro_name {
            () => {{
                return Err(Error::unexpected($expected, $token.display($self.input())));
            }};
            ($help:expr) => {{
                return Err(
                    Error::unexpected($expected, $token.display($self.input())).add_help($help)
                );
            }};
        }
    };
    ($macro_name:ident, $self:ident, $token:ident, $expected:ident, $context:ident, nopeek) => {
        macro_rules! $macro_name {
            () => {{
                return Err(Error::unexpected($expected, $token.display($self.input()))
                    .add_context($context));
            }};
            ($help:expr) => {{
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

    fn peek(&mut self) -> Result<Option<&Token>> {
        if self.peeked.0.is_none() {
            self.peeked.0 = self.lexer.next().map(|r| r.map_err(Into::into));
        }
        match &self.peeked.0 {
            None => Ok(None),
            Some(Ok(t)) => Ok(Some(t)),
            Some(Err(e)) => Err(e.clone()),
        }
    }

    /// Peeks at the second next token without consuming either.
    fn peek2(&mut self) -> Result<Option<&Token>> {
        if self.peeked.0.is_none() {
            self.peeked.0 = self.lexer.next().map(|r| r.map_err(Into::into));
        }
        if self.peeked.1.is_none() {
            self.peeked.1 = self.lexer.next().map(|r| r.map_err(Into::into));
        }
        match &self.peeked.1 {
            None => Ok(None),
            Some(Ok(t)) => Ok(Some(t)),
            Some(Err(e)) => Err(e.clone()),
        }
    }

    fn next(&mut self) -> Result<Option<Token>> {
        let result = if let Some(result) = self.peeked.0.take() {
            self.peeked.0 = self.peeked.1.take();
            result
        } else {
            return self.lexer.next().transpose().map_err(Into::into);
        };
        result.map(Some)
    }

    /// Call next when it's known to exist. Like after a peek.
    fn next_existing(&mut self) -> Token {
        self.next()
            .expect("known to not be an error")
            .expect("known to exist")
    }

    /// Like next but doesn't return anything.
    fn advance(&mut self) -> Result<()> {
        self.next().map(|_| ())
    }

    fn advance2(&mut self) -> Result<()> {
        self.next()?;
        self.next().map(|_| ())
    }

    /// Return the input of the lexer.
    fn input(&self) -> &'a str {
        self.lexer.input()
    }

    /// Check if the next token has the given kind and error if not.
    fn expect(&mut self, kind: TokenKind, expected: impl IntoExpected) -> Result<()> {
        let expected = expected.into_expected();
        let Some(token) = self.next()? else {
            return Err(Error::unexpected_eof(expected));
        };
        if token.kind != kind {
            return Err(Error::unexpected(expected, token.display(self.input())));
        }
        Ok(())
    }

    /// Check if the next token is EOF and error if not.
    fn expect_eof(&mut self) -> Result<()> {
        let Some(token) = self.next()? else {
            return Ok(());
        };
        Err(Error::unexpected(
            ExpectedValue::Eof,
            token.display(self.input()),
        ))
    }

    fn skip_whitespace(&mut self) -> Result<()> {
        if self
            .peek()?
            .is_some_and(|t| t.kind == TokenKind::Whitespace)
        {
            self.advance()?;
            debug_assert!(
                !self
                    .peek()?
                    .is_some_and(|t| t.kind == TokenKind::Whitespace),
                "consecutive whitespace tokens are impossible"
            );
        }
        Ok(())
    }

    /// Parses the section following a `##`.
    fn parse_tag_mode(&mut self) -> Result<Query> {
        let Some(first) = self.next()? else {
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

        self.skip_whitespace()?;

        self.expect_eof().add_help(Help::TagModeSingleComponent)?;

        Ok(Query::Tag {
            name: Some(tag_name),
        })
    }

    /// Parses the section after `@@`.
    fn parse_name_mode(&mut self) -> Result<Query> {
        let Some(first) = self.next()? else {
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
        while let Some(separator) = self.next()? {
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
            let Some(next_word) = self.next()? else { break };

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

        self.skip_whitespace()?;

        self.expect_eof().add_help(Help::NameModeSingleComponent)?;

        let person = match Person::new(names) {
            Ok(person) => person,
            Err(PersonError::Empty) => {
                unreachable!("We always have at least one name component at this point")
            }
        };

        Ok(Query::Person(Some(person)))
    }

    pub(crate) fn parse_top(&mut self) -> Result<Query> {
        self.skip_whitespace()?;
        let first = self.peek()?.ok_or(Error::empty())?;

        match first.kind {
            TokenKind::TagModePrefix => {
                self.advance()?;
                self.parse_tag_mode()
            }
            TokenKind::NameModePrefix => {
                self.advance()?;
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

    /// Parse the section after a `(`.
    fn parse_group(&mut self, group_depth: usize) -> Result<Option<ScoreQuery>> {
        let Some(peeked) = self.peek()? else {
            // expecting that unclosed groups will close at EOF.
            return Ok(None);
        };

        if let TokenKind::GroupEnd = peeked.kind {
            self.advance()?;
            return Ok(None);
        }

        let query = self.parse_any(group_depth + 1)?;

        self.skip_whitespace()?;
        let Some(next) = self.next()? else {
            // expecting that unclosed groups will close at EOF.
            return Ok(Some(query));
        };

        match next.kind {
            TokenKind::GroupEnd => {}

            TokenKind::Whitespace => unreachable!("skipped whitespace above"),
            TokenKind::TagPrefix
            | TokenKind::TagValueSeparator
            | TokenKind::NamePrefix
            | TokenKind::GroupStart
            | TokenKind::Or
            | TokenKind::Not
            | TokenKind::NameSeparator
            | TokenKind::Word
            | TokenKind::QuotedText
            | TokenKind::TagModePrefix
            | TokenKind::NameModePrefix => {
                unreachable!(
                    "subparsers should be greedy enough to consume all of these, got {:?}",
                    next.kind
                );
            }
        }

        Ok(Some(query))
    }

    fn parse_title(&mut self, first: Token, group_depth: usize) -> Result<SearchAtom> {
        debug_assert!(matches!(
            first.kind,
            TokenKind::Word | TokenKind::QuotedText
        ));

        let text = first.content(self.input());
        debug_assert!(!text.is_empty(), "the lexer should not return empty text");

        let mut parts = Vec::from([text]);

        while let Some(separator) = self.peek()? {
            let context = Context::EndOfTitle;

            let expected = ExpectedValue::WhiteSpace;
            let expected = if group_depth > 0 {
                expected.or(DisplayToken::GroupEnd)
            } else {
                expected.into_expected()
            };

            build_unexpected!(unexpected, self, separator, expected, context, peek);

            match separator.kind {
                TokenKind::Whitespace => {}
                TokenKind::Word if first.kind == TokenKind::QuotedText => {
                    unexpected!(Help::SpaceAfterQuote)
                }
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

            let Some(next_token) = self.peek2()? else {
                break;
            };

            let next_part = match next_token.kind {
                TokenKind::Word => {
                    self.advance()?;
                    self.next_existing().content(self.input())
                }
                TokenKind::QuotedText => {
                    self.advance()?;
                    self.next_existing().content(self.input())
                }

                TokenKind::GroupEnd if group_depth > 0 => {
                    // skip only the whitespace and leave the `)` for the group parser.
                    self.advance()?;
                    break;
                }
                TokenKind::NamePrefix
                | TokenKind::TagPrefix
                | TokenKind::GroupStart
                | TokenKind::Or
                | TokenKind::Not => {
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

    fn parse_and(&mut self, first_query: ScoreQuery, group_depth: usize) -> Result<ScoreQuery> {
        let mut parts = Vec::new();

        match first_query {
            ScoreQuery::Atom(search_atom) => parts.push(AndQuery::Atom(search_atom)),
            ScoreQuery::And(items) => parts.extend(items),
            ScoreQuery::Or(items) => parts.push(AndQuery::Or(items)),
            ScoreQuery::Not(not_query) => parts.push(AndQuery::Not(not_query)),
        }

        let mut first_run = true;
        loop {
            if !first_run {
                // the first run has already consumed a whitespace token.

                let Some(separator) = self.peek()? else {
                    break;
                };

                let expected = ExpectedValue::WhiteSpace;
                let expected = if group_depth > 0 {
                    expected.or(DisplayToken::GroupEnd)
                } else {
                    expected.into_expected()
                };

                build_unexpected!(unexpected1, self, separator, expected, peek);

                match separator.kind {
                    TokenKind::Whitespace => {}

                    TokenKind::TagPrefix => unexpected1!(Help::SpaceBeforeTag),
                    TokenKind::GroupStart => unexpected1!(Help::SpaceBeforeGroup),
                    TokenKind::NamePrefix => unexpected1!(Help::SpaceBeforeName),
                    TokenKind::QuotedText => unexpected1!(Help::SpaceBeforeQuote),
                    TokenKind::Word => unexpected1!(Help::SpaceBeforeWord),
                    TokenKind::Or => unexpected1!(Help::SpaceBeforeOr),
                    TokenKind::Not => unexpected1!(Help::SpaceBeforeNot),

                    TokenKind::TagValueSeparator
                    | TokenKind::GroupEnd
                    | TokenKind::NameSeparator => unexpected1!(),

                    TokenKind::NameModePrefix => unexpected1!(Help::NameModeAtStart),
                    TokenKind::TagModePrefix => unexpected1!(Help::TagModeAtStart),
                }

                self.advance()?;
            }

            let Some(next_token) = self.peek()? else {
                break;
            };

            let expected = ExpectedValue::TagExpression
                .or(ExpectedValue::NameExpression)
                .or(ExpectedValue::Group)
                .or(DisplayToken::Not)
                .or(DisplayToken::Or);

            let expected = if group_depth > 0 {
                expected.or(ExpectedValue::Group)
            } else {
                expected
            };

            build_unexpected!(unexpected2, self, next_token, expected, peek);

            let item = match next_token.kind {
                TokenKind::TagPrefix => {
                    self.advance()?;
                    self.parse_tag(group_depth)?
                }
                TokenKind::GroupStart => {
                    self.advance()?;
                    let Some(group) = self.parse_group(group_depth)? else {
                        // The group is empty, skip it and any whitespace after it.
                        self.skip_whitespace()?;
                        continue;
                    };
                    group
                }
                TokenKind::NamePrefix => {
                    self.advance()?;
                    self.parse_name(group_depth).map(ScoreQuery::Atom)?
                }
                TokenKind::Not => {
                    self.advance()?;
                    self.parse_not(group_depth)?
                }
                TokenKind::Or => {
                    self.advance()?;
                    let query_so_far = ScoreQuery::And(parts).simplify_sequence();

                    return self.parse_or(query_so_far, group_depth);
                }
                TokenKind::QuotedText | TokenKind::Word => {
                    let next_token = self.next_existing();

                    self.parse_title(next_token, group_depth)
                        .map(ScoreQuery::Atom)?
                }

                TokenKind::GroupEnd if group_depth > 0 => break,

                TokenKind::TagValueSeparator | TokenKind::NameSeparator | TokenKind::GroupEnd => {
                    unexpected2!()
                }

                TokenKind::Whitespace => unreachable!("the previous token was whitespace"),

                TokenKind::TagModePrefix => unexpected2!(Help::TagModeAtStart),
                TokenKind::NameModePrefix => unexpected2!(Help::NameModeAtStart),
            };

            match item {
                ScoreQuery::Atom(atom) => parts.push(AndQuery::Atom(atom)),
                ScoreQuery::And(and) => parts.extend(and),
                ScoreQuery::Or(or) => parts.push(AndQuery::Or(or)),
                ScoreQuery::Not(not) => parts.push(AndQuery::Not(not)),
            }

            first_run = false;
        }

        Ok(ScoreQuery::And(parts).simplify_sequence())
    }

    fn parse_or(&self, first_query: ScoreQuery, group_depth: usize) -> Result<ScoreQuery> {
        _ = first_query;
        _ = group_depth;
        todo!();
    }

    fn parse_not(&self, group_depth: usize) -> Result<ScoreQuery> {
        _ = group_depth;
        todo!();
    }

    /// Confirmed a whitespace after a the first toke in [`Self::parse_any`].
    fn parse_maybe_sequence(
        &mut self,
        first_query: ScoreQuery,
        group_depth: usize,
    ) -> Result<ScoreQuery> {
        _ = first_query;
        _ = group_depth;

        let Some(next) = self.peek()? else {
            return Ok(first_query);
        };

        match next.kind {
            TokenKind::Or => {
                self.advance()?;
                self.parse_or(first_query, group_depth)
            }
            TokenKind::Whitespace => unreachable!(
                "There was a whitespace before this and the lexer doesn't return consequtive whitespaces"
            ),
            TokenKind::GroupEnd if group_depth > 0 => Ok(first_query),

            TokenKind::TagPrefix
            | TokenKind::TagModePrefix
            | TokenKind::TagValueSeparator
            | TokenKind::NamePrefix
            | TokenKind::NameModePrefix
            | TokenKind::GroupStart
            | TokenKind::GroupEnd
            | TokenKind::Not
            | TokenKind::NameSeparator
            | TokenKind::Word
            | TokenKind::QuotedText => self.parse_and(first_query, group_depth),
        }
    }

    fn parse_single(&mut self, group_depth: usize) -> Result<ScoreQuery> {
        let expected = ExpectedValue::Title
            .or(ExpectedValue::TagExpression)
            .or(ExpectedValue::Group);

        let Some(token) = self.next()? else {
            return Err(Error::unexpected_eof(expected));
        };

        build_unexpected!(unexpected, self, token, expected, nopeek);

        match token.kind {
            TokenKind::TagPrefix => self.parse_tag(group_depth),
            TokenKind::NamePrefix => self.parse_name(group_depth).map(ScoreQuery::Atom),
            TokenKind::GroupStart => {
                let Some(group) = self.parse_group(group_depth)? else {
                    return self.parse_any(group_depth);
                };
                Ok(group)
            }
            TokenKind::Word | TokenKind::QuotedText => {
                self.parse_title(token, group_depth).map(ScoreQuery::Atom)
            }
            TokenKind::Not => self.parse_not(group_depth),

            TokenKind::Whitespace => unreachable!("we skipped whitespace"),

            TokenKind::GroupEnd if group_depth > 0 => unreachable!(
                "The group parser should check that the group isn't immediately closed"
            ),

            TokenKind::GroupEnd
            | TokenKind::TagValueSeparator
            | TokenKind::Or
            | TokenKind::NameSeparator => unexpected!(),

            TokenKind::TagModePrefix => unexpected!(Help::TagModeAtStart),
            TokenKind::NameModePrefix => unexpected!(Help::NameModeAtStart),
        }
    }

    /// Default parser when a more specific context doesn't exist.
    fn parse_any(&mut self, group_depth: usize) -> Result<ScoreQuery> {
        self.skip_whitespace()?;

        let first_query = self.parse_single(group_depth)?;

        let Some(token) = self.peek()? else {
            return Ok(first_query);
        };

        let expected = ExpectedValue::WhiteSpace;
        let expected_second = if group_depth > 0 {
            expected.or(DisplayToken::GroupEnd)
        } else {
            expected.into_expected()
        };

        build_unexpected!(unexpected, self, token, expected_second, peek);

        match token.kind {
            TokenKind::Whitespace => {
                self.advance()?;
                self.parse_maybe_sequence(first_query, group_depth)
            }

            // Leave GroupEnd in the stream for parse_group to consume.
            TokenKind::GroupEnd if group_depth > 0 => Ok(first_query),

            TokenKind::TagPrefix => unexpected!(Help::SpaceBeforeTag),
            TokenKind::GroupStart => unexpected!(Help::SpaceBeforeGroup),
            TokenKind::NamePrefix => unexpected!(Help::SpaceBeforeName),
            TokenKind::QuotedText => unexpected!(Help::SpaceBeforeQuote),
            TokenKind::Or => unexpected!(Help::SpaceBeforeOr),
            TokenKind::Not => unexpected!(Help::SpaceBeforeNot),

            TokenKind::GroupEnd
            | TokenKind::NameSeparator
            | TokenKind::Word
            | TokenKind::TagValueSeparator => unexpected!(),

            TokenKind::TagModePrefix => unexpected!(Help::TagModeAtStart),
            TokenKind::NameModePrefix => unexpected!(Help::NameModeAtStart),
        }
    }
}
