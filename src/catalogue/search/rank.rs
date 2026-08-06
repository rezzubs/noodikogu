//! Builds the relevance-ranking `ORDER BY` expression `Catalogue::search`
//! applies on top of the boolean id set `eval.rs` compiles.
//!
//! See `docs/decisions/0005-hand-rolled-search.md` for why this is a cheap
//! heuristic (distinct query words matched, boosted for a whole-title or
//! prefix-of-whole-title match) rather than a BM25-style model.

use super::eval::prefix_match;
use crate::catalogue::normalize::{normalize_text, words};
use crate::catalogue::schema::{TitleWords, Titles};
use crate::query::{AndQuery, OrQuery, ScoreQuery, SearchAtom};
use sea_query::{CaseStatement, Expr, ExprTrait, Func, Query, SelectStatement};
use tracing::warn;

/// Added on top of the word-match-quality sum when a score's primary
/// title equals the combined query text exactly. Large enough to
/// dominate the word-match-quality sum (bounded by the number of
/// distinct query words) at any realistic query length.
const EXACT_TITLE_BOOST: i64 = 1_000_000;
/// Added when the title isn't an exact match but starts with the
/// combined query text. Smaller than [`EXACT_TITLE_BOOST`] but still
/// large enough to dominate the word-match-quality sum.
const PREFIX_TITLE_BOOST: i64 = 1_000;

/// Ranks scores by how well their primary title matches the free-text
/// `Title` portion of a query - see [`TitleRanking::from_query`].
/// `Person`/`Tag` atoms are structured lookups, not free text, so they
/// never contribute a ranking signal.
pub(super) struct TitleRanking {
    /// Distinct normalized words from every positively-asserted `Title`
    /// atom in the query, order-preserving.
    words: Vec<String>,
    /// Every positively-asserted `Title` atom's text, normalized and
    /// joined with spaces - compared against a title's whole
    /// `value_normalized` for the exact/prefix boost.
    combined_normalized: String,
}

impl TitleRanking {
    /// Builds a ranking signal from every `Title` atom in `query` that
    /// isn't inside a `NotQuery`.
    ///
    /// An excluded term carries no positive ranking signal, so its text is
    /// skipped entirely rather than contributing words to rank by. AND/OR
    /// structure is otherwise ignored: every reachable `Title` atom's text is
    /// joined into one combined string, exactly as if it had all been typed as
    /// one title query - a continuous "ave maria" and a grouped "(ave) (maria)"
    /// rank identically.
    ///
    /// Returns `None` if the query has no such atom (a pure
    /// `Person`/`Tag` query, or one that's entirely negated) - the
    /// caller should skip ranking entirely in that case and keep the
    /// existing alphabetical-only order.
    pub(super) fn from_query(query: &ScoreQuery) -> Option<Self> {
        let mut texts = Vec::new();
        collect_score(query, &mut texts);
        if texts.is_empty() {
            return None;
        }

        let combined_normalized = normalize_text(&texts.join(" "));
        let mut distinct_words = Vec::new();
        // Note that `texts` may include whole sentences and thus needs to be
        // split again by `words`.
        for word in words(&combined_normalized) {
            // The O(n^2) loop is likely faster than a hash set for realistic
            // queries with not that many words and also keeps the iteration
            // order stable.
            if !distinct_words.iter().any(|seen: &String| seen == word) {
                distinct_words.push(word.to_string());
            }
        }

        Some(Self {
            words: distinct_words,
            combined_normalized,
        })
    }

    /// The `ORDER BY` expression: a sum of per-word match-quality scores
    /// (see [`word_match_quality`]) plus the whole-title boost.
    pub(super) fn expr(&self) -> Expr {
        let word_quality_sum = self.words.iter().fold(Expr::val(0.0_f64), |acc, word| {
            acc.add(word_match_quality(word))
        });

        let boost = CaseStatement::new()
            .case(
                Expr::col((Titles::Table, Titles::ValueNormalized))
                    .eq(self.combined_normalized.clone()),
                EXACT_TITLE_BOOST,
            )
            .case(
                prefix_match(
                    (Titles::Table, Titles::ValueNormalized),
                    &self.combined_normalized,
                ),
                PREFIX_TITLE_BOOST,
            )
            .finally(0_i64);

        word_quality_sum.add(boost)
    }
}

/// `len(word) / len(matched title word)` for the best-matching
/// `title_words` row (`MAX`, since a title can repeat a word - or the
/// same word could match several title words of different lengths),
/// `0.0` if none match. Correlated to the outer query's `titles.id`, so
/// this is only valid as an expression within a statement that already
/// has `titles` in scope (as `hydrate()`'s does).
///
/// This is what makes a full word match ("maria" matching "maria") rank
/// above a short prefix match on a much longer word ("mar" matching
/// "marseille") even though both satisfy the same `LIKE 'mar%'` filter
/// `eval.rs::compile_title` uses to decide whether a score matches at
/// all - ranking asks a finer-grained question than filtering does.
fn word_match_quality(word: &str) -> Expr {
    let word_len: u32 = word
        .chars()
        .count()
        .try_into()
        .unwrap_or_else(|error| {
            warn!(
                %error,
                "Tried to compute a word match quality for a length that doesn't fit inisde u32. Falling back to u32::MAX",
            );
            u32::MAX
        });
    let word_len = f64::from(word_len);

    let ratio = Expr::val(word_len).div(Func::char_length(Expr::col((
        TitleWords::Table,
        TitleWords::Word,
    ))));

    let best_match: SelectStatement = Query::select()
        .expr(Func::max(ratio))
        .from(TitleWords::Table)
        .and_where(
            Expr::col((TitleWords::Table, TitleWords::TitleId)).equals((Titles::Table, Titles::Id)),
        )
        .and_where(prefix_match((TitleWords::Table, TitleWords::Word), word))
        .take();

    Func::coalesce([best_match.into(), Expr::val(0.0_f64)]).into()
}

fn collect_score(query: &ScoreQuery, out: &mut Vec<String>) {
    match query {
        ScoreQuery::Atom(SearchAtom::Title(text)) => out.push(text.clone()),
        ScoreQuery::And(items) => collect_and(items, out),
        ScoreQuery::Or(items) => collect_or(items, out),
        // An excluded term carries no positive ranking signal.
        ScoreQuery::Atom(_) | ScoreQuery::Not(_) => {}
    }
}

fn collect_and(items: &[AndQuery], out: &mut Vec<String>) {
    for item in items {
        match item {
            AndQuery::Atom(SearchAtom::Title(text)) => out.push(text.clone()),
            AndQuery::Or(items) => collect_or(items, out),
            AndQuery::Atom(_) | AndQuery::Not(_) => {}
        }
    }
}

fn collect_or(items: &[OrQuery], out: &mut Vec<String>) {
    for item in items {
        match item {
            OrQuery::Atom(SearchAtom::Title(text)) => out.push(text.clone()),
            OrQuery::And(items) => collect_and(items, out),
            OrQuery::Atom(_) | OrQuery::Not(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::{NotQuery, Person, PersonName, Tag, TagItem};

    fn title(text: &str) -> SearchAtom {
        SearchAtom::Title(text.to_string())
    }

    fn person_atom(name: &str) -> SearchAtom {
        SearchAtom::Person(Person::new(vec![PersonName::parse(name).unwrap()]).unwrap())
    }

    fn tag_atom(name: &str) -> SearchAtom {
        SearchAtom::Tag(Tag {
            name: TagItem::parse(name).unwrap(),
            value: None,
        })
    }

    fn build(query: &ScoreQuery) -> Option<(String, sea_query::Values)> {
        TitleRanking::from_query(query).map(|r| {
            Query::select()
                .expr(r.expr())
                .build(sea_query::SqliteQueryBuilder)
        })
    }

    #[test]
    fn from_query_returns_none_for_a_person_only_query() {
        let query = ScoreQuery::Atom(person_atom("Anna"));
        assert!(TitleRanking::from_query(&query).is_none());
    }

    #[test]
    fn from_query_returns_none_for_a_tag_only_query() {
        let query = ScoreQuery::Atom(tag_atom("laulupidu"));
        assert!(TitleRanking::from_query(&query).is_none());
    }

    #[test]
    fn from_query_collects_words_from_a_single_title_atom() {
        let query = ScoreQuery::Atom(title("Ave Maria"));
        let ranking = TitleRanking::from_query(&query).unwrap();
        assert_eq!(ranking.words, vec!["ave", "maria"]);
        assert_eq!(ranking.combined_normalized, "ave maria");
    }

    #[test]
    fn from_query_joins_words_from_multiple_anded_title_atoms() {
        let query = ScoreQuery::And(vec![
            AndQuery::Atom(title("Ave")),
            AndQuery::Atom(title("Maria")),
        ]);
        let ranking = TitleRanking::from_query(&query).unwrap();
        assert_eq!(ranking.words, vec!["ave", "maria"]);
        assert_eq!(ranking.combined_normalized, "ave maria");
    }

    /// Exercises the mutual recursion between `collect_and`/`collect_or`:
    /// an `Or` group nested inside an `And` (`(ave | verum) maria`, the
    /// same shape as `query::parser::tests::group_forces_or_inside_and`)
    /// must still have all three atoms' words reach the result, not just
    /// the ones at the top level.
    #[test]
    fn from_query_collects_title_text_from_a_nested_and_or_structure() {
        let query = ScoreQuery::And(vec![
            AndQuery::Or(vec![
                OrQuery::Atom(title("Ave")),
                OrQuery::Atom(title("Verum")),
            ]),
            AndQuery::Atom(title("Maria")),
        ]);
        let ranking = TitleRanking::from_query(&query).unwrap();
        assert_eq!(ranking.words, vec!["ave", "verum", "maria"]);
        assert_eq!(ranking.combined_normalized, "ave verum maria");
    }

    #[test]
    fn from_query_excludes_title_text_inside_not() {
        let query = ScoreQuery::And(vec![
            AndQuery::Atom(title("Ave")),
            AndQuery::Not(NotQuery::Atom(title("Verum"))),
        ]);
        let ranking = TitleRanking::from_query(&query).unwrap();
        assert_eq!(ranking.words, vec!["ave"]);
        assert_eq!(ranking.combined_normalized, "ave");
    }

    #[test]
    fn from_query_returns_none_when_everything_is_negated() {
        let query = ScoreQuery::Not(NotQuery::Atom(title("Ave Maria")));
        assert!(TitleRanking::from_query(&query).is_none());
    }

    #[test]
    fn expr_snapshot_for_a_single_word_query() {
        let query = ScoreQuery::Atom(title("Maria"));
        let (sql, _values) = build(&query).unwrap();
        assert_eq!(
            sql,
            r#"SELECT ? + COALESCE((SELECT MAX(? / LENGTH("title_words"."word")) FROM "title_words" WHERE "title_words"."title_id" = "titles"."id" AND "title_words"."word" LIKE ? ESCAPE '\'), ?) + (CASE WHEN ("titles"."value_normalized" = ?) THEN ? WHEN ("titles"."value_normalized" LIKE ? ESCAPE '\') THEN ? ELSE ? END)"#
        );
    }
}
