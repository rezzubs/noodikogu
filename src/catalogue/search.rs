//! Evaluates `crate::query::ScoreQuery` against the catalogue.

mod eval;
mod rank;

use super::schema::Titles;
use super::{Catalogue, DatabaseError, ScoreId, params};
use crate::query::ScoreQuery;
use rank::TitleRanking;
use sea_query::{Alias, Expr, ExprTrait, Order, Query, SqliteQueryBuilder};

/// A page window into a result set. Both fields are `u64` (matching what
/// `sea-query`'s `LIMIT`/`OFFSET` need) so the conversion from whatever
/// numeric type a caller has on hand (e.g. a `usize` parsed from a web
/// query string) happens once, at construction, rather than being
/// re-derived - and potentially mis-derived - deep inside the query
/// builder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pagination {
    pub limit: u64,
    pub offset: u64,
}

/// One row of a score search result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScoreSummary {
    pub id: ScoreId,
    pub primary_title: String,
}

/// Errors from [`Catalogue::search`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SearchError {
    #[error(transparent)]
    Db(#[from] DatabaseError),
}

impl From<turso::Error> for SearchError {
    fn from(error: turso::Error) -> Self {
        Self::Db(DatabaseError::from(error))
    }
}

impl Catalogue {
    /// Evaluates `query` and returns one page of matching scores.
    ///
    /// The scores are ranked by how well each score's primary title matches the
    /// query's free-text `Title` portion, falling back to alphabetical order
    /// (case/diacritic-insensitively, by primary title) as the tie-breaker.
    ///
    /// # Errors
    ///
    /// Returns [`SearchError::Db`] on an underlying database failure.
    pub async fn search(
        &self,
        query: ScoreQuery,
        pagination: Pagination,
    ) -> Result<Vec<ScoreSummary>, SearchError> {
        let conn = self.connect().await?;

        let ranking = TitleRanking::from_query(&query);
        let ids = eval::compile_score_query(query);
        let hydrated = hydrate(ids, ranking, pagination);
        let (sql, values) = hydrated.build(SqliteQueryBuilder);
        let params = params::to_turso_values(values);

        let mut rows = conn.query(&sql, params).await?;
        let mut summaries = Vec::new();
        while let Some(row) = rows.next().await? {
            summaries.push(ScoreSummary {
                id: ScoreId(row.get::<i64>(0)?),
                primary_title: row.get::<String>(1)?,
            });
        }
        Ok(summaries)
    }
}

/// Joins a compiled score-id set back to each score's primary title,
/// sorts alphabetically or by [`TitleRanking`], and applies `pagination`.
///
/// Relies on every score having exactly one primary title at all times
/// (guaranteed by `create_score` and `set_primary_title`; `remove_title`
/// refuses to remove a score's current primary title rather than leaving
/// this unsatisfied - see `title.rs`). If that invariant were ever
/// violated, the affected score would silently drop out of results.
fn hydrate(
    ids: sea_query::SelectStatement,
    ranking: Option<TitleRanking>,
    pagination: Pagination,
) -> sea_query::SelectStatement {
    let matches = Alias::new("matches");
    let score_id = Alias::new("score_id");

    let mut select = Query::select();
    select
        .expr_as(
            Expr::col((matches.clone(), score_id.clone())),
            Alias::new("id"),
        )
        .expr_as(
            Expr::col((Titles::Table, Titles::Value)),
            Alias::new("primary_title"),
        )
        .from_subquery(ids, matches.clone())
        .inner_join(
            Titles::Table,
            Expr::col((Titles::Table, Titles::ScoreId))
                .equals((matches, score_id))
                .and(Expr::col((Titles::Table, Titles::IsPrimary)).eq(1)),
        );

    if let Some(ranking) = ranking {
        select.order_by_expr(ranking.expr(), Order::Desc);
    }

    select
        .order_by((Titles::Table, Titles::ValueNormalized), Order::Asc)
        .limit(pagination.limit)
        .offset(pagination.offset)
        .take()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalogue::migration;
    use crate::catalogue::normalize::{case_fold, normalize_text, words};
    use crate::query::{AndQuery, NotQuery, OrQuery, ScoreQuery, SearchAtom};

    /// A fully migrated in-memory catalogue with a handful of scores inserted
    /// directly via SQL (the write API this search evaluator is tested
    /// independently - see `score.rs`/`title.rs` for those). Each score gets
    /// one primary title plus matching `title_words` rows, mirroring exactly
    /// what `create_score` does. Constructs `Catalogue` and calls `connect()`
    /// directly - both are private to `catalogue`, but visible here since this
    /// test module is a descendant of it.
    async fn seeded_catalogue(titles: &[&str]) -> Catalogue {
        let database = turso::Builder::new_local(":memory:").build().await.unwrap();
        let mut conn = database.connect().unwrap();
        conn.execute("PRAGMA foreign_keys = ON", ()).await.unwrap();
        migration::run(&mut conn, migration::ALL).await.unwrap();

        let catalogue = Catalogue { database };
        for title in titles {
            insert_score(&catalogue, title).await;
        }
        catalogue
    }

    async fn insert_score(catalogue: &Catalogue, title: &str) -> i64 {
        let conn = catalogue.connect().await.unwrap();
        conn.execute(
            "INSERT INTO scores (created_at) VALUES ('2026-01-01T00:00:00Z')",
            (),
        )
        .await
        .unwrap();
        let score_id = conn.last_insert_rowid();

        let normalized = normalize_text(title);
        conn.execute(
            "INSERT INTO titles (score_id, value, value_normalized, is_primary)
             VALUES (?, ?, ?, 1)",
            (score_id, title, normalized.clone()),
        )
        .await
        .unwrap();
        let title_id = conn.last_insert_rowid();

        for word in words(&normalized) {
            conn.execute(
                "INSERT OR IGNORE INTO title_words (title_id, word) VALUES (?, ?)",
                (title_id, word),
            )
            .await
            .unwrap();
        }

        score_id
    }

    /// Inserts a `people` row (plus its `person_words`) directly via SQL,
    /// mirroring exactly what `person.rs::create_person` does - the
    /// mutation API is tested independently, see `person.rs`.
    async fn insert_person(catalogue: &Catalogue, names: &[&str]) -> i64 {
        let conn = catalogue.connect().await.unwrap();
        let display_name = names.join(" ");
        let display_name_key = case_fold(&display_name);
        conn.execute(
            "INSERT INTO people (display_name, display_name_key) VALUES (?, ?)",
            (display_name.clone(), display_name_key),
        )
        .await
        .unwrap();
        let person_id = conn.last_insert_rowid();

        for word in words(&normalize_text(&display_name)) {
            conn.execute(
                "INSERT OR IGNORE INTO person_words (person_id, word) VALUES (?, ?)",
                (person_id, word),
            )
            .await
            .unwrap();
        }

        person_id
    }

    /// Inserts a `tags` row directly via SQL, mirroring `tag.rs::create_tag`.
    /// A tag's value is a per-attachment concept, not stored here - see
    /// `attach_tag_to_score`.
    async fn insert_tag(catalogue: &Catalogue, name: &str) -> i64 {
        let conn = catalogue.connect().await.unwrap();
        let name_normalized = normalize_text(name);
        conn.execute(
            "INSERT INTO tags (name, name_normalized) VALUES (?, ?)",
            (name, name_normalized),
        )
        .await
        .unwrap();
        conn.last_insert_rowid()
    }

    async fn attach_person_to_score(catalogue: &Catalogue, score_id: i64, person_id: i64) {
        let conn = catalogue.connect().await.unwrap();
        conn.execute(
            "INSERT INTO score_people (score_id, person_id) VALUES (?, ?)",
            (score_id, person_id),
        )
        .await
        .unwrap();
    }

    /// Inserts a `score_tags` row directly via SQL, mirroring
    /// `tag.rs::attach_tag` - `value` is per-attachment, per this table.
    async fn attach_tag_to_score(
        catalogue: &Catalogue,
        score_id: i64,
        tag_id: i64,
        value: Option<&str>,
    ) {
        let conn = catalogue.connect().await.unwrap();
        let value_normalized = value.map(normalize_text);
        conn.execute(
            "INSERT INTO score_tags (score_id, tag_id, value, value_normalized) VALUES (?, ?, ?, ?)",
            (score_id, tag_id, value, value_normalized),
        )
        .await
        .unwrap();
    }

    fn atom(text: &str) -> ScoreQuery {
        ScoreQuery::Atom(SearchAtom::Title(text.to_string()))
    }

    fn person_atom(names: &[&str]) -> ScoreQuery {
        let names = names
            .iter()
            .map(|n| crate::query::PersonName::parse(n).unwrap())
            .collect();
        ScoreQuery::Atom(SearchAtom::Person(
            crate::query::Person::new(names).unwrap(),
        ))
    }

    fn tag_atom(name: &str, value: Option<&str>) -> ScoreQuery {
        ScoreQuery::Atom(SearchAtom::Tag(crate::query::Tag {
            name: crate::query::TagItem::parse(name).unwrap(),
            value: value.map(|v| crate::query::TagItem::parse(v).unwrap()),
        }))
    }

    fn full_page() -> Pagination {
        Pagination {
            limit: 100,
            offset: 0,
        }
    }

    async fn titles_for(catalogue: &Catalogue, query: ScoreQuery) -> Vec<String> {
        catalogue
            .search(query, full_page())
            .await
            .unwrap()
            .into_iter()
            .map(|s| s.primary_title)
            .collect()
    }

    #[tokio::test]
    async fn matches_single_word_prefix() {
        let catalogue = seeded_catalogue(&["Ave Maria", "Total Praise"]).await;
        assert_eq!(titles_for(&catalogue, atom("Mar")).await, vec!["Ave Maria"]);
    }

    #[tokio::test]
    async fn multi_word_atom_requires_every_word() {
        let catalogue = seeded_catalogue(&["Ave Maria", "Ave Verum"]).await;
        assert_eq!(
            titles_for(&catalogue, atom("ave maria")).await,
            vec!["Ave Maria"]
        );
    }

    #[tokio::test]
    async fn and_requires_both_atoms() {
        let catalogue = seeded_catalogue(&["Ave Maria", "Ave Verum", "Total Praise"]).await;
        let query = ScoreQuery::And(vec![
            AndQuery::Atom(SearchAtom::Title("Ave".to_string())),
            AndQuery::Atom(SearchAtom::Title("Maria".to_string())),
        ]);
        assert_eq!(titles_for(&catalogue, query).await, vec!["Ave Maria"]);
    }

    #[tokio::test]
    async fn or_matches_either_atom() {
        let catalogue = seeded_catalogue(&["Ave Maria", "Ave Verum", "Total Praise"]).await;
        let query = ScoreQuery::Or(vec![
            OrQuery::Atom(SearchAtom::Title("Maria".to_string())),
            OrQuery::Atom(SearchAtom::Title("Praise".to_string())),
        ]);
        assert_eq!(
            titles_for(&catalogue, query).await,
            vec!["Ave Maria", "Total Praise"]
        );
    }

    #[tokio::test]
    async fn not_excludes_matches() {
        let catalogue = seeded_catalogue(&["Ave Maria", "Ave Verum"]).await;
        let query = ScoreQuery::Not(NotQuery::Atom(SearchAtom::Title("Maria".to_string())));
        assert_eq!(titles_for(&catalogue, query).await, vec!["Ave Verum"]);
    }

    /// Exercises the `wrap()` associativity fix directly: `(A | B) & !C`
    /// mixes OR, AND, and NOT in one tree. If a differently-operated
    /// child were left unwrapped, `SQLite`'s left-to-right equal-precedence
    /// grouping would silently compute the wrong set.
    #[tokio::test]
    async fn mixed_operators_compose_correctly() {
        let catalogue = seeded_catalogue(&[
            "Ave Maria",    // matches (Ave | Nope) & !Verum -> kept
            "Ave Verum",    // matches Ave, but excluded by !Verum
            "Total Praise", // doesn't match Ave or Nope at all
        ])
        .await;

        let query = ScoreQuery::And(vec![
            AndQuery::Or(vec![
                OrQuery::Atom(SearchAtom::Title("Ave".to_string())),
                OrQuery::Atom(SearchAtom::Title("Nope".to_string())),
            ]),
            AndQuery::Not(NotQuery::Atom(SearchAtom::Title("Verum".to_string()))),
        ]);

        assert_eq!(titles_for(&catalogue, query).await, vec!["Ave Maria"]);
    }

    #[tokio::test]
    async fn like_metacharacters_are_matched_literally() {
        let catalogue = seeded_catalogue(&["100% Effort", "Total Praise"]).await;
        assert_eq!(
            titles_for(&catalogue, atom("100%")).await,
            vec!["100% Effort"]
        );
    }

    #[tokio::test]
    async fn results_are_sorted_alphabetically_by_primary_title() {
        let catalogue = seeded_catalogue(&["Zebra Song", "Alpha Song", "Middle Song"]).await;
        assert_eq!(
            titles_for(&catalogue, atom("Song")).await,
            vec!["Alpha Song", "Middle Song", "Zebra Song"]
        );
    }

    #[tokio::test]
    async fn pagination_limits_and_offsets_results() {
        let catalogue = seeded_catalogue(&["Alpha Song", "Beta Song", "Gamma Song"]).await;
        let page1 = catalogue
            .search(
                atom("Song"),
                Pagination {
                    limit: 2,
                    offset: 0,
                },
            )
            .await
            .unwrap();
        let page2 = catalogue
            .search(
                atom("Song"),
                Pagination {
                    limit: 2,
                    offset: 2,
                },
            )
            .await
            .unwrap();

        assert_eq!(
            page1
                .into_iter()
                .map(|s| s.primary_title)
                .collect::<Vec<_>>(),
            vec!["Alpha Song", "Beta Song"]
        );
        assert_eq!(
            page2
                .into_iter()
                .map(|s| s.primary_title)
                .collect::<Vec<_>>(),
            vec!["Gamma Song"]
        );
    }

    #[tokio::test]
    async fn person_atom_matches_a_score_with_that_person_attached() {
        let catalogue = seeded_catalogue(&["Ave Maria", "Total Praise"]).await;
        let scores = catalogue.search(atom("Ave"), full_page()).await.unwrap();
        let ave_maria_id = scores[0].id.0;
        let person_id = insert_person(&catalogue, &["Johann", "Bach"]).await;
        attach_person_to_score(&catalogue, ave_maria_id, person_id).await;

        assert_eq!(
            titles_for(&catalogue, person_atom(&["Bach"])).await,
            vec!["Ave Maria"]
        );
    }

    #[tokio::test]
    async fn multi_component_person_atom_requires_every_component_on_the_same_person() {
        let catalogue = seeded_catalogue(&["Ave Maria"]).await;
        let scores = catalogue.search(atom("Ave"), full_page()).await.unwrap();
        let score_id = scores[0].id.0;
        let johann_bach = insert_person(&catalogue, &["Johann", "Bach"]).await;
        attach_person_to_score(&catalogue, score_id, johann_bach).await;

        assert_eq!(
            titles_for(&catalogue, person_atom(&["Johann", "Bach"])).await,
            vec!["Ave Maria"]
        );
    }

    /// Two different people, each holding one of the two queried
    /// components, attached to the same score - the atom must not match,
    /// since its components have to correlate to the *same* person, not
    /// merely the same score (mirrors `compile_title`'s "same title row"
    /// rule, applied to `person_words`/`score_people`).
    #[tokio::test]
    async fn person_atom_does_not_match_components_split_across_different_people() {
        let catalogue = seeded_catalogue(&["Ave Maria"]).await;
        let scores = catalogue.search(atom("Ave"), full_page()).await.unwrap();
        let score_id = scores[0].id.0;
        let anna = insert_person(&catalogue, &["Anna"]).await;
        let berta = insert_person(&catalogue, &["Berta"]).await;
        attach_person_to_score(&catalogue, score_id, anna).await;
        attach_person_to_score(&catalogue, score_id, berta).await;

        assert_eq!(
            titles_for(&catalogue, person_atom(&["Anna", "Berta"])).await,
            Vec::<String>::new()
        );
    }

    #[tokio::test]
    async fn tag_atom_without_value_matches_regardless_of_the_tags_actual_value() {
        let catalogue = seeded_catalogue(&["Ave Maria"]).await;
        let scores = catalogue.search(atom("Ave"), full_page()).await.unwrap();
        let score_id = scores[0].id.0;
        let tag_id = insert_tag(&catalogue, "key").await;
        attach_tag_to_score(&catalogue, score_id, tag_id, Some("CMajor")).await;

        assert_eq!(
            titles_for(&catalogue, tag_atom("key", None)).await,
            vec!["Ave Maria"]
        );
    }

    /// A tag with no value attached at all must still match a bare (no
    /// value) atom - the whole point of `#difficulty` being usable without
    /// a value.
    #[tokio::test]
    async fn tag_atom_without_value_matches_attachments_with_no_value_too() {
        let catalogue = seeded_catalogue(&["Ave Maria"]).await;
        let score_id = catalogue.search(atom("Ave"), full_page()).await.unwrap()[0]
            .id
            .0;
        let tag_id = insert_tag(&catalogue, "sacred").await;
        attach_tag_to_score(&catalogue, score_id, tag_id, None).await;

        assert_eq!(
            titles_for(&catalogue, tag_atom("sacred", None)).await,
            vec!["Ave Maria"]
        );
    }

    /// A `Tag` atom with a value must not match a different score's
    /// attachment of the *same* tag with a different value - value is a
    /// per-attachment (`score_tags`) concept, matched against the specific
    /// attachment's row, not the tag's identity.
    #[tokio::test]
    async fn tag_atom_with_value_does_not_match_a_different_value() {
        let catalogue = seeded_catalogue(&["Ave Maria", "Total Praise"]).await;
        let ave_maria_id = catalogue.search(atom("Ave"), full_page()).await.unwrap()[0]
            .id
            .0;
        let total_praise_id = catalogue.search(atom("Total"), full_page()).await.unwrap()[0]
            .id
            .0;
        let key_tag = insert_tag(&catalogue, "key").await;
        attach_tag_to_score(&catalogue, ave_maria_id, key_tag, Some("CMajor")).await;
        attach_tag_to_score(&catalogue, total_praise_id, key_tag, Some("DMinor")).await;

        assert_eq!(
            titles_for(&catalogue, tag_atom("key", Some("CMajor"))).await,
            vec!["Ave Maria"]
        );
    }

    /// A single score may carry the same tag with several different
    /// values at once (e.g. `#laulupidu:2020` and `#laulupidu:2025`) -
    /// each value-specific query matches it, and the bare (no-value) query
    /// matches it exactly once despite the multiple underlying rows
    /// (`DISTINCT` on `score_id` in `compile_tag`).
    #[tokio::test]
    async fn tag_atom_matches_a_score_with_multiple_values_of_the_same_tag() {
        let catalogue = seeded_catalogue(&["Ave Maria"]).await;
        let score_id = catalogue.search(atom("Ave"), full_page()).await.unwrap()[0]
            .id
            .0;
        let tag_id = insert_tag(&catalogue, "laulupidu").await;
        attach_tag_to_score(&catalogue, score_id, tag_id, Some("2020")).await;
        attach_tag_to_score(&catalogue, score_id, tag_id, Some("2025")).await;

        assert_eq!(
            titles_for(&catalogue, tag_atom("laulupidu", Some("2020"))).await,
            vec!["Ave Maria"]
        );
        assert_eq!(
            titles_for(&catalogue, tag_atom("laulupidu", Some("2025"))).await,
            vec!["Ave Maria"]
        );
        assert_eq!(
            titles_for(&catalogue, tag_atom("laulupidu", None)).await,
            vec!["Ave Maria"]
        );
    }

    #[tokio::test]
    async fn person_and_title_atoms_combine_with_and() {
        let catalogue = seeded_catalogue(&["Ave Maria", "Total Praise"]).await;
        let ave_maria_id = catalogue.search(atom("Ave"), full_page()).await.unwrap()[0]
            .id
            .0;
        let total_praise_id = catalogue.search(atom("Total"), full_page()).await.unwrap()[0]
            .id
            .0;
        let anna = insert_person(&catalogue, &["Anna"]).await;
        attach_person_to_score(&catalogue, ave_maria_id, anna).await;
        attach_person_to_score(&catalogue, total_praise_id, anna).await;

        let query = ScoreQuery::And(vec![
            AndQuery::Atom(SearchAtom::Title("Ave".to_string())),
            match person_atom(&["Anna"]) {
                ScoreQuery::Atom(atom) => AndQuery::Atom(atom),
                _ => unreachable!(),
            },
        ]);
        assert_eq!(titles_for(&catalogue, query).await, vec!["Ave Maria"]);
    }

    /// Alphabetically `"Ave Maria"` sorts before `"Maria"`, so this only
    /// passes if the exact-whole-title-match boost outranks it.
    #[tokio::test]
    async fn exact_whole_title_match_ranks_above_a_title_that_merely_contains_the_word() {
        let catalogue = seeded_catalogue(&["Maria", "Ave Maria"]).await;
        assert_eq!(
            titles_for(&catalogue, atom("Maria")).await,
            vec!["Maria", "Ave Maria"]
        );
    }

    /// Alphabetically `"Alpha Ave"` sorts before `"Ave Verum"`, so this
    /// only passes if the prefix-of-whole-title boost outranks it.
    #[tokio::test]
    async fn prefix_of_whole_title_ranks_above_a_non_prefix_match_that_sorts_earlier() {
        let catalogue = seeded_catalogue(&["Alpha Ave", "Ave Verum"]).await;
        assert_eq!(
            titles_for(&catalogue, atom("Ave")).await,
            vec!["Ave Verum", "Alpha Ave"]
        );
    }

    #[tokio::test]
    async fn more_distinct_matched_words_ranks_higher() {
        let catalogue = seeded_catalogue(&["Ave Verum", "Maria Callas", "Ave Maria"]).await;
        let query = ScoreQuery::Or(vec![
            OrQuery::Atom(SearchAtom::Title("ave".to_string())),
            OrQuery::Atom(SearchAtom::Title("maria".to_string())),
        ]);
        assert_eq!(
            titles_for(&catalogue, query).await,
            vec!["Ave Maria", "Ave Verum", "Maria Callas"]
        );
    }

    /// Neither title's *whole* value starts with "mar", so the boost tier
    /// doesn't apply to either - this isolates the word-match-quality
    /// component: "mars" (4 letters) is a 3/4 match, "marseille" (9
    /// letters) only a 3/9 match. A flat word-count scheme would tie both
    /// at "1 word matched" and fall back to alphabetical order (`"Ave
    /// Marseille"` first) - this only passes if match quality is weighed.
    #[tokio::test]
    async fn a_more_complete_word_match_ranks_above_a_shorter_partial_match() {
        let catalogue = seeded_catalogue(&["Ave Marseille", "Total Mars"]).await;
        assert_eq!(
            titles_for(&catalogue, atom("mar")).await,
            vec!["Total Mars", "Ave Marseille"]
        );
    }

    #[tokio::test]
    async fn person_only_query_keeps_alphabetical_order_with_no_ranking_signal() {
        let catalogue = seeded_catalogue(&["Zebra Song", "Alpha Song"]).await;
        let zebra_id = catalogue.search(atom("Zebra"), full_page()).await.unwrap()[0]
            .id
            .0;
        let alpha_id = catalogue.search(atom("Alpha"), full_page()).await.unwrap()[0]
            .id
            .0;

        let anna = insert_person(&catalogue, &["Anna"]).await;
        attach_person_to_score(&catalogue, zebra_id, anna).await;
        attach_person_to_score(&catalogue, alpha_id, anna).await;

        assert_eq!(
            titles_for(&catalogue, person_atom(&["Anna"])).await,
            vec!["Alpha Song", "Zebra Song"]
        );
    }
}
