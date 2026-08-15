//! Search-mode Browser state: a tile cache backing a query's score list,
//! plus the selection/scroll state layered on top of it.

use std::collections::VecDeque;

use crate::catalogue::{Catalogue, Pagination, ScoreDetailError, ScoreSummary};
use crate::query::ScoreQuery;
use crate::tui::Effect;

/// Results per fetched [`Tile`].
///
/// Fixed rather than derived from the viewport: scrolling here is smooth with
/// scrolloff and there's no reason for the fetch granularity to track window
/// size - doing so would leave already-fetched tiles at a stale size across a
/// resize for no benefit.
///
/// `pub(crate)`: `tui.rs` needs this to build the bootstrap pagination for
/// an [`Effect::SearchInvalidated`] reset - see [`SearchContext::is_tile_stale`].
pub(crate) const TILE_SIZE: u64 = 32;

/// Extra tiles to keep buffered beyond exactly covering the viewport, in
/// each direction from the *selected* tile - so scrolling doesn't
/// immediately need a fetch on every keypress.
const TILE_CACHE_MARGIN: usize = 1;

/// How many tiles must be buffered on one side of the selected tile
/// to guarantee `viewport_height` rows of content are available there,
/// [`TILE_CACHE_MARGIN`] tiles of lookahead included.
///
/// A fixed tile count anchored on the selected tile regardless of
/// `viewport_height` would undercount on a tall terminal (more visible rows
/// than `TILE_CACHE_MARGIN` tiles' worth) - "a typical terminal height" isn't a
/// safe upper bound, a maximized window on a large display can comfortably show
/// well over 64 rows.
fn tiles_needed_for(viewport_height: u16) -> usize {
    let tile_size = usize::try_from(TILE_SIZE).expect("TILE_SIZE fits usize on any real platform");
    usize::from(viewport_height).div_ceil(tile_size) + TILE_CACHE_MARGIN
}

/// Rows of context kept between the selection and the viewport's top/bottom
/// edge while scrolling, unless not enough buffered rows exist on that side
/// to honor it - mirrors `command_line.rs`'s own `SCROLLOFF`, see
/// docs/tui.md's "Fetching" section ("smooth and scrolloff-style").
const SCROLLOFF: u16 = 2;

/// [`SCROLLOFF`], shrunk so it still fits within `viewport_height` - mirrors
/// `command_line::scroll`'s `effective_scrolloff`, for the same reason: a
/// margin on both edges needs `viewport_height >= 2 * SCROLLOFF + 1` to be
/// achievable at all.
fn effective_scrolloff(viewport_height: u16) -> u16 {
    SCROLLOFF.min(viewport_height.saturating_sub(1) / 2)
}

/// Search-mode Browser state.
///
/// [`Model::generation`] (not strictly increasing - see its own docs for why)
/// is threaded through every fetch [`Effect`] / response `Message` so a
/// result that arrives after the query has since changed can be dropped
/// instead of corrupting a newer [`SearchContext`] - see [`Model::update`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchContext {
    /// The query `results` was fetched with - needed to fetch further tiles
    /// at new offsets.
    query: ScoreQuery,
    /// The actual search results, shape depends on the query.
    results: SearchResults,
    /// The currently selected item. Always points at a [`TileState::Loaded`]
    /// tile with a non-empty `results`.
    ///
    /// Every method that could move it elsewhere refuses to if the destination
    /// doesn't hold up that invariant.
    selection: Selection,
    /// Row of the selected item, relative to the top of the viewport.
    selection_position: u16,
}

impl SearchContext {
    /// Moves the selection to the previous row, scrolling if needed.
    ///
    /// A no-op if already on the very first buffered row, or if the previous
    /// row would require crossing into a tile that isn't [`TileState::Loaded`]
    /// yet.
    pub(crate) fn move_up(&mut self, generation: u64, viewport_height: u16) -> Option<Effect> {
        if self.selection.item > 0 {
            // Still within the same tile.

            self.selection.item -= 1;
            self.selection_position = self.clamp_top_margin(viewport_height);
            return self.reconcile(generation, viewport_height);
        }

        // Need to switch tiles.

        // Returns None if we're already on the first element.
        let previous_index = self.selection.tile.checked_sub(1)?;

        let SearchResults::Score(tiles) = &self.results;
        let previous_len = tiles[previous_index]
            .loaded()
            .map(|tile| tile.results.len())?;

        // Assert for the subtraction below
        assert_eq!(
            previous_len,
            usize::try_from(TILE_SIZE).expect("known to fit into usize"),
            "Expected to resolve at fetch time by `is_tile_stale`"
        );

        self.selection.tile = previous_index;
        self.selection.item = previous_len - 1;
        self.selection_position = self.clamp_top_margin(viewport_height);
        self.reconcile(generation, viewport_height)
    }

    /// Symmetric with [`Self::move_up`].
    pub(crate) fn move_down(&mut self, generation: u64, viewport_height: u16) -> Option<Effect> {
        let SearchResults::Score(tiles) = &self.results;
        let current_len = tiles[self.selection.tile]
            .loaded()
            .expect("selection always points at a loaded tile")
            .results
            .len();

        if self.selection.item + 1 < current_len {
            self.selection.item += 1;
            self.selection_position =
                self.clamp_bottom_margin(self.selection_position + 1, viewport_height);
            return self.reconcile(generation, viewport_height);
        }

        let next_index = self.selection.tile + 1;
        if next_index >= tiles.len() {
            return None;
        }

        let next_len = tiles[next_index].loaded().map(|tile| tile.results.len())?;
        if next_len == 0 {
            // Empty tiles can happen if we have a full tile and the next
            // tile's search result turns up nothing. Nothing in this case is
            // still useful information; if we didn't store the empty tile then
            // there's no way to tell that case aprafr form "we just haven't
            // checked yet".
            return None;
        }

        self.selection.tile = next_index;
        self.selection.item = 0;
        self.selection_position =
            self.clamp_bottom_margin(self.selection_position + 1, viewport_height);
        self.reconcile(generation, viewport_height)
    }

    /// Keeps the selected row at the same on-screen position across a
    /// resize where possible, pulling it up just enough to stay visible
    /// (and to re-honor the scrolloff margin) otherwise.
    pub(crate) fn handle_resize(
        &mut self,
        generation: u64,
        new_viewport_height: u16,
    ) -> Option<Effect> {
        self.selection_position =
            self.clamp_bottom_margin(self.selection_position, new_viewport_height);
        self.reconcile(generation, new_viewport_height)
    }

    /// Pulls `naive_position` in from the bottom edge by
    /// [`effective_scrolloff`] rows, unless fewer than that many rows are
    /// actually buffered after the selection.
    ///
    /// Also clamps to the viewport's bottom edge, so callers don't need their
    /// own `.min(viewport_height - 1)`.
    fn clamp_bottom_margin(&self, naive_position: u16, viewport_height: u16) -> u16 {
        let max_position = viewport_height.saturating_sub(1);
        let naive_position = naive_position.min(max_position);
        let margin = effective_scrolloff(viewport_height);
        let capped = max_position.saturating_sub(margin);
        if naive_position > capped && self.rows_after(margin) >= margin {
            capped
        } else {
            naive_position
        }
    }

    /// Pulls the position one row up from the top edge by
    /// [`effective_scrolloff`] rows, unless fewer than that many rows are
    /// actually buffered before the selection.
    fn clamp_top_margin(&self, viewport_height: u16) -> u16 {
        let naive_position = self.selection_position.saturating_sub(1);
        let margin = effective_scrolloff(viewport_height);
        if naive_position < margin && self.rows_before(margin) >= margin {
            margin
        } else {
            naive_position
        }
    }

    /// Counts up to `limit` loaded, non-empty rows available strictly after
    /// the current selection, without moving it.
    ///
    /// Stops early at the true end of the buffered results (an out-of-range
    /// or empty tile) - same story as the true tail exemption on
    /// [`Self::is_tile_stale`]. Can also stop early at a tile that's merely
    /// still `Loading`; that's fine, since [`Self::clamp_bottom_margin`]
    /// treats "fewer than `limit` rows found" and "genuinely at the end" the
    /// same way (relax the margin), and a subsequent move re-evaluates once
    /// the fetch resolves.
    fn rows_after(&self, limit: u16) -> u16 {
        let SearchResults::Score(tiles) = &self.results;
        let mut tile_index = self.selection.tile;
        let mut item_index = self.selection.item;
        let mut count = 0;
        while count < limit {
            let Some(tile) = tiles.get(tile_index).and_then(TileState::loaded) else {
                break;
            };
            if item_index + 1 < tile.results.len() {
                item_index += 1;
            } else {
                let Some(next) = tiles.get(tile_index + 1).and_then(TileState::loaded) else {
                    break;
                };
                if next.results.is_empty() {
                    break;
                }
                tile_index += 1;
                item_index = 0;
            }
            count += 1;
        }
        count
    }

    /// Symmetric with [`Self::rows_after`], counting backward instead.
    ///
    /// Every non-tail tile is `TILE_SIZE` long (see [`Self::is_tile_stale`]),
    /// so a tile reached by counting backward from the selection is never
    /// empty in practice - `results.len().saturating_sub(1)` is just belt
    /// and suspenders against that, not a case expected to actually fire.
    fn rows_before(&self, limit: u16) -> u16 {
        let SearchResults::Score(tiles) = &self.results;
        let mut tile_index = self.selection.tile;
        let mut item_index = self.selection.item;
        let mut count = 0;
        while count < limit {
            if item_index > 0 {
                item_index -= 1;
            } else {
                let Some(previous_index) = tile_index.checked_sub(1) else {
                    break;
                };
                let Some(previous) = tiles.get(previous_index).and_then(TileState::loaded) else {
                    break;
                };
                tile_index = previous_index;
                item_index = previous.results.len().saturating_sub(1);
            }
            count += 1;
        }
        count
    }

    /// Applies a completed background fetch.
    ///
    /// Returns:
    /// - [`TileFetchOutcome::Applied`]: the normal case. Carries whatever
    ///   further [`Effect`] reconciling produced, if any. Also covers
    ///   a `pagination` with no matching `Loading` slot - that tile was
    ///   independently evicted while the fetch was in flight, so there's
    ///   nothing to apply, but it's not an error.
    /// - [`TileFetchOutcome::Stale`]: The caller must discard the whole context
    ///   and restart the search from scratch instead of inserting this tile.
    pub(crate) fn handle_tile_fetch(
        &mut self,
        generation: u64,
        pagination: Pagination,
        results: Vec<ScoreResult>,
        viewport_height: u16,
    ) -> TileFetchOutcome {
        if self.is_tile_stale(pagination, results.len()) {
            return TileFetchOutcome::Stale;
        }

        let SearchResults::Score(tiles) = &mut self.results;
        if let Some(slot) = tiles.iter_mut().find(|state| {
            matches!(state, TileState::Loading { .. }) && state.pagination() == pagination
        }) {
            *slot = TileState::Loaded(Tile {
                pagination,
                results: results.into_boxed_slice(),
            });
        }

        TileFetchOutcome::Applied(self.reconcile(generation, viewport_height))
    }

    /// `true` if a just-resolved tile's length proves the cache is stale.
    ///
    /// Under no concurrent mutation, every tile except the true tail of the
    /// result set is always exactly [`TILE_SIZE`] long: [`Self::ensure_buffered`]
    /// only ever requests full-size pages ahead of an already-loaded,
    /// non-empty neighbor. So a shorter tile anywhere else means something
    /// changed underneath the cache between two independent fetches -
    /// regardless of *why*. The true tail (the deque's current back tile)
    /// is exempt: a short or empty last page is completely normal.
    fn is_tile_stale(&self, pagination: Pagination, result_count: usize) -> bool {
        let tile_size =
            usize::try_from(TILE_SIZE).expect("TILE_SIZE fits usize on any real platform");
        if result_count >= tile_size {
            return false;
        }

        let SearchResults::Score(tiles) = &self.results;
        tiles.back().map(TileState::pagination) != Some(pagination)
    }

    /// Builds a fresh `SearchContext` from a bootstrap fetch's first tile, then
    /// immediately [reconciles](Self::reconcile) - prefetching further tiles
    /// right away if the viewport needs more than this first one covers.
    ///
    /// `None` if `results` is empty - a query whose first page comes back
    /// with zero matches isn't handled by this constructor. `selection`
    /// would otherwise point at a non-existent item, breaking the invariant
    /// documented on that field above.
    pub(crate) fn bootstrap(
        query: ScoreQuery,
        generation: u64,
        results: Vec<ScoreResult>,
        viewport_height: u16,
    ) -> Option<(Self, Option<Effect>)> {
        if results.is_empty() {
            return None;
        }

        let tile = TileState::Loaded(Tile {
            pagination: Pagination {
                offset: 0,
                limit: TILE_SIZE,
            },
            results: results.into_boxed_slice(),
        });
        let mut context = Self {
            query,
            results: SearchResults::Score(VecDeque::from([tile])),
            selection: Selection { tile: 0, item: 0 },
            selection_position: 0,
        };
        let effect = context.reconcile(generation, viewport_height);
        Some((context, effect))
    }

    /// Evicts tiles that fell out of range, then tops up the cache if either
    /// edge is now short.
    ///
    /// See [`Self::evict`]/[`Self::ensure_buffered`].
    fn reconcile(&mut self, generation: u64, viewport_height: u16) -> Option<Effect> {
        self.evict(viewport_height);
        self.ensure_buffered(generation, viewport_height)
    }

    /// Evicts tiles that fell out of range.
    fn evict(&mut self, viewport_height: u16) {
        let needed = tiles_needed_for(viewport_height);
        let SearchResults::Score(tiles) = &mut self.results;

        while self.selection.tile > needed {
            tiles.pop_front();
            self.selection.tile -= 1;
        }
        while tiles.len() - 1 - self.selection.tile > needed {
            tiles.pop_back();
        }
    }

    /// Requests one more tile if either edge of the cache is short of
    /// [`tiles_needed_for`], giving priority to the backward direction. At
    /// most one [`Effect`] per call - if both edges are short, the other
    /// side isn't caught until a later call.
    ///
    /// Every mutating method here (`move_up`/`move_down`/
    /// [`Self::handle_tile_fetch`]) ends by calling this through
    /// [`Self::reconcile`], and [`Self::handle_tile_fetch`] itself runs on
    /// every resolved [`Effect::FetchScoreTile`] - so a still-short edge keeps
    /// requesting the next tile it needs, one per fetch response, fully driven
    /// by fetch resolutions with no further scrolling required, until both
    /// edges are satisfied and this returns `None`.
    fn ensure_buffered(&mut self, generation: u64, viewport_height: u16) -> Option<Effect> {
        let needed = tiles_needed_for(viewport_height);
        let query = self.query.clone();
        let SearchResults::Score(tiles) = &mut self.results;

        if self.selection.tile < needed {
            let front = tiles
                .front()
                .expect("selection.tile is valid, so tiles is non-empty");
            if let TileState::Loaded(tile) = front
                && tile.pagination.offset > 0
            {
                let pagination = Pagination {
                    offset: tile.pagination.offset.saturating_sub(TILE_SIZE),
                    limit: TILE_SIZE,
                };
                tiles.push_front(TileState::Loading { pagination });
                // Everything shifted right by one slot in the deque.
                self.selection.tile += 1;
                return Some(Effect::FetchScoreTile {
                    generation,
                    query,
                    pagination,
                });
            }
        }

        if tiles.len() - 1 - self.selection.tile < needed {
            let back = tiles
                .back()
                .expect("selection.tile is valid, so tiles is non-empty");
            // An empty tile can only mean the true end of the result set was
            // already reached - fetching past it would just request the
            // same empty page forever.
            if let TileState::Loaded(tile) = back
                && !tile.results.is_empty()
            {
                let results_len = u64::try_from(tile.results.len())
                    .expect("a tile's result count fits in u64 on any real platform");
                let pagination = Pagination {
                    offset: tile.pagination.offset + results_len,
                    limit: TILE_SIZE,
                };
                tiles.push_back(TileState::Loading { pagination });
                return Some(Effect::FetchScoreTile {
                    generation,
                    query,
                    pagination,
                });
            }
        }

        None
    }
}

/// Outcome of [`SearchContext::handle_tile_fetch`].
#[derive(Debug)]
pub(crate) enum TileFetchOutcome {
    /// The tile was applied normally (or discarded because its `Loading` slot
    /// no longer existed - see [`SearchContext::handle_tile_fetch`]'s own
    /// docs). Carries whatever `Effect` reconciling produced, if any.
    Applied(Option<Effect>),
    /// The tile was determined as stale - the caller must discard the whole
    /// context and restart the search rather than apply it.
    Stale,
}

/// The currently selected item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Selection {
    /// The tile that contains the selection.
    tile: usize,
    /// The selected item in the tile.
    item: usize,
}

/// A cached search result from the catalogue.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Tile<T> {
    /// The pagination that was used to fetch this tile. Used to fetch the
    /// following tiles.
    pagination: Pagination,
    /// The actual results.
    results: Box<[T]>,
}

/// A tile that has been requested but not necessarily loaded yet
///
/// Fetches run in the background, so a tile's slot in the deque has to exist
/// (keeping index math consistent) before its data has actually arrived. Only
/// one tile is fetched at a time.
///
/// Because we buffer tiles off-screen it's unlikely that a user will actually
/// hit this loading state unless database reads become extremely slow.
#[derive(Debug, Clone, PartialEq, Eq)]
enum TileState<T> {
    /// The tile is still loading.
    Loading {
        /// The pagination that was used to fetch this tile.
        pagination: Pagination,
    },
    /// The tile is loaded.
    Loaded(Tile<T>),
}

impl<T> TileState<T> {
    /// The pagination that was used to fetch this tile.
    fn pagination(&self) -> Pagination {
        match self {
            Self::Loading { pagination } | Self::Loaded(Tile { pagination, .. }) => *pagination,
        }
    }

    /// Map `Loading` into `None` and `Loaded` into `Some`.
    fn loaded(&self) -> Option<&Tile<T>> {
        match self {
            Self::Loading { .. } => None,
            Self::Loaded(tile) => Some(tile),
        }
    }
}

/// The results fetched.
#[derive(Debug, Clone, PartialEq, Eq)]
enum SearchResults {
    // TODO: People and tag results.
    /// Results from a "score" query.
    Score(VecDeque<TileState<ScoreResult>>),
}

/// A single row representing a score in the table of results.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ScoreResult {
    /// The summary returned by querying the catalogue.
    summary: ScoreSummary,
    /// The display names of the people associated with the score.
    people: Vec<String>,
}

/// Builds an arbitrary, otherwise-meaningless `ScoreResult` for tests
/// outside this module that only need *some* result to fill a tile with -
/// `ScoreResult`'s own fields are private, so nothing outside `search.rs`
/// can construct one directly.
#[cfg(test)]
pub(crate) fn test_score_result(id: i64) -> ScoreResult {
    ScoreResult {
        summary: ScoreSummary {
            id: crate::catalogue::ScoreId(id),
            primary_title: format!("Test score {id}"),
        },
        people: Vec::new(),
    }
}

/// Fetches one tile's worth of scores for `query`/`pagination`, hydrating
/// each with its attached people. The async body executed by the runtime
/// for an [`Effect::FetchScoreTile`].
///
/// # Errors
///
/// [`ScoreDetailError`] if the search failed, no cases handled by this
/// function.
pub(crate) async fn fetch_tile(
    catalogue: &Catalogue,
    query: &ScoreQuery,
    pagination: Pagination,
) -> Result<Vec<ScoreResult>, ScoreDetailError> {
    let summaries = catalogue.search(query.clone(), pagination).await?;

    let mut results = Vec::with_capacity(summaries.len());
    for summary in summaries {
        let people = catalogue
            .score_people(summary.id)
            .await?
            .into_iter()
            .map(|person| person.display_name)
            .collect();
        results.push(ScoreResult { summary, people });
    }
    Ok(results)
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;
    use crate::catalogue::{ScoreId, Title};
    use crate::query::{PersonName, SearchAtom};

    fn score_result(id: i64, title: &str) -> ScoreResult {
        ScoreResult {
            summary: ScoreSummary {
                id: ScoreId(id),
                primary_title: title.to_string(),
            },
            people: Vec::new(),
        }
    }

    fn atom_query(title: &str) -> ScoreQuery {
        ScoreQuery::Atom(SearchAtom::Title(title.to_string()))
    }

    /// A full-size (`TILE_SIZE`-long) run of results, standing in for a
    /// realistic non-tail tile - every non-tail `Loaded` tile is exactly
    /// this long, see [`SearchContext::is_tile_stale`].
    fn full_tile_results() -> Vec<ScoreResult> {
        let tile_size = usize::try_from(TILE_SIZE).expect("TILE_SIZE fits usize");
        (1..=tile_size)
            .map(|id| {
                score_result(
                    i64::try_from(id).expect("small id fits i64"),
                    "full tile item",
                )
            })
            .collect()
    }

    /// Builds a `SearchContext` with `tiles` already loaded (no `Loading`
    /// placeholders) and the selection at `(tile, item)` - the state a real
    /// context would be in once its bootstrap fetch(es) have resolved.
    fn context(
        tiles: impl IntoIterator<Item = Vec<ScoreResult>>,
        selection: (usize, usize),
        selection_position: u16,
    ) -> SearchContext {
        let mut offset = 0u64;
        let tiles = tiles
            .into_iter()
            .map(|results| {
                let pagination = Pagination {
                    offset,
                    limit: TILE_SIZE,
                };
                offset += u64::try_from(results.len()).expect("test fixtures stay small");
                TileState::Loaded(Tile {
                    pagination,
                    results: results.into_boxed_slice(),
                })
            })
            .collect();

        SearchContext {
            query: atom_query("test"),
            results: SearchResults::Score(tiles),
            selection: Selection {
                tile: selection.0,
                item: selection.1,
            },
            selection_position,
        }
    }

    #[test]
    fn move_down_advances_within_a_tile() {
        let mut ctx = context(
            [vec![score_result(1, "a"), score_result(2, "b")]],
            (0, 0),
            0,
        );
        // Not asserting on the return value here: `tiles_needed_for(10)` (2)
        // is bigger than this tiny fixture, so `reconcile` always has a
        // fetch to issue - that's covered by the `ensure_buffered_*` tests
        // below, this test only cares about the resulting selection.
        ctx.move_down(0, 10);
        assert_eq!(ctx.selection, Selection { tile: 0, item: 1 });
        assert_eq!(ctx.selection_position, 1);
    }

    #[test]
    fn move_down_crosses_into_the_next_loaded_tile() {
        // Tile 0 isn't the tail, so it must be full-size.
        let tile_size = usize::try_from(TILE_SIZE).expect("TILE_SIZE fits usize");
        let mut ctx = context(
            [full_tile_results(), vec![score_result(999, "b")]],
            (0, tile_size - 1),
            0,
        );
        ctx.move_down(0, 10);
        assert_eq!(ctx.selection, Selection { tile: 1, item: 0 });
        assert_eq!(ctx.selection_position, 1);
    }

    #[test]
    fn move_down_refuses_to_cross_into_a_loading_tile() {
        let mut ctx = context([vec![score_result(1, "a")]], (0, 0), 0);
        let SearchResults::Score(tiles) = &mut ctx.results;
        tiles.push_back(TileState::Loading {
            pagination: Pagination {
                offset: 1,
                limit: TILE_SIZE,
            },
        });

        assert!(ctx.move_down(0, 10).is_none());
        assert_eq!(ctx.selection, Selection { tile: 0, item: 0 });
    }

    #[test]
    fn move_down_clamps_selection_position_to_the_viewport() {
        let mut ctx = context(
            [vec![
                score_result(1, "a"),
                score_result(2, "b"),
                score_result(3, "c"),
            ]],
            (0, 1),
            1,
        );
        // viewport_height 2 -> the last visible row is index 1.
        ctx.move_down(0, 2);
        assert_eq!(ctx.selection, Selection { tile: 0, item: 2 });
        assert_eq!(ctx.selection_position, 1);
    }

    #[test]
    fn move_up_retreats_within_a_tile() {
        let mut ctx = context(
            [vec![score_result(1, "a"), score_result(2, "b")]],
            (0, 1),
            1,
        );
        ctx.move_up(0, 10);
        assert_eq!(ctx.selection, Selection { tile: 0, item: 0 });
        assert_eq!(ctx.selection_position, 0);
    }

    #[test]
    fn move_up_crosses_into_the_previous_loaded_tile() {
        // Tile 0 isn't the tail, so it must be full-size.
        let tile_size = usize::try_from(TILE_SIZE).expect("TILE_SIZE fits usize");
        let mut ctx = context(
            [full_tile_results(), vec![score_result(999, "c")]],
            (1, 0),
            1,
        );
        ctx.move_up(0, 10);
        assert_eq!(
            ctx.selection,
            Selection {
                tile: 0,
                item: tile_size - 1
            }
        );
        // Plenty of rows are buffered before the new selection (31 of them,
        // within tile 0), so the scrolloff margin holds instead of riding
        // all the way to row 0.
        assert_eq!(ctx.selection_position, SCROLLOFF);
    }

    #[test]
    fn move_up_is_a_no_op_on_the_very_first_row() {
        let mut ctx = context([vec![score_result(1, "a")]], (0, 0), 0);
        assert!(ctx.move_up(0, 10).is_none());
        assert_eq!(ctx.selection, Selection { tile: 0, item: 0 });
    }

    #[test]
    fn move_down_holds_the_bottom_scrolloff_margin_when_more_rows_remain() {
        let ids = 1..=10;
        let tile = ids.map(|id| score_result(id, "a")).collect();
        // viewport_height 8 -> effective_scrolloff is 2, so row 5 (index
        // `max_position - SCROLLOFF` = 7 - 2) is the bottom margin boundary.
        let mut ctx = context([tile], (0, 4), 5);
        ctx.move_down(0, 8);
        assert_eq!(ctx.selection, Selection { tile: 0, item: 5 });
        // 4 rows remain after the new selection (indices 6-9) - more than
        // enough to hold the margin, so the position doesn't advance.
        assert_eq!(ctx.selection_position, 5);
    }

    #[test]
    fn move_down_lets_the_position_ride_past_the_margin_near_the_true_end() {
        let tile = vec![
            score_result(1, "a"),
            score_result(2, "b"),
            score_result(3, "c"),
            score_result(4, "d"),
        ];
        let mut ctx = context([tile], (0, 1), 6);
        ctx.move_down(0, 8);
        assert_eq!(ctx.selection, Selection { tile: 0, item: 2 });
        // Only 1 row remains after the new selection (index 3) - fewer than
        // `effective_scrolloff(8)` (2), so the margin gives way rather than
        // holding the position back.
        assert_eq!(ctx.selection_position, 7);
    }

    #[test]
    fn move_up_holds_the_top_scrolloff_margin_when_more_rows_remain() {
        let ids = 1..=10;
        let tile = ids.map(|id| score_result(id, "a")).collect();
        let mut ctx = context([tile], (0, 5), 2);
        ctx.move_up(0, 8);
        assert_eq!(ctx.selection, Selection { tile: 0, item: 4 });
        // 4 rows remain before the new selection (indices 0-3) - more than
        // enough to hold the margin, so the position doesn't retreat.
        assert_eq!(ctx.selection_position, 2);
    }

    #[test]
    fn move_up_lets_the_position_ride_past_the_margin_near_the_true_start() {
        let tile = vec![
            score_result(1, "a"),
            score_result(2, "b"),
            score_result(3, "c"),
            score_result(4, "d"),
        ];
        let mut ctx = context([tile], (0, 2), 1);
        ctx.move_up(0, 8);
        assert_eq!(ctx.selection, Selection { tile: 0, item: 1 });
        // Only 1 row remains before the new selection (index 0) - fewer than
        // `effective_scrolloff(8)` (2), so the margin gives way rather than
        // holding the position back.
        assert_eq!(ctx.selection_position, 0);
    }

    #[test]
    fn effective_scrolloff_shrinks_for_a_short_viewport() {
        assert_eq!(effective_scrolloff(10), SCROLLOFF);
        assert_eq!(effective_scrolloff(5), 2);
        assert_eq!(effective_scrolloff(4), 1);
        assert_eq!(effective_scrolloff(1), 0);
    }

    #[test]
    fn resize_keeps_the_selection_in_place_when_it_still_fits() {
        let mut ctx = context([vec![score_result(1, "a")]], (0, 0), 3);
        ctx.handle_resize(0, 10);
        assert_eq!(ctx.selection_position, 3);
    }

    #[test]
    fn resize_pulls_the_selection_up_when_it_no_longer_fits() {
        let mut ctx = context([vec![score_result(1, "a")]], (0, 0), 8);
        ctx.handle_resize(0, 5);
        assert_eq!(ctx.selection_position, 4);
    }

    #[test]
    fn evict_drops_a_tile_beyond_the_cache_radius_ahead_of_the_selection() {
        // 4 tiles, selection on the first (index 0), viewport_height 10
        // (tiles_needed_for(10) == 2): tiles 1 and 2 are within range, tile
        // 3 is one past it.
        let mut ctx = context(
            [
                vec![score_result(1, "a")],
                vec![score_result(2, "b")],
                vec![score_result(3, "c")],
                vec![score_result(4, "d")],
            ],
            (0, 0),
            0,
        );
        ctx.evict(10);
        let SearchResults::Score(tiles) = &ctx.results;
        assert_eq!(tiles.len(), 3);
        assert_eq!(ctx.selection, Selection { tile: 0, item: 0 });
    }

    #[test]
    fn evict_drops_a_tile_behind_the_selection_and_shifts_its_index() {
        let mut ctx = context(
            [
                vec![score_result(1, "a")],
                vec![score_result(2, "b")],
                vec![score_result(3, "c")],
                vec![score_result(4, "d")],
            ],
            (3, 0),
            0,
        );
        ctx.evict(10);
        let SearchResults::Score(tiles) = &ctx.results;
        assert_eq!(tiles.len(), 3);
        // Tile 0 (2 tiles behind the cache radius of the selection) was
        // dropped, so the selection's own index shifts down by one to keep
        // pointing at the same tile.
        assert_eq!(ctx.selection, Selection { tile: 2, item: 0 });
    }

    #[test]
    fn evict_keeps_more_tiles_buffered_for_a_tall_viewport() {
        // tiles_needed_for(200) == ceil(200/32) + 1 == 8, so none of these 5
        // tiles are far enough from the selection to be evicted.
        let mut ctx = context(
            [
                vec![score_result(1, "a")],
                vec![score_result(2, "b")],
                vec![score_result(3, "c")],
                vec![score_result(4, "d")],
                vec![score_result(5, "e")],
            ],
            (0, 0),
            0,
        );
        ctx.evict(200);
        let SearchResults::Score(tiles) = &ctx.results;
        assert_eq!(
            tiles.len(),
            5,
            "a tall viewport must not evict everything down to the old fixed radius"
        );
    }

    #[test]
    fn tiles_needed_for_scales_with_viewport_height() {
        assert_eq!(
            tiles_needed_for(0),
            TILE_CACHE_MARGIN,
            "always at least the lookahead margin"
        );
        assert_eq!(
            tiles_needed_for(32),
            1 + TILE_CACHE_MARGIN,
            "exactly one tile's worth, plus margin"
        );
        assert_eq!(
            tiles_needed_for(33),
            2 + TILE_CACHE_MARGIN,
            "one row over a tile needs a second tile"
        );
        assert_eq!(
            tiles_needed_for(200),
            7 + TILE_CACHE_MARGIN,
            "a tall viewport needs proportionally more tiles"
        );
    }

    #[test]
    fn ensure_buffered_requests_a_tile_ahead_when_short() {
        let mut ctx = context([vec![score_result(1, "a")]], (0, 0), 0);
        let command = ctx.ensure_buffered(7, 10);
        let SearchResults::Score(tiles) = &ctx.results;
        assert_eq!(tiles.len(), 2);
        assert!(matches!(tiles[1], TileState::Loading { .. }));
        assert_eq!(
            command,
            Some(Effect::FetchScoreTile {
                generation: 7,
                query: atom_query("test"),
                pagination: Pagination {
                    offset: 1,
                    limit: TILE_SIZE,
                },
            })
        );
    }

    #[test]
    fn ensure_buffered_does_not_refetch_past_an_empty_trailing_tile() {
        let mut ctx = context([vec![score_result(1, "a")], vec![]], (0, 0), 0);
        assert_eq!(ctx.ensure_buffered(0, 10), None);
        let SearchResults::Score(tiles) = &ctx.results;
        assert_eq!(tiles.len(), 2);
    }

    #[test]
    fn ensure_buffered_requests_backward_and_shifts_the_selection_index() {
        let mut ctx = context([vec![score_result(1, "a")]], (0, 0), 0);
        // Move the sole tile's offset so there's something before it.
        let SearchResults::Score(tiles) = &mut ctx.results;
        tiles[0] = TileState::Loaded(Tile {
            pagination: Pagination {
                offset: TILE_SIZE,
                limit: TILE_SIZE,
            },
            results: vec![score_result(1, "a")].into_boxed_slice(),
        });

        let command = ctx.ensure_buffered(3, 10);
        let SearchResults::Score(tiles) = &ctx.results;
        assert_eq!(tiles.len(), 2);
        assert!(matches!(tiles[0], TileState::Loading { .. }));
        // The selection now sits one slot later since a tile was inserted
        // in front of it.
        assert_eq!(ctx.selection, Selection { tile: 1, item: 0 });
        assert_eq!(
            command,
            Some(Effect::FetchScoreTile {
                generation: 3,
                query: atom_query("test"),
                pagination: Pagination {
                    offset: 0,
                    limit: TILE_SIZE,
                },
            })
        );
    }

    #[test]
    fn on_tile_fetched_replaces_the_matching_loading_slot() {
        let mut ctx = context([vec![score_result(1, "a")]], (0, 0), 0);
        let SearchResults::Score(tiles) = &mut ctx.results;
        let pagination = Pagination {
            offset: 1,
            limit: TILE_SIZE,
        };
        tiles.push_back(TileState::Loading { pagination });

        ctx.handle_tile_fetch(0, pagination, vec![score_result(2, "b")], 10);
        let SearchResults::Score(tiles) = &ctx.results;
        assert_eq!(tiles[1].loaded().map(|tile| tile.results.len()), Some(1));
    }

    #[test]
    fn on_tile_fetched_is_a_no_op_if_the_slot_was_already_evicted() {
        let mut ctx = context([vec![score_result(1, "a")]], (0, 0), 0);
        let evicted_pagination = Pagination {
            offset: 99,
            limit: TILE_SIZE,
        };
        // No matching `Loading` slot exists for `evicted_pagination` - this
        // must not panic, and must leave the original tile untouched (a
        // `reconcile` pass afterward may still append a fresh `Loading`
        // placeholder of its own - that's `ensure_buffered`'s job, covered
        // separately, not what this test is about).
        //
        // A full-size result set, so `is_tile_stale` can't intercept this
        // fetch first - this test is specifically about the "independently
        // evicted" no-op path in `handle_tile_fetch` itself, not about
        // staleness detection (covered separately by the `is_tile_stale_*`
        // tests below).
        ctx.handle_tile_fetch(0, evicted_pagination, full_tile_results(), 10);
        let SearchResults::Score(tiles) = &ctx.results;
        assert_eq!(
            tiles[0].loaded().map(|tile| tile.results.len()),
            Some(1),
            "the original tile must be untouched"
        );
        assert!(
            !tiles
                .iter()
                .any(|state| state.pagination() == evicted_pagination),
            "no tile should ever have been created for the evicted pagination"
        );
    }

    #[test]
    fn handle_tile_fetch_returns_stale_without_applying_the_tile() {
        let mut ctx = context([vec![score_result(1, "a")]], (0, 0), 0);
        let mismatched_pagination = Pagination {
            offset: 99,
            limit: TILE_SIZE,
        };
        // Fewer than `TILE_SIZE` results for a pagination that doesn't match
        // the deque's back tile - `is_tile_stale`'s own definition of stale.
        // `handle_tile_fetch` is expected to check this itself (see its
        // docs) rather than relying on the caller to have checked first.
        let outcome =
            ctx.handle_tile_fetch(0, mismatched_pagination, vec![score_result(9, "x")], 10);
        assert!(matches!(outcome, TileFetchOutcome::Stale));
        let SearchResults::Score(tiles) = &ctx.results;
        assert_eq!(
            tiles[0].loaded().map(|tile| tile.results.len()),
            Some(1),
            "a stale fetch must not touch the existing cache"
        );
    }

    #[test]
    fn is_tile_stale_true_for_an_undersized_non_tail_tile() {
        let ctx = context(
            [vec![score_result(1, "a")], vec![score_result(2, "b")]],
            (0, 0),
            0,
        );
        let SearchResults::Score(tiles) = &ctx.results;
        let front_pagination = tiles[0].pagination();
        let tile_size = usize::try_from(TILE_SIZE).expect("TILE_SIZE fits usize");

        assert!(ctx.is_tile_stale(front_pagination, tile_size - 1));
    }

    #[test]
    fn is_tile_stale_false_for_an_undersized_tail_tile() {
        let ctx = context(
            [vec![score_result(1, "a")], vec![score_result(2, "b")]],
            (0, 0),
            0,
        );
        let SearchResults::Score(tiles) = &ctx.results;
        let back_pagination = tiles
            .back()
            .expect("non-empty by construction")
            .pagination();
        let tile_size = usize::try_from(TILE_SIZE).expect("TILE_SIZE fits usize");

        assert!(!ctx.is_tile_stale(back_pagination, tile_size - 1));
    }

    #[test]
    fn is_tile_stale_false_for_a_full_size_tile() {
        let ctx = context(
            [vec![score_result(1, "a")], vec![score_result(2, "b")]],
            (0, 0),
            0,
        );
        let SearchResults::Score(tiles) = &ctx.results;
        let front_pagination = tiles[0].pagination();
        let tile_size = usize::try_from(TILE_SIZE).expect("TILE_SIZE fits usize");

        assert!(!ctx.is_tile_stale(front_pagination, tile_size));
    }

    #[test]
    fn bootstrap_builds_a_single_loaded_tile_and_prefetches_if_the_viewport_needs_more() {
        let results = vec![score_result(1, "a"), score_result(2, "b")];

        let (ctx, effect) = SearchContext::bootstrap(atom_query("test"), 5, results, 10)
            .expect("non-empty results should bootstrap");

        assert_eq!(ctx.selection, Selection { tile: 0, item: 0 });
        assert_eq!(ctx.selection_position, 0);
        let SearchResults::Score(tiles) = &ctx.results;
        assert_eq!(
            tiles[0].loaded().map(|tile| tile.results.len()),
            Some(2),
            "the bootstrap tile itself should hold the results it was built from"
        );
        // `tiles_needed_for(10)` is 2, so a single bootstrap tile isn't
        // enough - `reconcile` (called internally) should already have
        // pushed a second, `Loading` tile and issued its fetch, same as
        // every other mutating method here does.
        assert_eq!(tiles.len(), 2);
        assert!(matches!(tiles[1], TileState::Loading { .. }));
        assert!(
            effect.is_some(),
            "bootstrap should prefetch ahead when one tile doesn't cover the viewport"
        );
    }

    #[test]
    fn bootstrap_returns_none_for_an_empty_first_page() {
        assert!(SearchContext::bootstrap(atom_query("test"), 5, Vec::new(), 10).is_none());
    }

    #[tokio::test]
    async fn fetch_tile_hydrates_people_and_respects_pagination() {
        let catalogue = Catalogue::open(":memory:")
            .await
            .expect("in-memory catalogue should open");

        let first = catalogue
            .create_score(Title::from_str("A First Score").expect("valid title"))
            .await
            .expect("create_score should succeed");
        let second = catalogue
            .create_score(Title::from_str("A Second Score").expect("valid title"))
            .await
            .expect("create_score should succeed");

        let person = catalogue
            .create_person(&[PersonName::parse("Ada").expect("valid name")])
            .await
            .expect("create_person should succeed");
        catalogue
            .attach_person(first, person)
            .await
            .expect("attach_person should succeed");

        let query = atom_query("Score");

        let page = fetch_tile(
            &catalogue,
            &query,
            Pagination {
                offset: 0,
                limit: 1,
            },
        )
        .await
        .expect("fetch_tile should succeed");

        assert_eq!(page.len(), 1, "limit 1 should return exactly one result");
        assert_eq!(page[0].summary.id, first);
        assert_eq!(page[0].people, vec!["Ada".to_string()]);

        let second_page = fetch_tile(
            &catalogue,
            &query,
            Pagination {
                offset: 1,
                limit: 1,
            },
        )
        .await
        .expect("fetch_tile should succeed");

        assert_eq!(second_page.len(), 1);
        assert_eq!(second_page[0].summary.id, second);
        assert!(
            second_page[0].people.is_empty(),
            "the second score has no people attached"
        );
    }
}
