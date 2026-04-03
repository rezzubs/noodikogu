use std::{iter::Peekable, ops::Range, str::Chars};

/// A single lexical token produced by the [`Lexer`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Range<usize>,
}

impl AsRef<Token> for Token {
    fn as_ref(&self) -> &Token {
        self
    }
}

/// The kind of a [`Token`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TokenKind {
    /// `#`
    TagStart,
    /// `##`
    TagMode,
    /// `:`
    TagValueStart,
    /// `@`
    NameStart,
    /// `@@`
    NameMode,
    /// `(`
    GroupStart,
    /// `)`
    GroupEnd,
    /// `|`
    Or,
    /// `!`
    Not,
    /// `.`
    Dot,
    /// Any sequence of whitespace characters.
    Whitespace,
    /// Text for a [`super::TagItem`], [`super::PersonName`] or title.
    Word,
    /// `"` delimited text. Can contain any unicode characters.
    ///
    /// The span covers only the content between the quotes, not the quotes themselves.
    /// An unterminated string is treated as a complete `RawText`. Empty strings are ignored.
    RawText,
}

/// An iterator over the [`Token`]s of a query string.
pub struct Lexer<'a> {
    input: &'a str,
    iter: Peekable<Chars<'a>>,
    position: usize,
}

impl<'a> Lexer<'a> {
    /// Creates a new `Lexer` over the given input string.
    pub fn new(input: &'a str) -> Self {
        Lexer {
            input,
            iter: input.chars().peekable(),
            position: 0,
        }
    }

    /// Returns the original input string.
    pub fn input(&self) -> &'a str {
        self.input
    }

    fn advance(&mut self) -> Option<char> {
        let c = self.iter.next()?;
        self.position += c.len_utf8();
        Some(c)
    }

    fn peek(&mut self) -> Option<char> {
        self.iter.peek().copied()
    }
}

impl<'a> Iterator for Lexer<'a> {
    type Item = Token;

    fn next(&mut self) -> Option<Token> {
        let start = self.position;
        let c = self.advance()?;

        let kind = match c {
            '#' => {
                if self.peek() == Some('#') {
                    self.advance();
                    TokenKind::TagMode
                } else {
                    TokenKind::TagStart
                }
            }
            '@' => {
                if self.peek() == Some('@') {
                    self.advance();
                    TokenKind::NameMode
                } else {
                    TokenKind::NameStart
                }
            }
            ':' => TokenKind::TagValueStart,
            '(' => TokenKind::GroupStart,
            ')' => TokenKind::GroupEnd,
            '|' => TokenKind::Or,
            '!' => TokenKind::Not,
            '.' => TokenKind::Dot,
            '"' => {
                let content_start = self.position;
                loop {
                    let before = self.position;
                    match self.advance() {
                        Some('"') => {
                            if before == content_start {
                                // Empty `""` — skip and continue.
                                return self.next();
                            }
                            return Some(Token {
                                kind: TokenKind::RawText,
                                span: content_start..before,
                            });
                        }
                        Some(_) => {}
                        None => {
                            if content_start == self.position {
                                // Lone `"` at end of input — nothing to emit.
                                return None;
                            }
                            // Unterminated string — treat content as RawText.
                            return Some(Token {
                                kind: TokenKind::RawText,
                                span: content_start..self.position,
                            });
                        }
                    }
                }
            }
            c if c.is_whitespace() => {
                while self.peek().is_some_and(|c| c.is_whitespace()) {
                    self.advance();
                }
                TokenKind::Whitespace
            }
            _ => {
                while self.peek().is_some_and(|c| !is_special(c)) {
                    self.advance();
                }
                TokenKind::Word
            }
        };

        Some(Token {
            kind,
            span: start..self.position,
        })
    }
}

/// Returns true if `c` terminates a [`TokenKind::Word`].
fn is_special(c: char) -> bool {
    matches!(c, '#' | '@' | ':' | '(' | ')' | '|' | '!' | '.' | '"') || c.is_whitespace()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lex(input: &str) -> Vec<Token> {
        Lexer::new(input).collect()
    }

    fn kinds(input: &str) -> Vec<TokenKind> {
        Lexer::new(input).map(|t| t.kind).collect()
    }

    #[test]
    fn empty_input() {
        assert_eq!(kinds(""), []);
    }

    #[test]
    fn title_words_and_spans() {
        let input = "Pseudo Yoik";
        let tokens = lex(input);
        assert_eq!(tokens[0].kind, TokenKind::Word);
        assert_eq!(&input[tokens[0].span.clone()], "Pseudo");
        assert_eq!(tokens[1].kind, TokenKind::Whitespace);
        assert_eq!(tokens[2].kind, TokenKind::Word);
        assert_eq!(&input[tokens[2].span.clone()], "Yoik");
    }

    #[test]
    fn whitespace_is_collapsed() {
        let tokens = lex("a  \r\n\tb");
        assert_eq!(tokens[1].kind, TokenKind::Whitespace);
        assert_eq!(tokens[1].span, 1..6);
    }

    #[test]
    fn tag_no_value() {
        assert_eq!(kinds("#folk"), [TokenKind::TagStart, TokenKind::Word]);
    }

    #[test]
    fn tag_with_value() {
        assert_eq!(
            kinds("#key:C"),
            [
                TokenKind::TagStart,
                TokenKind::Word,
                TokenKind::TagValueStart,
                TokenKind::Word
            ]
        );
    }

    #[test]
    fn tag_value_not() {
        assert_eq!(
            kinds("#key:!C"),
            [
                TokenKind::TagStart,
                TokenKind::Word,
                TokenKind::TagValueStart,
                TokenKind::Not,
                TokenKind::Word
            ]
        );
    }

    #[test]
    fn tag_value_and_group() {
        assert_eq!(
            kinds("#key:(C D)"),
            [
                TokenKind::TagStart,
                TokenKind::Word,
                TokenKind::TagValueStart,
                TokenKind::GroupStart,
                TokenKind::Word,
                TokenKind::Whitespace,
                TokenKind::Whitespace,
                TokenKind::Word,
                TokenKind::GroupEnd,
            ]
        );
    }

    #[test]
    fn tag_mode() {
        assert_eq!(kinds("##difficulty"), [TokenKind::TagMode, TokenKind::Word]);
    }

    #[test]
    fn tag_mode_alone() {
        assert_eq!(kinds("##"), [TokenKind::TagMode]);
    }

    #[test]
    fn person() {
        assert_eq!(kinds("@Soosalu"), [TokenKind::NameStart, TokenKind::Word]);
    }

    #[test]
    fn person_multiple_components() {
        assert_eq!(
            kinds("@Vettik.Tuudur"),
            [
                TokenKind::NameStart,
                TokenKind::Word,
                TokenKind::Dot,
                TokenKind::Word
            ]
        );
    }

    #[test]
    fn people_mode() {
        assert_eq!(kinds("@@Vettik"), [TokenKind::NameMode, TokenKind::Word]);
    }

    #[test]
    fn people_mode_alone() {
        assert_eq!(kinds("@@"), [TokenKind::NameMode]);
    }

    #[test]
    fn boolean_or() {
        assert_eq!(
            kinds("Yoik | #folk"),
            [
                TokenKind::Word,
                TokenKind::Whitespace,
                TokenKind::Or,
                TokenKind::Whitespace,
                TokenKind::TagStart,
                TokenKind::Word
            ]
        );
    }

    #[test]
    fn boolean_not() {
        assert_eq!(
            kinds("!#classical"),
            [TokenKind::Not, TokenKind::TagStart, TokenKind::Word]
        );
    }

    #[test]
    fn grouping() {
        assert_eq!(
            kinds("(#folk | #classical)"),
            [
                TokenKind::GroupStart,
                TokenKind::TagStart,
                TokenKind::Word,
                TokenKind::Whitespace,
                TokenKind::Or,
                TokenKind::Whitespace,
                TokenKind::TagStart,
                TokenKind::Word,
                TokenKind::GroupEnd,
            ]
        );
    }

    #[test]
    fn raw_text_span_excludes_quotes() {
        let input = r##""#literal""##;
        let tokens = lex(input);
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].kind, TokenKind::RawText);
        assert_eq!(&input[tokens[0].span.clone()], "#literal");
    }

    #[test]
    fn raw_text_unicode_byte_spans() {
        // 'Ä' and 'ä' are 2 bytes each in UTF-8.
        let input = "\"Ä käru\"";
        let tokens = lex(input);
        assert_eq!(tokens[0].kind, TokenKind::RawText);
        assert_eq!(&input[tokens[0].span.clone()], "Ä käru");
    }

    #[test]
    fn empty_raw_text_skipped() {
        // `""` is ignored; tokens after it are still yielded.
        assert_eq!(kinds("\"\" foo"), [TokenKind::Whitespace, TokenKind::Word]);
    }

    #[test]
    fn unterminated_string_as_raw_text() {
        let input = "\"hello";
        let tokens = lex(input);
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].kind, TokenKind::RawText);
        assert_eq!(&input[tokens[0].span.clone()], "hello");
    }

    #[test]
    fn complex_query() {
        assert_eq!(
            kinds("Pseudo Yoik @Soosalu #difficulty:medium | #genre:dubstep"),
            [
                TokenKind::Word, // Pseudo
                TokenKind::Whitespace,
                TokenKind::Word, // Yoik
                TokenKind::Whitespace,
                TokenKind::NameStart,
                TokenKind::Word, // Soosalu
                TokenKind::Whitespace,
                TokenKind::TagStart,
                TokenKind::Word, // difficulty
                TokenKind::TagValueStart,
                TokenKind::Word, // medium
                TokenKind::Whitespace,
                TokenKind::Or,
                TokenKind::Whitespace,
                TokenKind::TagStart,
                TokenKind::Word, // genre
                TokenKind::TagValueStart,
                TokenKind::Word, // dubstep
            ]
        );
    }
}
