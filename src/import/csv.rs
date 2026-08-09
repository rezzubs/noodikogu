//! A hand-rolled, RFC4180-ish CSV tokenizer: comma-separated fields, one row
//! per line, `"quoted,fields"` with `""` as an escaped embedded quote.
//! Deliberately minimal - not a general-purpose CSV library, just enough to
//! parse the bootstrap import format.

/// Error returned by [`parse_rows`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CsvError {
    #[error("line {line}: quoted field is never closed")]
    UnterminatedQuotedField { line: usize },
    #[error(
        "line {line}: unexpected character after a closing quote (expected ',' or end of line)"
    )]
    UnexpectedCharacterAfterQuote { line: usize },
}

#[derive(Clone, Copy)]
enum State {
    /// Start of a fresh field: a `"` here opens a quoted field, anything
    /// else is the first character of an unquoted one.
    FieldStart,
    Unquoted,
    /// Inside a quoted field, before the closing quote.
    Quoted,
    /// Just saw a `"` while inside a quoted field - ambiguous between an
    /// escaped `""` and the field actually closing.
    QuoteInQuoted,
}

/// Parses `input` into rows of fields.
///
/// Fields are comma-separated; a field may be wrapped in `"..."` to contain
/// commas or newlines verbatim, with `""` inside a quoted field representing
/// one literal `"`. `\r` is treated as insignificant whitespace around line
/// breaks (both bare `\n` and `\r\n` are accepted).
pub(super) fn parse_rows(input: &str) -> Result<Vec<Vec<String>>, CsvError> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut state = State::FieldStart;
    let mut line = 1usize;

    for c in input.chars() {
        match (state, c) {
            // `\r` is insignificant in every state, so both bare `\n` and
            // `\r\n` line endings are accepted without a lookahead.
            (_, '\r') => {}
            (State::FieldStart, '"') => state = State::Quoted,
            // Closing a field on `,`/`\n` is the same action regardless of
            // whether it was unquoted or a quoted field that just closed.
            (State::FieldStart | State::Unquoted | State::QuoteInQuoted, ',') => {
                row.push(std::mem::take(&mut field));
                state = State::FieldStart;
            }
            (State::FieldStart | State::Unquoted | State::QuoteInQuoted, '\n') => {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
                line += 1;
                state = State::FieldStart;
            }
            (State::FieldStart | State::Unquoted, other) => {
                field.push(other);
                state = State::Unquoted;
            }
            (State::Quoted, '"') => state = State::QuoteInQuoted,
            // Embedded newlines are literal data inside a quoted field, not
            // a row separator - unlike the merged arm above.
            (State::Quoted, '\n') => {
                field.push('\n');
                line += 1;
            }
            (State::Quoted, other) => field.push(other),
            (State::QuoteInQuoted, '"') => {
                field.push('"');
                state = State::Quoted;
            }
            (State::QuoteInQuoted, _) => {
                return Err(CsvError::UnexpectedCharacterAfterQuote { line });
            }
        }
    }

    if let State::Quoted = state {
        return Err(CsvError::UnterminatedQuotedField { line });
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }

    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_unquoted_row() {
        assert_eq!(parse_rows("a,b,c").unwrap(), vec![vec!["a", "b", "c"]]);
    }

    #[test]
    fn parses_multiple_rows() {
        assert_eq!(
            parse_rows("a,b\nc,d\n").unwrap(),
            vec![vec!["a", "b"], vec!["c", "d"]]
        );
    }

    #[test]
    fn quoted_field_may_contain_commas() {
        assert_eq!(parse_rows("\"a,b\",c").unwrap(), vec![vec!["a,b", "c"]]);
    }

    #[test]
    fn quoted_field_with_escaped_quote_produces_literal_quote() {
        assert_eq!(parse_rows("\"a\"\"b\"").unwrap(), vec![vec!["a\"b"]]);
    }

    #[test]
    fn quoted_field_may_contain_embedded_newline() {
        assert_eq!(parse_rows("\"a\nb\",c").unwrap(), vec![vec!["a\nb", "c"]]);
    }

    #[test]
    fn handles_crlf_line_endings() {
        assert_eq!(
            parse_rows("a,b\r\nc,d\r\n").unwrap(),
            vec![vec!["a", "b"], vec!["c", "d"]]
        );
    }

    #[test]
    fn trailing_newline_does_not_produce_an_extra_empty_row() {
        assert_eq!(parse_rows("a,b\n").unwrap(), vec![vec!["a", "b"]]);
    }

    #[test]
    fn trailing_comma_without_trailing_newline_produces_a_final_empty_field() {
        assert_eq!(parse_rows("a,b,").unwrap(), vec![vec!["a", "b", ""]]);
    }

    #[test]
    fn empty_input_produces_no_rows() {
        assert_eq!(parse_rows("").unwrap(), Vec::<Vec<String>>::new());
    }

    #[test]
    fn blank_line_produces_a_row_with_one_empty_field() {
        assert_eq!(parse_rows("\n").unwrap(), vec![vec![""]]);
    }

    #[test]
    fn unterminated_quoted_field_is_an_error() {
        assert_eq!(
            parse_rows("\"a,b").unwrap_err(),
            CsvError::UnterminatedQuotedField { line: 1 }
        );
    }

    #[test]
    fn unexpected_character_after_closing_quote_is_an_error() {
        assert_eq!(
            parse_rows("\"a\"b,c").unwrap_err(),
            CsvError::UnexpectedCharacterAfterQuote { line: 1 }
        );
    }
}
