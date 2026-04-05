use std::fmt::Display;

use crate::query::parser::DisplayToken;

pub trait IntoExpected {
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

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Expected {
    /// One of multiple expected values.
    OneOf { options: Vec<ExpectedValue> },
    /// Exactly one expected value.
    One(ExpectedValue),
}

impl Expected {
    /// Chain another expected value
    pub fn or(self, other: impl IntoExpectedValue) -> Self {
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
                let (sep, prefix) = if options.len() == 2 {
                    (" or ", "")
                } else {
                    (", ", "one of: ")
                };

                write!(
                    f,
                    "{}{}",
                    prefix,
                    options
                        .iter()
                        .map(|option| option.to_string())
                        .collect::<Vec<_>>()
                        .join(sep)
                )
            }
            Expected::One(option) => {
                write!(f, "{}", option)
            }
        }
    }
}

pub trait IntoExpectedValue {
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
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ExpectedValue {
    /// A specific token.
    Token(DisplayToken),
    /// End of input.
    Eof,
    /// A tag name.
    TagName,
    /// A tag value.
    TagValueExpression,
    /// A single name of a person
    NameSegment,
    /// Any whitespace
    WhiteSpace,
    /// A full tag expression `#tag:value` or `#tag`.
    TagExpression,
    /// A `()` delimited group
    Group,
    /// A score title
    Title,
    /// A full name expression `@first.middle.last` or `@first`
    NameExpression,
}

impl ExpectedValue {
    /// Chain another expected value
    pub fn or(self, other: impl IntoExpectedValue) -> Expected {
        Expected::OneOf {
            options: Vec::from([self, other.into_expected_value()]),
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
            ExpectedValue::NameSegment => write!(f, "a name segment"),
            ExpectedValue::WhiteSpace => write!(f, "whitespace"),
            ExpectedValue::TagExpression => write!(f, "a tag expression"),
            ExpectedValue::Group => write!(f, "a group (`()`)"),
            ExpectedValue::Title => write!(f, "a score title"),
            ExpectedValue::NameExpression => write!(f, "a name expression"),
            ExpectedValue::TagValueExpression => write!(f, "a tag value expression"),
        }
    }
}
