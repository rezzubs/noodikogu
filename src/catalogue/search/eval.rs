//! Compiles `crate::query::{ScoreQuery, AndQuery, OrQuery, NotQuery,
//! SearchAtom}` into a `sea_query::SelectStatement` yielding one
//! `score_id` column per matching score.
//!
//! # The `SQLite` set-operator associativity gotcha
//!
//! Every `compile_*` function below returns a statement yielding one
//! column, aliased [`SCORE_ID`]. Chaining `.union(UnionType::X, ...)`
//! calls to directly mirror the AST's nesting would be unsound: unlike
//! the SQL standard, `SQLite` gives `UNION`/`INTERSECT`/`EXCEPT` equal
//! precedence, grouped left-to-right - `A INTERSECT B EXCEPT C` always
//! means `(A INTERSECT B) EXCEPT C`, never `A INTERSECT (B EXCEPT C)`.
//! Since `AndQuery`/`OrQuery`/`NotQuery` freely nest each other, naively
//! chaining would silently compute the wrong boolean structure whenever
//! two different operators meet.
//!
//! The rule applied uniformly below: whenever a composite node's result
//! becomes one operand of a *different* set operator, [`wrap`] it first
//! into a derived table. A flat run of the *same* operator never needs
//! wrapping (associative). This is applied mechanically, not
//! case-by-case, so adding `Person`/`Tag` atoms later needs zero changes
//! to this composition logic - only a new [`compile_atom`] arm.

use crate::catalogue::normalize::{normalize_text, words};
use crate::catalogue::schema::{Scores, TitleWords, Titles};
use crate::query::{AndQuery, NotQuery, OrQuery, ScoreQuery, SearchAtom};
use sea_query::{Alias, Expr, ExprTrait, JoinType, LikeExpr, Query, SelectStatement, UnionType};

/// The name every compiled statement's sole output column is aliased to.
const SCORE_ID: &str = "score_id";

/// Compiles a top-level score query into a statement yielding one
/// distinct `score_id` per matching score.
///
/// Every atom kind compiles independently via [`compile_atom`] to this
/// same single-column shape; this function and its siblings only ever
/// combine already-compiled statements by id.
pub(super) fn compile_score_query(query: ScoreQuery) -> SelectStatement {
    match query {
        ScoreQuery::Atom(atom) => compile_atom(atom),
        ScoreQuery::And(items) => compile_and(items),
        ScoreQuery::Or(items) => compile_or(items),
        ScoreQuery::Not(not) => compile_not(not),
    }
}

/// Intersects (AND) at least two operands.
fn compile_and(items: Vec<AndQuery>) -> SelectStatement {
    combine(
        items.into_iter().map(compile_and_term),
        UnionType::Intersect,
    )
}

fn compile_and_term(item: AndQuery) -> SelectStatement {
    match item {
        // A bare atom is a plain SELECT (no top-level set operator of its
        // own), so it composes directly into the surrounding INTERSECT
        // chain without wrapping.
        AndQuery::Atom(atom) => compile_atom(atom),
        // `Or`/`Not` each produce their own UNION/EXCEPT chain internally
        // - a *different* operator than this INTERSECT - so they must be
        // wrapped before joining the chain.
        AndQuery::Or(items) => wrap(compile_or(items)),
        AndQuery::Not(not) => wrap(compile_not(not)),
    }
}

/// Unions (OR) at least two operands. `UnionType::Distinct` (plain
/// `UNION`, deduped) is used rather than `All`, since the same `score_id`
/// could otherwise appear once per matching OR branch.
fn compile_or(items: Vec<OrQuery>) -> SelectStatement {
    combine(items.into_iter().map(compile_or_term), UnionType::Distinct)
}

fn compile_or_term(item: OrQuery) -> SelectStatement {
    match item {
        OrQuery::Atom(atom) => compile_atom(atom),
        OrQuery::And(items) => wrap(compile_and(items)),
        OrQuery::Not(not) => wrap(compile_not(not)),
    }
}

/// `universe EXCEPT inner`.
fn compile_not(not: NotQuery) -> SelectStatement {
    let inner = match not {
        NotQuery::Atom(atom) => compile_atom(atom),
        NotQuery::And(items) => compile_and(items),
        NotQuery::Or(items) => compile_or(items),
    };
    let mut universe = all_score_ids();
    // `inner` is always wrapped here regardless of its shape: EXCEPT is
    // not associative, so its right operand must always be fully
    // parenthesized, never left to flatten into whatever operator `inner`
    // used internally.
    universe.union(UnionType::Except, wrap(inner));
    universe
}

/// `SELECT id AS score_id FROM scores` - the universe a top-level or
/// nested `NOT` subtracts from.
fn all_score_ids() -> SelectStatement {
    Query::select()
        .expr_as(Expr::col((Scores::Table, Scores::Id)), Alias::new(SCORE_ID))
        .from(Scores::Table)
        .take()
}

/// Wraps `inner` in a derived-table `SELECT score_id FROM (...)`, so it
/// composes safely as a single operand of a different set operator one
/// level up (see the module doc for why this is necessary in `SQLite`).
fn wrap(inner: SelectStatement) -> SelectStatement {
    let alias = Alias::new("wrapped_scores");
    Query::select()
        .expr_as(
            Expr::col((alias.clone(), Alias::new(SCORE_ID))),
            Alias::new(SCORE_ID),
        )
        .from_subquery(inner, alias)
        .take()
}

/// Chains already-compiled statements with the same `union_type`. Safe to
/// chain flat because `INTERSECT`/`UNION`/`EXCEPT` applied to a run of the
/// *same* operator is associative - only *mixing* operators needs the
/// [`wrap`] treatment, which callers apply before reaching this function.
///
/// # Panics
///
/// Panics if `statements` is empty. The `ScoreQuery`/`AndQuery`/`OrQuery`
/// AST guarantees every `And`/`Or` vec has at least 2 elements (the
/// parser's `simplify_sequence` collapses 0- and 1-element sequences
/// away), so this is unreachable in practice.
fn combine(
    mut statements: impl Iterator<Item = SelectStatement>,
    union_type: UnionType,
) -> SelectStatement {
    let mut acc = statements
        .next()
        .expect("AST guarantees non-empty And/Or sequences");
    for stmt in statements {
        acc.union(union_type, stmt);
    }
    acc
}

fn compile_atom(atom: SearchAtom) -> SelectStatement {
    match atom {
        SearchAtom::Title(text) => compile_title(&text),
        // No `people`/`score_people` schema yet. When it exists, this
        // becomes `compile_person(person)` returning the same
        // single-`score_id`-column shape as `compile_title` - nothing
        // above this match arm needs to change.
        SearchAtom::Person(_) => todo!("no people/score_people schema yet"),
        // Same story for tags.
        SearchAtom::Tag(_) => todo!("no tags/score_tags schema yet"),
    }
}

/// Matches a `Title` atom: every normalized word in `text` must appear as
/// a prefix of some word in the *same* title (not merely the same
/// score). Implemented as a self-join of `title_words` (one extra join
/// per word after the first) rather than an `INTERSECT` of per-word
/// `title_id` sets, so a multi-word title atom is a plain JOIN/WHERE with
/// no top-level set operator - no [`wrap`] needed for this case at all.
///
/// # Panics
///
/// Panics if `text` is empty or contains only whitespace.
fn compile_title(text: &str) -> SelectStatement {
    let normalized = normalize_text(text);
    let query_words: Vec<&str> = words(&normalized).collect();
    assert!(
        !query_words.is_empty(),
        "a Title atom always has >= 1 word by construction"
    );

    let base = Alias::new("title_words_0");
    let mut select = Query::select();
    select
        .expr_as(
            Expr::col((Titles::Table, Titles::ScoreId)),
            Alias::new(SCORE_ID),
        )
        .distinct() // a title with a repeated word could otherwise match twice
        .from_as(TitleWords::Table, base.clone())
        .inner_join(
            Titles::Table,
            Expr::col((Titles::Table, Titles::Id)).equals((base.clone(), TitleWords::TitleId)),
        )
        .and_where(word_prefix(base.clone(), query_words[0]));

    for (i, word) in query_words.iter().enumerate().skip(1) {
        let alias = Alias::new(format!("title_words_{i}"));
        select
            .join_as(
                JoinType::InnerJoin,
                TitleWords::Table,
                alias.clone(),
                Expr::col((alias.clone(), TitleWords::TitleId))
                    .equals((base.clone(), TitleWords::TitleId)),
            )
            .and_where(word_prefix(alias, word));
    }

    select.take()
}

/// `<alias>.word LIKE '<escaped word>%' ESCAPE '\'` - an index-backed
/// prefix match (no leading wildcard).
///
/// `%`, `_`, and a literal `\` in `word` are backslash-escaped first,
/// since `word` is arbitrary user-typed text and those three characters
/// would otherwise be interpreted as LIKE wildcards / the escape
/// introducer rather than literal characters to match.
fn word_prefix(alias: Alias, word: &str) -> Expr {
    let mut pattern = String::with_capacity(word.len() + 1);
    for c in word.chars() {
        if matches!(c, '%' | '_' | '\\') {
            pattern.push('\\');
        }
        pattern.push(c);
    }
    pattern.push('%');
    Expr::col((alias, TitleWords::Word)).like(LikeExpr::new(pattern).escape('\\'))
}

/// Snapshot-style tests asserting the exact SQL/params `compile_score_query`
/// produces, one per AST shape. These exist alongside the
/// behavioral/end-to-end tests in `search.rs` (which run the queries
/// against a real database and assert on results) because a compilation
/// bug can be invisible there: two different `ScoreQuery` shapes can
/// happen to select the same rows from a small fixture even though the
/// generated SQL is structurally wrong (e.g. the `SQLite`
/// associativity/`wrap()` bug the module doc describes) - checking the
/// literal SQL text is what actually pins down *how* the AST composes.
#[cfg(test)]
mod tests {
    use super::*;

    fn compile(query: ScoreQuery) -> (String, sea_query::Values) {
        compile_score_query(query).build(sea_query::SqliteQueryBuilder)
    }

    fn atom(text: &str) -> SearchAtom {
        SearchAtom::Title(text.to_string())
    }

    fn like_values(patterns: &[&str]) -> sea_query::Values {
        sea_query::Values(
            patterns
                .iter()
                .map(|p| sea_query::Value::String(Some((*p).to_string())))
                .collect(),
        )
    }

    #[test]
    fn single_word_atom_is_one_self_joinless_select() {
        let (sql, values) = compile(ScoreQuery::Atom(atom("Maria")));
        assert_eq!(
            sql,
            r#"SELECT DISTINCT "titles"."score_id" AS "score_id" FROM "title_words" AS "title_words_0" INNER JOIN "titles" ON "titles"."id" = "title_words_0"."title_id" WHERE "title_words_0"."word" LIKE ? ESCAPE '\'"#
        );
        assert_eq!(values, like_values(&["maria%"]));
    }

    #[test]
    fn multi_word_atom_self_joins_title_words_once_per_extra_word() {
        let (sql, values) = compile(ScoreQuery::Atom(atom("ave maria")));
        assert_eq!(
            sql,
            r#"SELECT DISTINCT "titles"."score_id" AS "score_id" FROM "title_words" AS "title_words_0" INNER JOIN "titles" ON "titles"."id" = "title_words_0"."title_id" INNER JOIN "title_words" AS "title_words_1" ON "title_words_1"."title_id" = "title_words_0"."title_id" WHERE "title_words_0"."word" LIKE ? ESCAPE '\' AND "title_words_1"."word" LIKE ? ESCAPE '\'"#
        );
        assert_eq!(values, like_values(&["ave%", "maria%"]));
    }

    #[test]
    fn like_metacharacters_are_escaped_in_bound_values() {
        let (_sql, values) = compile(ScoreQuery::Atom(atom("100%")));
        assert_eq!(values, like_values(&["100\\%%"]));
    }

    #[test]
    fn and_of_atoms_is_a_flat_intersect_with_no_wrapping() {
        let (sql, _values) = compile(ScoreQuery::And(vec![
            AndQuery::Atom(atom("ave")),
            AndQuery::Atom(atom("maria")),
        ]));
        assert_eq!(
            sql,
            r#"SELECT DISTINCT "titles"."score_id" AS "score_id" FROM "title_words" AS "title_words_0" INNER JOIN "titles" ON "titles"."id" = "title_words_0"."title_id" WHERE "title_words_0"."word" LIKE ? ESCAPE '\' INTERSECT SELECT DISTINCT "titles"."score_id" AS "score_id" FROM "title_words" AS "title_words_0" INNER JOIN "titles" ON "titles"."id" = "title_words_0"."title_id" WHERE "title_words_0"."word" LIKE ? ESCAPE '\'"#
        );
    }

    #[test]
    fn or_of_atoms_is_a_flat_union_with_no_wrapping() {
        let (sql, _values) = compile(ScoreQuery::Or(vec![
            OrQuery::Atom(atom("maria")),
            OrQuery::Atom(atom("praise")),
        ]));
        assert_eq!(
            sql,
            r#"SELECT DISTINCT "titles"."score_id" AS "score_id" FROM "title_words" AS "title_words_0" INNER JOIN "titles" ON "titles"."id" = "title_words_0"."title_id" WHERE "title_words_0"."word" LIKE ? ESCAPE '\' UNION SELECT DISTINCT "titles"."score_id" AS "score_id" FROM "title_words" AS "title_words_0" INNER JOIN "titles" ON "titles"."id" = "title_words_0"."title_id" WHERE "title_words_0"."word" LIKE ? ESCAPE '\'"#
        );
    }

    #[test]
    fn not_subtracts_a_wrapped_inner_from_all_score_ids() {
        let (sql, _values) = compile(ScoreQuery::Not(NotQuery::Atom(atom("verum"))));
        assert_eq!(
            sql,
            r#"SELECT "scores"."id" AS "score_id" FROM "scores" EXCEPT SELECT "wrapped_scores"."score_id" AS "score_id" FROM (SELECT DISTINCT "titles"."score_id" AS "score_id" FROM "title_words" AS "title_words_0" INNER JOIN "titles" ON "titles"."id" = "title_words_0"."title_id" WHERE "title_words_0"."word" LIKE ? ESCAPE '\') AS "wrapped_scores""#
        );
    }

    /// Pins down the exact wrapped shape of `(A | B) & !C`: this is the
    /// case that motivated `wrap()` in the first place, since naively
    /// chaining `.union()` calls to mirror the AST would rely on `SQLite`
    /// treating `INTERSECT`/`EXCEPT`/`UNION` with different precedence,
    /// which it doesn't - see the module doc.
    #[test]
    fn mixed_operators_wrap_each_differently_operated_child() {
        let (sql, _values) = compile(ScoreQuery::And(vec![
            AndQuery::Or(vec![OrQuery::Atom(atom("ave")), OrQuery::Atom(atom("nope"))]),
            AndQuery::Not(NotQuery::Atom(atom("verum"))),
        ]));
        assert_eq!(
            sql,
            r#"SELECT "wrapped_scores"."score_id" AS "score_id" FROM (SELECT DISTINCT "titles"."score_id" AS "score_id" FROM "title_words" AS "title_words_0" INNER JOIN "titles" ON "titles"."id" = "title_words_0"."title_id" WHERE "title_words_0"."word" LIKE ? ESCAPE '\' UNION SELECT DISTINCT "titles"."score_id" AS "score_id" FROM "title_words" AS "title_words_0" INNER JOIN "titles" ON "titles"."id" = "title_words_0"."title_id" WHERE "title_words_0"."word" LIKE ? ESCAPE '\') AS "wrapped_scores" INTERSECT SELECT "wrapped_scores"."score_id" AS "score_id" FROM (SELECT "scores"."id" AS "score_id" FROM "scores" EXCEPT SELECT "wrapped_scores"."score_id" AS "score_id" FROM (SELECT DISTINCT "titles"."score_id" AS "score_id" FROM "title_words" AS "title_words_0" INNER JOIN "titles" ON "titles"."id" = "title_words_0"."title_id" WHERE "title_words_0"."word" LIKE ? ESCAPE '\') AS "wrapped_scores") AS "wrapped_scores""#
        );
    }
}
