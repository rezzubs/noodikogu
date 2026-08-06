//! `sea-query` table/column identifiers, shared by every module that builds
//! SQL against the catalogue schema (`search`, `score`, `title`). Mirrors
//! `src/catalogue/migration/m0001_scores_and_titles.sql` by hand - nothing
//! checks the two stay in sync automatically; the tests throughout this
//! module are what would catch drift.

#[derive(sea_query::Iden)]
pub(super) enum Scores {
    Table,
    Id,
    Description,
    CreatedAt,
}

#[derive(sea_query::Iden)]
pub(super) enum Titles {
    Table,
    Id,
    ScoreId,
    Value,
    ValueNormalized,
    IsPrimary,
}

#[derive(sea_query::Iden)]
pub(super) enum TitleWords {
    Table,
    TitleId,
    Word,
}

#[derive(sea_query::Iden)]
pub(super) enum People {
    Table,
    Id,
    DisplayName,
    DisplayNameKey,
}

#[derive(sea_query::Iden)]
pub(super) enum PersonWords {
    Table,
    PersonId,
    Word,
}

#[derive(sea_query::Iden)]
pub(super) enum ScorePeople {
    Table,
    ScoreId,
    PersonId,
}

#[derive(sea_query::Iden)]
pub(super) enum Tags {
    Table,
    Id,
    Name,
    NameNormalized,
}

#[derive(sea_query::Iden)]
pub(super) enum ScoreTags {
    Table,
    ScoreId,
    TagId,
    Value,
    ValueNormalized,
}
