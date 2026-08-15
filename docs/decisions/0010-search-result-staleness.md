# 0010. Search-result staleness under concurrent catalogue writes

**Status:** Accepted

## Context

`SearchContext` (`src/tui/model/context/search.rs`) caches a query's results
in fixed-size tiles, fetched in the background as the user scrolls. Nothing
about this cache is invalidated if the underlying data changes while it's
open. Two distinct problems follow:

1. **Stale/undersized tiles.** Tiles are fetched by independent async calls,
   each seeing whatever the database looks like when it happens to run. If
   matching rows are removed between two such fetches, an already-loaded tile
   can keep implying more data sits past it than the query still has: a tile at
   offset 32 was genuinely full when it loaded, but a *later* fetch at offset
   0 (triggered by scrolling toward the top) sees the post-delete state and
   can come back with fewer than `TILE_SIZE` rows - because the query's total
   match count has since dropped below 32.
2. **Reordering causes duplicate or invisible rows.** `Catalogue::search`'s
   `ORDER BY` (`src/catalogue/search.rs`, `hydrate()`) sorts by a title-match
   ranking, then normalized title - both mutable, and with no tiebreaker.
   Editing a score's title (or anything the ranking reads) can change its sort
   position after some tiles were already cached at their old positions; a score
   can then appear twice (old and new tile both claim it) or not at all (it
   moved out of every cached tile's range).

Both problems require a *write from outside the current search's own fetches*
\- normal solo browsing never triggers them. Today that mostly means concurrent
action within the same TUI session (e.g. deleting a score while a background
tile for the same query is still in flight), but the design explicitly
anticipates a future web server as a separate process with its own connections
to the same database file (see `docs/tui.md`'s Concurrency section). Any fix has
to work across process boundaries, not just within one `Catalogue` instance.

### Ruled out: an in-process counter

An `Arc<AtomicU64>` on `Catalogue`, bumped by every mutating method, was the
first idea. It's useless for the actual threat model: it only observes
writes made through that specific in-memory counter, not writes from a
different process (or even a different `Catalogue::open` call in the same
process). Given the web server is explicitly planned, this doesn't solve the
problem it needs to solve.

### Ruled out: a free database-native signal

SQLite has `PRAGMA data_version` for exactly this ("did the file change
under me") use case. turso doesn't implement it -
`turso_core-0.6.1/pragma.rs`'s `pragma_for` has no `DataVersion` arm.
`total_changes()` exists as a SQL function, but it's scoped per-connection,
and `Catalogue::connect()` (`src/catalogue.rs`) deliberately opens a fresh
connection for every single method call - so it can't accumulate writes
across calls without a larger change to how `Catalogue` holds connections.

## Decision

The complete fix, planned but **not implemented yet**: a database-level,
trigger-maintained generation counter.

- A single-row table (e.g. `search_generation`) holding one integer.
- `AFTER INSERT/UPDATE/DELETE` triggers, one set per table, on every table
  that can change which scores a search returns or their order. As of this
  writing (see `src/catalogue/search/eval.rs` and `search/rank.rs`), that's:
  `scores`, `titles`, `title_words` (title matching + ranking), `person_words`
  \+ `score_people` (person-atom matching), `tags` + `score_tags` (tag-atom
  matching). Notably **not** `roles` - no `SearchAtom` variant reads it, so it
  can never affect a search's row set.
- **Invariant to maintain going forward: every table reachable from
  `eval::compile_score_query` or `TitleRanking` must have a matching
  trigger.** Adding a new `SearchAtom` kind (or changing what the ranking
  reads) means updating this list. This is a schema-level, migration-review
  time obligation, not enforced by the type system - call it out explicitly
  in any PR that touches search's SQL.
- `Catalogue::search` reads the current generation value alongside its
  results, in the same call (avoiding a second connection/round-trip and the
  narrow race that would introduce).
- `SearchContext` records the generation of its own bootstrap fetch. Any
  later fetch response carrying a different generation means the cache may
  be wrong in either of the two ways described above.
- On a mismatch, the entire cached tile set is discarded and the search
  restarts from the top - see the "Interim mitigation" section below for the
  *mechanism* (an `Effect`, not a silent in-place fix), which this full fix
  is meant to reuse and eventually gate behind a real prompt ("results
  changed, refreshed" or similar) instead of resetting silently.

## Interim mitigation (implemented now)

Without the generation counter, one specific symptom is still detectable
with zero new infrastructure: **a non-tail tile resolving with fewer than
`TILE_SIZE` rows.** Under no concurrent mutation, every tile except the true
tail of a result set is always exactly `TILE_SIZE` long (`ensure_buffered`
only ever requests full-size pages ahead of an already-loaded, non-empty
neighbor) - so a shorter one anywhere else is proof something changed
underneath the cache, regardless of *why*.

This catches problem 1 above in its most common shape, and, because a
detected mismatch resets the whole context, self-heals problem 2 as a side
effect within the same window (no permanent duplicate/invisible row,
just a possible brief inconsistency until the reset completes). It does
**not** catch every case - most notably, a *reordering* that doesn't happen
to produce a short tile (e.g. a title edit that moves a row between two
still-full tiles) goes undetected until something else notices. That gap is
accepted, not fixed, until the full generation-counter design above lands.

The reset goes through `Effect::SearchInvalidated` rather than being applied
directly in `Model::update`, specifically so a future prompt/confirmation
step has somewhere to live without changing `Model::update`'s shape again -
see `src/tui.rs`'s handling of it.

## Why

- External writes are the actual threat model (a future web server as a
  separate process), which rules out any purely in-process detection
  mechanism.
- The complete fix is real, understood, and worth building properly but is a big
  change that deserves to be done separately from the core TUI implementation
  that's the current focus. Writing it down now means the invariants and the
  reasoning aren't lost, and the interim mitigation's `Effect`-based shape is
  deliberately compatible with it.

## Sources

- `turso_core-0.6.1/pragma.rs` (`pragma_for` / `PragmaName` match) - no
  `DataVersion` pragma implemented.
- `turso-0.6.1/src/lib.rs` (`ScalarFunc::TotalChanges`) - `total_changes()`
  exists as a SQL function, standard SQLite per-connection semantics assumed
  (not overridden anywhere found in turso's source).
- `src/catalogue.rs` (`Catalogue::connect`) - fresh connection per method
  call, doc comment explicitly requires every method to go through it.
- `src/catalogue/search/eval.rs`, `src/catalogue/search/rank.rs` - tables
  read while compiling/ranking a `ScoreQuery`.
