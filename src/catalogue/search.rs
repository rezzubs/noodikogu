//! Evaluates `crate::query::ScoreQuery` against the catalogue.

mod eval;

use super::schema::Titles;
use super::{Catalogue, DatabaseError, ScoreId, params};
use crate::query::ScoreQuery;
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
    /// Evaluates `query` and returns one page of matching scores, sorted
    /// alphabetically (case/diacritic-insensitively) by primary title.
    /// Relevance ranking is deferred
    ///
    /// # Errors
    ///
    /// Returns [`SearchError::Db`] on an underlying database failure.
    ///
    /// # Panics
    ///
    /// Panics if `query` contains a `Person` or `Tag` search atom - not
    /// yet supported, see `docs/decisions/0005-hand-rolled-search.md`.
    pub async fn search(
        &self,
        query: ScoreQuery,
        pagination: Pagination,
    ) -> Result<Vec<ScoreSummary>, SearchError> {
        let conn = self.connect().await?;

        let ids = eval::compile_score_query(query);
        let hydrated = hydrate(ids, pagination);
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
/// sorts alphabetically, and applies `pagination`.
///
/// Relies on every score having exactly one primary title at all times
/// (guaranteed by `create_score` and `set_primary_title`; `remove_title`
/// refuses to remove a score's current primary title rather than leaving
/// this unsatisfied - see `title.rs`). If that invariant were ever
/// violated, the affected score would silently drop out of results.
fn hydrate(ids: sea_query::SelectStatement, pagination: Pagination) -> sea_query::SelectStatement {
    let matches = Alias::new("matches");
    Query::select()
        .expr_as(
            Expr::col((matches.clone(), Alias::new("score_id"))),
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
                .equals((matches, Alias::new("score_id")))
                .and(Expr::col((Titles::Table, Titles::IsPrimary)).eq(1)),
        )
        .order_by((Titles::Table, Titles::ValueNormalized), Order::Asc)
        .limit(pagination.limit)
        .offset(pagination.offset)
        .take()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalogue::migration;
    use crate::catalogue::normalize::{normalize_text, words};
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

    async fn insert_score(catalogue: &Catalogue, title: &str) {
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
    }

    fn atom(text: &str) -> ScoreQuery {
        ScoreQuery::Atom(SearchAtom::Title(text.to_string()))
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
}
