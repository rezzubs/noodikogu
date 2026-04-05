mod error;
mod lexer;

use crate::query::{
    AndQuery, NotQuery, OrQuery, Person, PersonError, PersonName, PersonNameError, Query,
    ScoreQuery, SearchAtom, Tag, TagItem, TagItemError,
};
use error::{AddHelp, IntoExpected, IntoExpectedValue};
pub use error::{Context, Error, ErrorKind, Expected, ExpectedValue, Help, Result};
pub use lexer::DisplayToken;
use lexer::{Lexer, Token, TokenKind};

/// Defines a local `$macro_name!` macro that, when invoked, consumes the next
/// token and returns an [`Error::unexpected`] from the enclosing function.
///
/// Two flavours:
/// - `$macro_name!()` — plain unexpected error.
/// - `$macro_name!($help)` — unexpected error with an attached [`Help`] note.
///
/// This variant is for matches that use `peek`.
macro_rules! build_unexpected_peek {
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

/// See [`build_unexpected_peek`]
///
/// This variant is for matches that use `next`.
macro_rules! build_unexpected_next {
    ($macro_name:ident, $self:ident, $token:ident, $expected:ident) => {
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
    ($macro_name:ident, $self:ident, $token:ident, $expected:ident, $context:ident) => {
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

/// A recursive-descent parser for the catalogue query language.
///
/// Maintains up to two tokens of lookahead via an internal peek buffer so that
/// the grammar can be parsed without backtracking.
pub(crate) struct Parser<'a> {
    lexer: Lexer<'a>,
    cursor_pos: usize,
    peeked: (Option<Result<Token>>, Option<Result<Token>>),
}

impl<'a> Parser<'a> {
    /// Creates a new parser over `input` with the cursor at `cursor_pos`.
    pub fn new(input: &'a str, cursor_pos: usize) -> Self {
        Self::new_with_lexer(Lexer::new(input), cursor_pos)
    }

    /// Creates a parser from an already-constructed [`Lexer`].
    ///
    /// Useful in tests or when the caller needs to configure the lexer
    /// directly.
    pub fn new_with_lexer(lexer: Lexer<'a>, cursor_pos: usize) -> Self {
        Self {
            lexer,
            cursor_pos,
            peeked: (None, None),
        }
    }

    /// Returns a reference to the next token without consuming it.
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

    /// Consumes and returns the next token.
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

    /// Consumes the next two tokens, discarding both.
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

    /// Consumes a leading [`TokenKind::Whitespace`] token if one is present.
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

    /// Parses a complete query string and returns the top-level [`Query`] node.
    ///
    /// Dispatches to tag mode (`##`), people mode (`@@`), or score mode based
    /// on the first non-whitespace token. Returns [`ErrorKind::Empty`] if the
    /// input contains only whitespace or is empty.
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

    fn parse_tag_value(&mut self, tag_name: TagItem, group_depth: usize) -> Result<ScoreQuery> {
        let expected = ExpectedValue::TagValueExpression;
        let value = self
            .peek()?
            .ok_or(Error::unexpected_eof(expected.clone()).add_help(Help::AddTagValue))?;

        build_unexpected_peek!(unexpected_val, self, value, expected);

        match value.kind {
            TokenKind::Word => {
                let value = self.next_existing();
                let value_str = value.content(self.input());
                let value_item = TagItem::parse(value_str).map_err(|err| match err {
                    TagItemError::Empty => {
                        unreachable!("The lexer should not produce empty word tokens")
                    }
                    TagItemError::InvalidChar(invalid) => Error::new(ErrorKind::InvalidTagValue {
                        invalid,
                        name: value_str.into(),
                    }),
                })?;

                Ok(ScoreQuery::Atom(SearchAtom::Tag(Tag {
                    name: tag_name,
                    value: Some(value_item),
                })))
            }
            TokenKind::QuotedText => Err(Error::new(ErrorKind::QuotedTagValue)),

            // TODO: tag value expressions
            TokenKind::GroupStart | TokenKind::Not => {
                _ = group_depth;
                unexpected_val!()
            }

            TokenKind::Whitespace
            | TokenKind::TagPrefix
            | TokenKind::TagValueSeparator
            | TokenKind::NamePrefix
            | TokenKind::GroupEnd
            | TokenKind::Or
            | TokenKind::NameSeparator => unexpected_val!(),

            TokenKind::TagModePrefix => unexpected_val!(Help::TagModeAtStart),
            TokenKind::NameModePrefix => unexpected_val!(Help::NameModeAtStart),
        }
    }

    /// Parses a tag expression (`#name` or `#name:value`) after the `#` has
    /// been consumed.
    fn parse_tag(&mut self, group_depth: usize) -> Result<ScoreQuery> {
        _ = group_depth;

        let expected = ExpectedValue::TagName;

        let name_token = self
            .next()?
            .ok_or(Error::unexpected_eof(expected.clone()))?;

        build_unexpected_next!(unexpected_name, self, name_token, expected);

        let name = match name_token.kind {
            TokenKind::Word => {
                let name = name_token.content(self.input());
                TagItem::parse(name).map_err(|err| match err {
                    TagItemError::Empty => {
                        unreachable!("The lexer should not produce empty word tokens")
                    }
                    TagItemError::InvalidChar(invalid) => Error::new(ErrorKind::InvalidTagName {
                        name: name.into(),
                        invalid,
                    }),
                })?
            }

            TokenKind::TagValueSeparator => unexpected_name!(Help::AddTagNameBeforeValue),
            TokenKind::QuotedText => {
                return Err(Error::new(ErrorKind::QuotedTagName).add_help(Help::QuotedTagName));
            }

            TokenKind::NamePrefix
            | TokenKind::GroupStart
            | TokenKind::GroupEnd
            | TokenKind::Or
            | TokenKind::Not
            | TokenKind::NameSeparator
            | TokenKind::Whitespace => unexpected_name!(),

            TokenKind::TagPrefix => {
                unreachable!("The lexer should interpret `##` as the tag mode prefix.")
            }

            TokenKind::TagModePrefix => unexpected_name!(Help::TagModeAtStart),
            TokenKind::NameModePrefix => unexpected_name!(Help::NameModeAtStart),
        };

        let Some(separator) = self.peek()? else {
            return Ok(ScoreQuery::Atom(SearchAtom::Tag(Tag { name, value: None })));
        };

        let expected = DisplayToken::TagValueSeparator.or(ExpectedValue::WhiteSpace);
        let expected = if group_depth > 0 {
            expected.or(DisplayToken::GroupEnd)
        } else {
            expected
        };

        build_unexpected_peek!(unexpected_sep, self, separator, expected);

        match separator.kind {
            TokenKind::TagValueSeparator => {
                self.advance()?;
                self.parse_tag_value(name, group_depth)
            }
            TokenKind::Whitespace => {
                Ok(ScoreQuery::Atom(SearchAtom::Tag(Tag { name, value: None })))
            }
            TokenKind::GroupEnd if group_depth > 0 => {
                Ok(ScoreQuery::Atom(SearchAtom::Tag(Tag { name, value: None })))
            }

            TokenKind::GroupStart | TokenKind::Word | TokenKind::Not => {
                unexpected_sep!(Help::ForgottenValueSep)
            }

            TokenKind::TagPrefix
            | TokenKind::NamePrefix
            | TokenKind::Or
            | TokenKind::NameSeparator
            | TokenKind::QuotedText
            | TokenKind::GroupEnd => unexpected_sep!(),

            TokenKind::TagModePrefix => unexpected_sep!(Help::TagModeAtStart),
            TokenKind::NameModePrefix => unexpected_sep!(Help::NameModeAtStart),
        }
    }

    /// Parses a person name expression (`@Name1.Name2`) after the `@` has been
    /// consumed.
    fn parse_name(&mut self, group_depth: usize) -> Result<SearchAtom> {
        _ = group_depth;
        todo!()
    }

    /// Parses the contents of a group after the opening `(` has been consumed.
    ///
    /// Returns `None` if the group is empty (no content before the `)` or EOF),
    /// in which case the caller should skip it. Returns `Some` for non-empty
    /// groups regardless of whether the closing `)` is present — an unclosed
    /// group is accepted and its content is returned as-is.
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

    /// Parses a title search term starting from `first`, greedily joining
    /// consecutive words and quoted strings into a single
    /// [`SearchAtom::Title`].
    ///
    /// Stops at `|`, `!`, `(`, `@`, `#`, or `)` (inside a group). The parts are
    /// joined with spaces, matching how FTS5 will receive them.
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

            build_unexpected_peek!(unexpected, self, separator, expected, context);

            match separator.kind {
                TokenKind::Whitespace => {}
                // A `Word` token as a separator (no whitespace before it) can
                // only occur when the previous accumulated part ended with a
                // closing `"`, because the lexer always merges consecutive word
                // characters.
                TokenKind::Word => unexpected!(Help::SpaceAfterQuote),
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

    /// Parses a space-separated AND sequence starting from `first_query`.
    ///
    /// Called after the whitespace following the first term has already been
    /// consumed. Collects additional terms separated by whitespace and flattens
    /// nested AND nodes into a single [`ScoreQuery::And`]. If a `|` is
    /// encountered, hands off to [`Self::parse_or`] with the AND so far as the
    /// left-hand side.
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

                build_unexpected_peek!(unexpected1, self, separator, expected);

                match separator.kind {
                    TokenKind::Whitespace => {}
                    TokenKind::GroupEnd if group_depth > 0 => {
                        break;
                    }

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

            build_unexpected_peek!(unexpected2, self, next_token, expected);

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

    /// Parses a `|`-separated OR sequence starting from `first_query`.
    ///
    /// Called after the leading `|` has been consumed. Each OR item is
    /// separated by ` | ` (whitespace required on both sides). If whitespace
    /// after an OR item is followed by something other than `|`, that item and
    /// everything after it is handed to [`Self::parse_and`] — this is what
    /// gives AND higher precedence than OR. Nested OR nodes are flattened into
    /// the current sequence.
    fn parse_or(&mut self, first_query: ScoreQuery, group_depth: usize) -> Result<ScoreQuery> {
        let mut parts: Vec<OrQuery> = Vec::new();

        match first_query {
            ScoreQuery::Atom(a) => parts.push(OrQuery::Atom(a)),
            ScoreQuery::And(items) => parts.push(OrQuery::And(items)),
            ScoreQuery::Or(items) => parts.extend(items),
            ScoreQuery::Not(not) => parts.push(OrQuery::Not(not)),
        }

        // `|` was already consumed by the caller. first_run consumes the
        // required space after it; subsequent runs look for the full ` | `
        // separator and also consume the space that follows.
        let mut first_run = true;
        loop {
            if !first_run {
                // The separator between OR items is (WS + `|` + WS).
                let Some(whitespace) = self.peek()? else {
                    break;
                };

                let expected_sep = ExpectedValue::WhiteSpace;
                let expected_sep = if group_depth > 0 {
                    expected_sep.or(DisplayToken::GroupEnd)
                } else {
                    expected_sep.into_expected()
                };

                build_unexpected_peek!(unexpected_sep, self, whitespace, expected_sep);

                match whitespace.kind {
                    TokenKind::Whitespace => {
                        let Some(after_whitespace) = self.peek2()? else {
                            break; // trailing whitespace, end of OR
                        };

                        match after_whitespace.kind {
                            // Only a ` | ` continues the OR sequence.
                            TokenKind::Or => {}

                            // Anything that is not `|` should mean the starting
                            // of an AND sequence (which binds stronger). The
                            // AND sequence already handles all invalid values.
                            TokenKind::QuotedText
                            | TokenKind::Word
                            | TokenKind::TagPrefix
                            | TokenKind::TagModePrefix
                            | TokenKind::TagValueSeparator
                            | TokenKind::NamePrefix
                            | TokenKind::NameModePrefix
                            | TokenKind::GroupStart
                            | TokenKind::GroupEnd
                            | TokenKind::Not
                            | TokenKind::NameSeparator => {
                                // skip the whitespace
                                self.advance()?;

                                let and_lhs = parts.pop().expect("This is the second run and there is already one item at the start of the first").to_score();
                                let and_result = self.parse_and(and_lhs, group_depth)?;

                                match and_result {
                                    ScoreQuery::Atom(atom) => parts.push(OrQuery::Atom(atom)),
                                    ScoreQuery::And(and) => parts.push(OrQuery::And(and)),
                                    ScoreQuery::Or(or) => parts.extend(or),
                                    ScoreQuery::Not(not) => parts.push(OrQuery::Not(not)),
                                }

                                continue;
                            }

                            TokenKind::Whitespace => unreachable!("previous element is whitespace"),
                        }
                        self.advance2()?; // consume WS + `|`
                    }

                    TokenKind::GroupEnd if group_depth > 0 => break,

                    TokenKind::TagPrefix => unexpected_sep!(Help::SpaceBeforeTag),
                    TokenKind::GroupStart => unexpected_sep!(Help::SpaceBeforeGroup),
                    TokenKind::NamePrefix => unexpected_sep!(Help::SpaceBeforeName),
                    TokenKind::QuotedText => unexpected_sep!(Help::SpaceBeforeQuote),
                    TokenKind::Word => unexpected_sep!(Help::SpaceBeforeWord),
                    TokenKind::Not => unexpected_sep!(Help::SpaceBeforeNot),

                    TokenKind::TagValueSeparator
                    | TokenKind::GroupEnd
                    | TokenKind::Or
                    | TokenKind::NameSeparator => unexpected_sep!(),

                    TokenKind::NameModePrefix => unexpected_sep!(Help::NameModeAtStart),
                    TokenKind::TagModePrefix => unexpected_sep!(Help::TagModeAtStart),
                }
            }

            // Both first_run and subsequent runs must consume WS after `|`.
            let expected_ws = ExpectedValue::WhiteSpace;

            build_unexpected_peek!(unexpected_ws, self, ws_tok, expected_ws);

            match self.peek()? {
                None => return Err(Error::unexpected_eof(expected_ws)),
                Some(t) if t.kind != TokenKind::Whitespace => unexpected_ws!(Help::SpaceAfterOr),
                _ => {}
            }
            self.advance()?;

            // Parse the next OR item.
            let Some(next_token) = self.peek()? else {
                break;
            };

            let expected_item = ExpectedValue::Title
                .or(ExpectedValue::TagExpression)
                .or(ExpectedValue::NameExpression)
                .or(ExpectedValue::Group)
                .or(DisplayToken::Not);

            build_unexpected_peek!(unexpected_item, self, next_token, expected_item);

            let item = match next_token.kind {
                TokenKind::TagPrefix => {
                    self.advance()?;
                    self.parse_tag(group_depth)?
                }
                TokenKind::GroupStart => {
                    self.advance()?;
                    let Some(group) = self.parse_group(group_depth)? else {
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
                TokenKind::QuotedText | TokenKind::Word => {
                    let next_token = self.next_existing();
                    self.parse_title(next_token, group_depth)
                        .map(ScoreQuery::Atom)?
                }

                TokenKind::Or | TokenKind::TagValueSeparator | TokenKind::NameSeparator => {
                    unexpected_item!()
                }
                TokenKind::GroupEnd => unexpected_item!(Help::OrMissingItem),

                TokenKind::Whitespace => unreachable!("whitespace was just consumed"),

                TokenKind::TagModePrefix => unexpected_item!(Help::TagModeAtStart),
                TokenKind::NameModePrefix => unexpected_item!(Help::NameModeAtStart),
            };

            match item {
                ScoreQuery::Atom(a) => parts.push(OrQuery::Atom(a)),
                ScoreQuery::And(and) => parts.push(OrQuery::And(and)),
                ScoreQuery::Or(or) => parts.extend(or),
                ScoreQuery::Not(not) => parts.push(OrQuery::Not(not)),
            }

            first_run = false;
        }

        Ok(ScoreQuery::Or(parts).simplify_sequence())
    }

    /// Parses the operand of a `!` that has already been consumed.
    ///
    /// Accepts any single atom or group as the operand — whitespace between `!`
    /// and its operand is skipped. Double negation (`!(!(x))`) is flattened to
    /// `x`; `!!x` (adjacent `!` tokens without a group) is an error.
    fn parse_not(&mut self, group_depth: usize) -> Result<ScoreQuery> {
        self.skip_whitespace()?;

        let expected = ExpectedValue::Title
            .or(ExpectedValue::TagExpression)
            .or(ExpectedValue::NameExpression)
            .or(ExpectedValue::Group);

        let Some(token) = self.peek()? else {
            return Err(Error::unexpected_eof(expected));
        };

        build_unexpected_peek!(unexpected, self, token, expected);

        let inner: ScoreQuery = match token.kind {
            TokenKind::Word | TokenKind::QuotedText => {
                let token = self.next_existing();
                self.parse_title(token, group_depth).map(ScoreQuery::Atom)?
            }
            TokenKind::TagPrefix => {
                self.advance()?;
                self.parse_tag(group_depth)?
            }
            TokenKind::GroupStart => {
                self.advance()?;
                match self.parse_group(group_depth)? {
                    Some(q) => q,
                    None => return self.parse_not(group_depth),
                }
            }
            TokenKind::NamePrefix => {
                self.advance()?;
                self.parse_name(group_depth).map(ScoreQuery::Atom)?
            }
            TokenKind::Not => {
                unexpected!(Help::DoubleNegation)
            }

            TokenKind::Whitespace => unreachable!("Whitespace has been skipped"),

            TokenKind::Or
            | TokenKind::GroupEnd
            | TokenKind::TagValueSeparator
            | TokenKind::NameSeparator => unexpected!(),
            TokenKind::TagModePrefix => unexpected!(Help::TagModeAtStart),
            TokenKind::NameModePrefix => unexpected!(Help::NameModeAtStart),
        };

        let not_query = match inner {
            ScoreQuery::Atom(a) => NotQuery::Atom(a),
            ScoreQuery::And(items) => NotQuery::And(items),
            ScoreQuery::Or(items) => NotQuery::Or(items),
            // flatten the nested not
            ScoreQuery::Not(not) => match not {
                NotQuery::Atom(atom) => return Ok(ScoreQuery::Atom(atom)),
                NotQuery::And(and) => return Ok(ScoreQuery::And(and)),
                NotQuery::Or(or) => return Ok(ScoreQuery::Or(or)),
            },
        };

        Ok(ScoreQuery::Not(not_query))
    }

    /// Decides what to do after the first term and a trailing whitespace have
    /// been parsed by [`Self::parse_any`].
    ///
    /// If the next token is `|`, delegates to [`Self::parse_or`]. Otherwise
    /// delegates to [`Self::parse_and`], which handles both continued AND
    /// sequences and the case where the whitespace turns out to be trailing.
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

    /// Parses a single score-mode term: a title, tag, name, group, or `!` NOT.
    ///
    /// Does not consume any leading whitespace. Does not look for a following
    /// operator — that is left to the caller.
    fn parse_single(&mut self, group_depth: usize) -> Result<ScoreQuery> {
        let expected = ExpectedValue::Title
            .or(ExpectedValue::TagExpression)
            .or(ExpectedValue::Group);

        let Some(token) = self.next()? else {
            return Err(Error::unexpected_eof(expected));
        };

        build_unexpected_next!(unexpected, self, token, expected);

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

    /// Parses a score-mode (sub-)expression in the absence of a more specific
    /// context.
    ///
    /// Skips leading whitespace, parses the first term via [`Self::parse_single`],
    /// then peeks at the following token. A whitespace hands off to
    /// [`Self::parse_maybe_sequence`] to resolve AND vs OR; a `)` at non-zero
    /// depth returns the term as-is for the enclosing group.
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

        build_unexpected_peek!(unexpected, self, token, expected_second);

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

#[cfg(test)]
mod tests {
    use crate::query::{
        AndQuery, NotQuery, OrQuery, Person, PersonName, Query, ScoreQuery, SearchAtom, TagItem,
    };

    use super::{ErrorKind, Help, Result};

    fn parse(input: &str) -> Result<Query> {
        Query::parse(input, input.len())
    }

    fn t(s: &str) -> SearchAtom {
        SearchAtom::Title(s.to_owned())
    }

    fn atom(s: &str) -> ScoreQuery {
        ScoreQuery::Atom(t(s))
    }

    fn score(q: ScoreQuery) -> Query {
        Query::Score(q)
    }

    fn pname(s: &str) -> PersonName {
        PersonName::parse(s).unwrap()
    }

    fn titem(s: &str) -> TagItem {
        TagItem::parse(s).unwrap()
    }

    #[test]
    fn empty_input_is_error() {
        assert_eq!(parse("").unwrap_err().kind, ErrorKind::Empty);
    }

    #[test]
    fn single_word_title() {
        assert_eq!(parse("hello"), Ok(score(atom("hello"))));
    }

    #[test]
    fn multi_word_title_joined() {
        assert_eq!(parse("hello world"), Ok(score(atom("hello world"))));
    }

    #[test]
    fn three_word_title() {
        assert_eq!(
            parse("Pseudo Yoik extra"),
            Ok(score(atom("Pseudo Yoik extra")))
        );
    }

    #[test]
    fn leading_whitespace_ignored() {
        assert_eq!(parse("  hello"), Ok(score(atom("hello"))));
    }

    #[test]
    fn quoted_text_as_title() {
        assert_eq!(parse(r###""#literal""###), Ok(score(atom("#literal"))));
    }

    #[test]
    fn two_quoted_parts_joined() {
        assert_eq!(parse(r#""hello" "world""#), Ok(score(atom("hello world"))));
    }

    #[test]
    fn quoted_then_word_joined() {
        assert_eq!(parse(r#""hello" world"#), Ok(score(atom("hello world"))));
    }

    #[test]
    fn word_then_quoted_joined() {
        assert_eq!(parse(r#"hello "world""#), Ok(score(atom("hello world"))));
    }

    #[test]
    fn word_quoted_word_joined() {
        assert_eq!(
            parse(r#"hello "beautiful" world"#),
            Ok(score(atom("hello beautiful world")))
        );
    }

    #[test]
    fn multi_word_title_stops_at_or() {
        assert_eq!(
            parse("hello world | foo bar"),
            Ok(score(ScoreQuery::Or(vec![
                OrQuery::Atom(t("hello world")),
                OrQuery::Atom(t("foo bar")),
            ]))),
        );
    }

    #[test]
    fn multi_word_title_stops_at_group() {
        // title stops consuming when it sees a group start after whitespace
        assert_eq!(
            parse("hello world (foo)"),
            Ok(score(ScoreQuery::And(vec![
                AndQuery::Atom(t("hello world")),
                AndQuery::Atom(t("foo")),
            ]))),
        );
    }

    #[test]
    fn multi_word_title_stops_at_not() {
        assert_eq!(
            parse("hello world !(foo)"),
            Ok(score(ScoreQuery::And(vec![
                AndQuery::Atom(t("hello world")),
                AndQuery::Not(NotQuery::Atom(t("foo"))),
            ]))),
        );
    }

    #[test]
    fn not_applied_to_full_multi_word_title() {
        // `!` greedily takes all consecutive words as its operand
        assert_eq!(
            parse("!hello world"),
            Ok(score(ScoreQuery::Not(NotQuery::Atom(t("hello world"))))),
        );
    }

    #[test]
    fn quoted_title_in_or() {
        assert_eq!(
            parse(r#""hello" | "world""#),
            Ok(score(ScoreQuery::Or(vec![
                OrQuery::Atom(t("hello")),
                OrQuery::Atom(t("world")),
            ]))),
        );
    }

    #[test]
    fn quoted_mixed_title_in_or() {
        assert_eq!(
            parse(r#"hello "world" | foo"#),
            Ok(score(ScoreQuery::Or(vec![
                OrQuery::Atom(t("hello world")),
                OrQuery::Atom(t("foo")),
            ]))),
        );
    }

    #[test]
    fn multi_word_title_inside_group() {
        assert_eq!(parse("(hello world)"), Ok(score(atom("hello world"))));
    }

    #[test]
    fn quoted_title_inside_group() {
        assert_eq!(parse(r#"("hello world")"#), Ok(score(atom("hello world"))));
    }

    #[test]
    fn mixed_title_inside_group() {
        assert_eq!(
            parse(r#"(hello "beautiful" world)"#),
            Ok(score(atom("hello beautiful world")))
        );
    }

    #[test]
    fn group_with_multi_word_title_in_and() {
        assert_eq!(
            parse("(hello world) (foo bar)"),
            Ok(score(ScoreQuery::And(vec![
                AndQuery::Atom(t("hello world")),
                AndQuery::Atom(t("foo bar")),
            ]))),
        );
    }

    #[test]
    fn not_of_multi_word_title_in_and() {
        assert_eq!(
            parse("(hello) !(foo bar)"),
            Ok(score(ScoreQuery::And(vec![
                AndQuery::Atom(t("hello")),
                AndQuery::Not(NotQuery::Atom(t("foo bar"))),
            ]))),
        );
    }

    #[test]
    fn title_and_group_in_or_flattened() {
        assert_eq!(
            parse("hello world | (foo | bar)"),
            Ok(score(ScoreQuery::Or(vec![
                OrQuery::Atom(t("hello world")),
                OrQuery::Atom(t("foo")),
                OrQuery::Atom(t("bar")),
            ]))),
        );
    }

    #[test]
    fn single_group_unwrapped() {
        assert_eq!(parse("(hello)"), Ok(score(atom("hello"))));
    }

    #[test]
    fn nested_groups_unwrapped() {
        assert_eq!(parse("((hello))"), Ok(score(atom("hello"))));
    }

    #[test]
    fn not_word() {
        assert_eq!(
            parse("!hello"),
            Ok(score(ScoreQuery::Not(NotQuery::Atom(t("hello"))))),
        );
    }

    #[test]
    fn not_group() {
        assert_eq!(
            parse("!(hello)"),
            Ok(score(ScoreQuery::Not(NotQuery::Atom(t("hello"))))),
        );
    }

    #[test]
    fn double_not_cancels() {
        assert_eq!(parse("!(!(hello))"), Ok(score(atom("hello"))));
    }

    #[test]
    fn double_not_direct_is_error() {
        let err = parse("!!hello").unwrap_err();
        assert!(err.help.contains(&Help::DoubleNegation));
    }

    #[test]
    fn or_two_titles() {
        assert_eq!(
            parse("a | b"),
            Ok(score(ScoreQuery::Or(vec![
                OrQuery::Atom(t("a")),
                OrQuery::Atom(t("b")),
            ]))),
        );
    }

    #[test]
    fn or_three_titles_flat() {
        assert_eq!(
            parse("a | b | c"),
            Ok(score(ScoreQuery::Or(vec![
                OrQuery::Atom(t("a")),
                OrQuery::Atom(t("b")),
                OrQuery::Atom(t("c")),
            ]))),
        );
    }

    #[test]
    fn or_with_not_rhs() {
        assert_eq!(
            parse("a | !b"),
            Ok(score(ScoreQuery::Or(vec![
                OrQuery::Atom(t("a")),
                OrQuery::Not(NotQuery::Atom(t("b"))),
            ]))),
        );
    }

    #[test]
    fn and_two_groups() {
        assert_eq!(
            parse("(a) (b)"),
            Ok(score(ScoreQuery::And(vec![
                AndQuery::Atom(t("a")),
                AndQuery::Atom(t("b")),
            ]))),
        );
    }

    #[test]
    fn and_three_groups() {
        assert_eq!(
            parse("(a) (b) (c)"),
            Ok(score(ScoreQuery::And(vec![
                AndQuery::Atom(t("a")),
                AndQuery::Atom(t("b")),
                AndQuery::Atom(t("c")),
            ]))),
        );
    }

    #[test]
    fn and_title_then_group() {
        assert_eq!(
            parse("hello (world)"),
            Ok(score(ScoreQuery::And(vec![
                AndQuery::Atom(t("hello")),
                AndQuery::Atom(t("world")),
            ]))),
        );
    }

    #[test]
    fn and_with_not() {
        assert_eq!(
            parse("(a) !(b)"),
            Ok(score(ScoreQuery::And(vec![
                AndQuery::Atom(t("a")),
                AndQuery::Not(NotQuery::Atom(t("b"))),
            ]))),
        );
    }

    #[test]
    fn and_binds_tighter_than_or_lhs() {
        assert_eq!(
            parse("(a) (b) | c"),
            Ok(score(ScoreQuery::Or(vec![
                OrQuery::And(vec![AndQuery::Atom(t("a")), AndQuery::Atom(t("b"))]),
                OrQuery::Atom(t("c")),
            ]))),
        );
    }

    #[test]
    fn and_binds_tighter_than_or_rhs() {
        // a | (b) (c)  →  a | And(b, c)
        assert_eq!(
            parse("a | (b) (c)"),
            Ok(score(ScoreQuery::Or(vec![
                OrQuery::Atom(t("a")),
                OrQuery::And(vec![AndQuery::Atom(t("b")), AndQuery::Atom(t("c"))]),
            ]))),
        );
    }

    #[test]
    fn group_forces_or_inside_and() {
        assert_eq!(
            parse("(a | b) (c)"),
            Ok(score(ScoreQuery::And(vec![
                AndQuery::Or(vec![OrQuery::Atom(t("a")), OrQuery::Atom(t("b"))]),
                AndQuery::Atom(t("c")),
            ]))),
        );
    }

    #[test]
    fn not_binds_tighter_than_or() {
        assert_eq!(
            parse("a | !(b) (c)"),
            Ok(score(ScoreQuery::Or(vec![
                OrQuery::Atom(t("a")),
                OrQuery::And(vec![
                    AndQuery::Not(NotQuery::Atom(t("b"))),
                    AndQuery::Atom(t("c")),
                ]),
            ]))),
        );
    }

    #[test]
    fn tag_mode_alone() {
        assert_eq!(parse("##"), Ok(Query::Tag { name: None }));
    }

    #[test]
    fn tag_mode_trailing_space() {
        assert_eq!(parse("## "), Ok(Query::Tag { name: None }));
    }

    #[test]
    fn tag_mode_with_name() {
        assert_eq!(
            parse("##laulupidu"),
            Ok(Query::Tag {
                name: Some(titem("laulupidu"))
            }),
        );
    }

    #[test]
    fn tag_mode_with_hyphenated_name() {
        assert_eq!(
            parse("##my-tag_name"),
            Ok(Query::Tag {
                name: Some(titem("my-tag_name"))
            }),
        );
    }

    #[test]
    fn tag_mode_extra_content_is_error() {
        let err = parse("##laulupidu extra").unwrap_err();
        assert!(err.help.contains(&Help::TagModeSingleComponent));
    }

    #[test]
    fn tag_mode_invalid_char_in_name_is_error() {
        // `*` is not special to the lexer so it becomes part of the word,
        // then TagItem validation rejects it.
        let err = parse("##inva*lid").unwrap_err();
        assert_eq!(
            err.kind,
            ErrorKind::InvalidTagName {
                invalid: '*',
                name: "inva*lid".into()
            }
        );
    }

    #[test]
    fn name_mode_alone() {
        assert_eq!(parse("@@"), Ok(Query::Person(None)));
    }

    #[test]
    fn name_mode_trailing_space() {
        assert_eq!(parse("@@ "), Ok(Query::Person(None)));
    }

    #[test]
    fn name_mode_single_name() {
        assert_eq!(
            parse("@@Vettik"),
            Ok(Query::Person(Some(
                Person::new(vec![pname("Vettik")]).unwrap()
            ))),
        );
    }

    #[test]
    fn name_mode_two_components() {
        assert_eq!(
            parse("@@Vettik.Tuudur"),
            Ok(Query::Person(Some(
                Person::new(vec![pname("Vettik"), pname("Tuudur")]).unwrap()
            ))),
        );
    }

    #[test]
    fn name_mode_three_components() {
        assert_eq!(
            parse("@@First.Middle.Last"),
            Ok(Query::Person(Some(
                Person::new(vec![pname("First"), pname("Middle"), pname("Last")]).unwrap()
            ))),
        );
    }

    #[test]
    fn name_mode_trailing_dot_ignored() {
        assert_eq!(
            parse("@@Vettik."),
            Ok(Query::Person(Some(
                Person::new(vec![pname("Vettik")]).unwrap()
            ))),
        );
    }

    #[test]
    fn name_mode_extra_content_is_error() {
        let err = parse("@@Vettik extra").unwrap_err();
        assert!(err.help.contains(&Help::NameModeSingleComponent));
    }

    #[test]
    fn name_mode_invalid_char_is_error() {
        let err = parse("@@123").unwrap_err();
        assert!(matches!(err.kind, ErrorKind::InvalidPersonName { .. }));
    }

    #[test]
    fn or_of_two_and_groups() {
        assert_eq!(
            parse("(a) (b) | (c) (d)"),
            Ok(score(ScoreQuery::Or(vec![
                OrQuery::And(vec![AndQuery::Atom(t("a")), AndQuery::Atom(t("b"))]),
                OrQuery::And(vec![AndQuery::Atom(t("c")), AndQuery::Atom(t("d"))]),
            ]))),
        );
    }

    #[test]
    fn and_of_two_or_groups() {
        assert_eq!(
            parse("(a | b) (c | d)"),
            Ok(score(ScoreQuery::And(vec![
                AndQuery::Or(vec![OrQuery::Atom(t("a")), OrQuery::Atom(t("b"))]),
                AndQuery::Or(vec![OrQuery::Atom(t("c")), OrQuery::Atom(t("d"))]),
            ]))),
        );
    }

    #[test]
    fn not_strips_following_whitespace() {
        assert_eq!(
            parse("!  test"),
            Ok(score(ScoreQuery::Not(NotQuery::Atom(t("test")))))
        )
    }

    #[test]
    fn not_of_or_group() {
        assert_eq!(
            parse("!(a | b)"),
            Ok(score(ScoreQuery::Not(NotQuery::Or(vec![
                OrQuery::Atom(t("a")),
                OrQuery::Atom(t("b")),
            ])))),
        );
    }

    #[test]
    fn not_of_and_group() {
        assert_eq!(
            parse("!((a) (b))"),
            Ok(score(ScoreQuery::Not(NotQuery::And(vec![
                AndQuery::Atom(t("a")),
                AndQuery::Atom(t("b")),
            ])))),
        );
    }

    #[test]
    fn double_not_of_or_cancels() {
        assert_eq!(
            parse("!(!(a | b))"),
            Ok(score(ScoreQuery::Or(vec![
                OrQuery::Atom(t("a")),
                OrQuery::Atom(t("b")),
            ]))),
        );
    }

    #[test]
    fn not_of_or_in_and() {
        assert_eq!(
            parse("!(a | b) (c)"),
            Ok(score(ScoreQuery::And(vec![
                AndQuery::Not(NotQuery::Or(vec![
                    OrQuery::Atom(t("a")),
                    OrQuery::Atom(t("b")),
                ])),
                AndQuery::Atom(t("c")),
            ]))),
        );
    }

    #[test]
    fn or_flattened_across_groups() {
        assert_eq!(
            parse("(a | b) | c"),
            Ok(score(ScoreQuery::Or(vec![
                OrQuery::Atom(t("a")),
                OrQuery::Atom(t("b")),
                OrQuery::Atom(t("c")),
            ]))),
        );
    }

    #[test]
    fn three_level_nesting() {
        assert_eq!(
            parse("((a | b) (c))"),
            Ok(score(ScoreQuery::And(vec![
                AndQuery::Or(vec![OrQuery::Atom(t("a")), OrQuery::Atom(t("b"))]),
                AndQuery::Atom(t("c")),
            ]))),
        );
    }

    #[test]
    fn mixed_three_level() {
        assert_eq!(
            parse("a | (b | (c) (d))"),
            Ok(score(ScoreQuery::Or(vec![
                OrQuery::Atom(t("a")),
                OrQuery::Atom(t("b")),
                OrQuery::And(vec![AndQuery::Atom(t("c")), AndQuery::Atom(t("d"))]),
            ]))),
        );
    }

    #[test]
    fn and_with_not_of_and() {
        assert_eq!(
            parse("(a) !((b) (c))"),
            Ok(score(ScoreQuery::And(vec![
                AndQuery::Atom(t("a")),
                AndQuery::Not(NotQuery::And(vec![
                    AndQuery::Atom(t("b")),
                    AndQuery::Atom(t("c")),
                ])),
            ]))),
        );
    }

    #[test]
    fn word_directly_after_quoted_requires_space() {
        // `a "b"c` — word after closing quote with no space
        let err = parse(r###"a "b"c"###).unwrap_err();
        assert!(err.help.contains(&Help::SpaceAfterQuote));
    }

    #[test]
    fn empty_group_skipped_in_and_sequence() {
        // (a) () (b) — the empty group should be ignored, giving And(a, b)
        assert_eq!(
            parse("(a) () (b)"),
            Ok(score(ScoreQuery::And(vec![
                AndQuery::Atom(t("a")),
                AndQuery::Atom(t("b")),
            ]))),
        );
    }

    #[test]
    fn empty_quoted_string_is_error() {
        let err = parse(r#""""#).unwrap_err();
        assert_eq!(err.kind, ErrorKind::EmptyQuotedString);
    }

    #[test]
    fn or_without_space_before_is_error() {
        let err = parse("a|b").unwrap_err();
        assert!(err.help.contains(&Help::SpaceBeforeOr));
    }

    #[test]
    fn or_without_space_after_is_error() {
        let err = parse("a |b").unwrap_err();
        assert!(err.help.contains(&Help::SpaceAfterOr));
    }

    #[test]
    fn not_without_operand_is_error() {
        let err = parse("!").unwrap_err();
        assert!(matches!(err.kind, ErrorKind::UnexpectedEof { .. }));
    }

    #[test]
    fn tag_mode_mid_query_is_error() {
        let err = parse("hello ##world").unwrap_err();
        assert!(err.help.contains(&Help::TagModeAtStart));
    }

    #[test]
    fn name_mode_mid_query_is_error() {
        let err = parse("hello @@world").unwrap_err();
        assert!(err.help.contains(&Help::NameModeAtStart));
    }
}
