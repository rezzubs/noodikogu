# Hand-rolled search instead of turso's `fts` index

**Status:** Accepted

## Context

The catalogue's search language is already implemented as a parsed AST,
independent of any storage decision (`src/query.rs`, `src/query/parser.rs`):

- `Query` is one of three top-level search modes: `Score(ScoreQuery)`
  (default - boolean search over scores), `Tag(...)` (`##`, tags by name
  prefix), `Person(...)` (`@@`, people by name-component prefix).
- In score mode, `ScoreQuery`/`AndQuery`/`OrQuery`/`NotQuery` compose
  `SearchAtom::{Title(String), Person(Person), Tag(Tag)}` terms with
  AND/OR/NOT and grouping, in a normalized flat form (no nested same-
  operator sequences, no double negation - the parser guarantees this).

Evaluating this AST against the database means: for every atom in it,
produce the set of matching score ids, then combine those sets according
to the AST's boolean structure (intersect for AND, union for OR, set
difference for NOT). That combination step is unavoidably hand-written -
no single index type spans `titles`, person names, and tags at once - and
it has to happen regardless of which mechanism each individual atom kind
uses to find its matches.

Only the `Title` atom has a plausible FTS-shaped answer. `Person` atoms
match name components by prefix against a `person_names` table
(`value LIKE 'prefix%'`) and `Tag` atoms match tag names/values by prefix
against a `tags` table - both are plain structured-identifier lookups with
no FTS-equivalent concept. So the real design question isn't "should search
use FTS" in general, it's specifically: should the `Title` atom's
resolution use FTS5-style tokenized matching, while `Person`/`Tag` atoms
stay plain SQL?

Title matching itself needs word-level prefix matching (a query for "Mar"
should find a title containing "Maria", not just titles starting with
"Mar") and some form of relevance ranking so better matches surface first.
The natural SQLite tool for that is FTS5. Reading turso 0.6.1's source
directly (`turso_core-0.6.1`, `turso-0.6.1`) shows it does not implement
FTS5 at all: no `CREATE VIRTUAL TABLE ... USING fts5(...)`, no
`unicode61`/`porter` tokenizers, no `remove_diacritics`, no
external-content-table concept - attempting any of that fails with "no
such module".

Instead turso has its own, unrelated, Tantivy-backed mechanism:
`CREATE INDEX idx ON table USING fts (col, ...) WITH (tokenizer='...', weights='...')`
(`turso_core-0.6.1/translate/index.rs:50-202`). Notable properties:

- Gated behind the Cargo feature `fts` *and* a runtime opt-in
  (`experimental_index_method(true)`); errors with "experimental feature"
  otherwise (`translate/index.rs:58-63`). Incompatible with MVCC mode.
- Tokenizers: `default`, `raw`, `simple`, `whitespace`, `ngram`
  (`index_method/fts.rs:1341-1347`) - none diacritic-aware.
- Behaves like a real secondary index (auto-maintained on
  insert/update/delete, no triggers needed -
  `vdbe/execute.rs:1097-1107,9610-9760`), queried via `col MATCH 'text'` or
  `fts_match()`/`fts_score()`/`fts_highlight()` (`function.rs:246-274`).
- No dedicated tests exist for it upstream in 0.6.1 - the only usage found
  is one example file and a benchmark.

## Decision

Don't use turso's `fts` index feature for the `Title` atom either. Every
atom kind in the query AST resolves to plain SQL:

- **Title matching**: a `title_words` junction table (`title_id`, `word`),
  populated by tokenizing each title's normalized text at write time, with
  a plain B-tree index on `word`. A query word becomes
  `WHERE word LIKE 'mar%'` - index-backed (no leading wildcard), so it
  scales the same way any ordinary indexed lookup does. Same shape of
  query as the `Person`/`Tag` atoms' prefix lookups.
- **Ranking**: a cheap heuristic - count of distinct query words matched,
  boosted for a whole-title or prefix-of-whole-title match - instead of
  BM25's term-frequency/inverse-document-frequency/length-normalization
  model. Full design deferred to the dedicated change that builds the
  AST-to-SQL evaluator.
- **Boolean composition** across `Title`/`Person`/`Tag` atoms stays exactly
  what it already had to be - hand-rolled SQL (`JOIN`/`INTERSECT`/`UNION`/
  `EXCEPT` over id sets) driven by the existing AST - also deferred to
  that same change.

## Why

- **Every atom kind produces the same shape of subquery.** `Person` and
  `Tag` atoms were always going to be plain prefix lookups (there's no
  FTS-equivalent for matching a structured name/tag identifier). If
  `Title` alone used turso's `fts` index, the AST-to-SQL evaluator would
  need two different execution strategies - `MATCH`/`fts_score()` for one
  atom kind, `WHERE`/`JOIN` for the other two - and still have to merge
  both into the same `INTERSECT`/`UNION`/`EXCEPT` composition the AST
  demands. Keeping `Title` on plain SQL too means the evaluator has one
  uniform shape to generate and compose for every atom, not a special case
  for exactly one of them.
- **Avoids an experimental, untested engine feature.** turso's `fts` isn't
  just unfamiliar - it's feature-flagged as experimental, has no upstream
  test coverage, and is incompatible with MVCC. Depending on it now means
  depending on a code path turso itself doesn't yet stand behind.
- **A hand-rolled word index gets most of the value at this project's
  scale.** BM25's sophistication targets long-document corpora; titles
  here are short strings over a modest corpus (1-2k rows now, ~50k
  ceiling). A simple matched-word-count heuristic is unlikely to be
  noticeably worse in practice.
- **Schema stays forward-compatible either way.** The normalized title text
  this indexes is exactly what turso's `fts` index would need too, so if
  the hand-rolled approach ever proves inadequate for `Title` specifically,
  adding `CREATE INDEX ... USING fts` later is a purely additive
  migration - no restructuring of `scores`/`titles` needed, and no change
  to how `Person`/`Tag` atoms resolve.

## Sources

- `src/query.rs`, `src/query/parser.rs` - the existing search AST and
  parser this decision composes with.
- Local `turso_core-0.6.1`/`turso-0.6.1` crate source
  (`~/.cargo/registry/src/.../`, version 0.6.1 as vendored in this
  project's `Cargo.lock`):
  - `translate/index.rs:50-202,58-63` - `CREATE INDEX ... USING` grammar,
    experimental-feature gate, MVCC incompatibility.
  - `index_method/fts.rs:1341-1347,1884-1896` - supported tokenizer list.
  - `vdbe/execute.rs:1097-1107,9610-9760` - automatic index-maintenance
    dispatch for FTS cursors (no manual sync needed).
  - `function.rs:246-274` - `fts_match`/`fts_score`/`fts_highlight`
    signatures.
