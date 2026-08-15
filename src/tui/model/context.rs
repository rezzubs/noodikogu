pub mod search;

use crate::catalogue::ScoreDetail;

use search::SearchContext;

/// The currently visible context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Context {
    Search(SearchContext),
    Score(ScoreContext),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ScoreContext {
    detail: ScoreDetail,
}
