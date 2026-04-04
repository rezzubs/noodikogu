//! Query types for the score catalogue search language.
//!
//! # AST invariants
//!
//! The boolean query types ([`ScoreQuery`], [`AndQuery`], [`OrQuery`],
//! [`NotQuery`]) enforce a normalized form so that consumers (e.g. the SQL
//! generator) never have to handle redundant structure:
//!
//! - **Flat sequences.** AND and OR nodes hold a `Vec` of their operands rather
//!   than a binary pair, so `A & B & C` is `And([A, B, C])` instead of `And(A,
//!   And(B, C))`. As a consequence, an AND node cannot appear inside another AND
//!   node, and likewise for OR — there is no such variant in [`AndQuery`] or
//!   [`OrQuery`]. The parser is responsible for flattening consecutive
//!   same-operator terms.
//!
//! - **No double negation.** [`NotQuery`] has no `Not` variant, making `!(!x)`
//!   unrepresentable. The parser simplifies double negations away before
//!   constructing the AST.

pub mod parser;

use std::ops::Deref;
use std::str::FromStr;

/// The top-level query AST node, encoding the active search mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Query {
    /// Default score mode — search scores with boolean logic.
    Score(ScoreQuery),
    /// Tag mode (`##`) — search for tags by name prefix.
    Tag { name: Option<TagItem> },
    /// People mode (`@@`) — search for people by name component prefixes.
    Person(Option<Person>),
}

impl Query {
    pub fn parse(input: &str, cursor_pos: usize) -> parser::Result<Self> {
        parser::Parser::new(input, cursor_pos).parse_top()
    }
}

/// A score-mode boolean query.
///
/// Combines search atoms with AND, OR, and NOT.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScoreQuery {
    Atom(SearchAtom),
    And(Vec<AndQuery>),
    Or(Vec<OrQuery>),
    Not(NotQuery),
}

impl ScoreQuery {
    // Flatten single length sequences to atoms.
    fn simplify_sequence(self) -> ScoreQuery {
        match self {
            ScoreQuery::And(items) => {
                if items.len() != 1 {
                    return ScoreQuery::And(items);
                }

                match items.into_iter().next().expect("checked above") {
                    AndQuery::Atom(atom) => ScoreQuery::Atom(atom),
                    AndQuery::Or(or) => ScoreQuery::Or(or),
                    AndQuery::Not(not) => ScoreQuery::Not(not),
                }
            }
            ScoreQuery::Or(items) => {
                if items.len() != 1 {
                    return ScoreQuery::Or(items);
                }

                match items.into_iter().next().expect("checked above") {
                    OrQuery::Atom(atom) => ScoreQuery::Atom(atom),
                    OrQuery::And(and) => ScoreQuery::And(and),
                    OrQuery::Not(not) => ScoreQuery::Not(not),
                }
            }
            _ => self,
        }
    }
}

/// A term that can appear inside an AND expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AndQuery {
    Atom(SearchAtom),
    Or(Vec<OrQuery>),
    Not(NotQuery),
}

/// A term that can appear inside an OR expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrQuery {
    Atom(SearchAtom),
    And(Vec<AndQuery>),
    Not(NotQuery),
}

/// A term that can be negated with `!`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotQuery {
    Atom(SearchAtom),
    And(Vec<AndQuery>),
    Or(Vec<OrQuery>),
}

/// A single search term in score mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchAtom {
    /// A title
    Title(String),
    /// A person name-component term (`@Name1.Name2`).
    Person(Person),
    /// A tag term (`#name` or `#name:value-expr`).
    Tag(Tag),
}

/// A people search mode query (`@@`).
///
/// Must contain at least one name component. Use `Query::Person(None)` to represent `@@` alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Person {
    names: Vec<PersonName>,
}

/// Error returned when constructing a [`Person`] fails.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PersonError {
    #[error("person query must contain at least one name component")]
    Empty,
}

impl Person {
    /// Constructs a person query from a non-empty list of name components.
    pub fn new(names: Vec<PersonName>) -> Result<Self, PersonError> {
        if names.is_empty() {
            return Err(PersonError::Empty);
        }
        Ok(Person { names })
    }

    /// Returns a slice of the name components in this person query.
    ///
    /// The slice is guaranteed to be non-empty
    pub fn names(&self) -> &[PersonName] {
        &self.names
    }
}

/// A validated name component, used in person queries.
///
/// - Cannot be empty
/// - Must only contain alphabetic characters
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersonName(String);

/// Error returned when parsing a [`PersonName`] fails.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PersonNameError {
    #[error("person name cannot be empty")]
    Empty,
    #[error("person name contains invalid character: '{0}'")]
    InvalidChar(char),
}

impl PersonName {
    /// Parses and validates a person name component.
    pub fn parse(s: &str) -> Result<Self, PersonNameError> {
        s.parse()
    }
}

impl FromStr for PersonName {
    type Err = PersonNameError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() {
            return Err(PersonNameError::Empty);
        }
        if let Some(c) = s.chars().find(|c| !c.is_alphabetic()) {
            return Err(PersonNameError::InvalidChar(c));
        }
        Ok(PersonName(s.to_owned()))
    }
}

impl Deref for PersonName {
    type Target = str;

    fn deref(&self) -> &str {
        &self.0
    }
}

/// A tag term within a score query (`#name` or `#name:value`).
///
/// `#tag:!value` in the query syntax is desugared by the parser to `!#tag:value`
/// (outer NOT), so a [`Tag`] always represents a positive value assertion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tag {
    /// The name of the tag.
    pub name: TagItem,
    /// The required value, if any.
    pub value: Option<TagItem>,
}

/// A validated identifier for tag names or values.
///
/// - Cannot be empty
/// - May only contain alphanumeric characters, `_`, or `-`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagItem(String);

/// Error returned when parsing a [`TagItem`] fails.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TagItemError {
    #[error("tag item cannot be empty")]
    Empty,
    #[error("tag item contains invalid character: '{0}'")]
    InvalidChar(char),
}

impl TagItem {
    /// Parses and validates a tag name or value.
    pub fn parse(s: &str) -> Result<Self, TagItemError> {
        s.parse()
    }
}

impl FromStr for TagItem {
    type Err = TagItemError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() {
            return Err(TagItemError::Empty);
        }
        if let Some(c) = s
            .chars()
            .find(|c| !c.is_alphanumeric() && *c != '_' && *c != '-')
        {
            return Err(TagItemError::InvalidChar(c));
        }
        Ok(TagItem(s.to_owned()))
    }
}

impl Deref for TagItem {
    type Target = str;

    fn deref(&self) -> &str {
        &self.0
    }
}
