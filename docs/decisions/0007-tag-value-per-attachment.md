# Tags: value lives on the attachment, not the tag, and a score may hold several values of the same tag

**Status:** Accepted

## Context

`docs/decisions/0005-hand-rolled-search.md` sketched `Tag` atoms as prefix
lookups matching "tag names/values by prefix against a `tags` table",
deferring the concrete schema. The first implementation of that schema put
both `name` and `value` on the `tags` entity itself
(`tags(id, name, name_normalized, value, value_normalized)`), deduped on
the `(name, value)` pair, with `score_tags(score_id, tag_id)` as a plain
many-to-many junction uniquely keyed on the pair.

That design surfaced a real gap during review: some tags are genuinely
event-like, where the *value* identifies which instance of a repeating event a
score belongs to (e.g. `#laulupidu` - an Estonian song festival - tagged with
the year it took place, `#laulupidu:2020`). A single score can legitimately
need more than one such tag at once (a piece performed at both the 2020 and
2025 festivals). Under `(score_id, tag_id)` uniqueness, this was structurally
impossible: `tag_id` for `laulupidu:2020` and `laulupidu:2025` were two
*different* rows in `tags` (since name+value was the tag's identity), so
attaching both to one score required two separate `tag_id`s that happened to
share a name by coincidence - the schema had no concept of "the same tag, two
different values, same score."

A secondary question came up while fixing this: should a tag attached *without*
a value be allowed to coexist with the same tag attached *with* one, for the
same score? No concrete use case was found for that coexisting - some tags
simply never carry a value, which is a legitimate permanent state, but once
a specific value has been recorded for a `(score, tag)` pair, an additional
valueless entry for that same pair doesn't add information.

## Decision

- `tags(id, name, name_normalized)` - a tag is just a key, deduped by name
  only (`tags_name_normalized_idx UNIQUE (name_normalized)` -
  `src/catalogue/migration/m0002_people_and_tags.sql:44-50`).
- `score_tags(score_id, tag_id, value, value_normalized)` - the value is a
  per-attachment fact, not part of the tag's identity
  (`src/catalogue/migration/m0002_people_and_tags.sql:52-61`). Uniqueness
  moved from `(score_id, tag_id)` to the full
  `(score_id, tag_id, COALESCE(value_normalized, ''))` triple
  (`score_tags_score_tag_value_idx`,
  `src/catalogue/migration/m0002_people_and_tags.sql:63-73`) - the same
  `COALESCE`-collapses-`NULL` technique the old `tags` uniqueness index
  used, now serving a different purpose: instead of preventing duplicate
  `(name, value)` tag definitions, it lets a score hold many distinct
  values of the same tag while still preventing the exact same value being
  attached twice, and still capping valueless attachments at one per
  `(score, tag)`.
- Mutation API follows the value onto the attachment:
  `create_tag(name)` takes no value; `attach_tag(score_id, tag_id, value)`
  supplies it at attach time; `detach_tag(score_id, tag_id, value)` now
  needs `value` to identify which specific attachment to remove, since
  `(score_id, tag_id)` alone no longer identifies a unique row
  (`src/catalogue/tag.rs:189-245,261-286`).
- **Valueless and valued attachments are mutually exclusive** for a given
  `(score, tag)` pair - enforced procedurally in `attach_tag`, not by the
  schema alone (the schema would happily allow a `NULL` row and several
  valued rows to coexist): attaching *with* a value when a valueless
  attachment exists implicitly replaces it; attaching *without* a value
  when any valued attachment exists is rejected with
  `AttachTagError::ValueRequired` (`src/catalogue/tag.rs:205-223`).
- `eval.rs`'s `compile_tag` needed **no changes** for any of this - the
  join between `tags` (name) and `score_tags` (value) already correlates a
  matched name with one specific attachment's value on the same joined
  row, and `.distinct()` on the outer `score_id` already collapses a score
  matching through several values into one search result.

## Why

- **A tag's value describes a specific use, not the tag itself.** Putting
  `value` on `tags` made "laulupidu:2020" and "laulupidu:2025" unrelated
  entities that happened to share a name, rather than two facts about
  (possibly) the same score under one shared "laulupidu" concept. Moving
  `value` to the junction table is what makes "the same tag, several
  values, one score" representable at all.
- **Multi-value uniqueness reuses a technique already proven for a
  different purpose.** The `COALESCE(value_normalized, '')` trick was
  first added to prevent `tags` itself from admitting unlimited duplicate
  valueless rows per name (SQLite treats every `NULL` as distinct in a
  unique index by default). The exact same trick, applied to
  `score_tags`'s new `(score_id, tag_id, value)` key, solves the analogous
  problem one level down: at most one valueless *attachment* per
  `(score, tag)`, with distinct real values otherwise unconstrained in
  count.
- **Valueless-vs-valued exclusivity is a data-integrity judgment call, not
  a technical requirement.** No use case justified letting both exist
  simultaneously for the same `(score, tag)` pair - a lingering "no
  specific value" entry alongside a real value doesn't convey anything a
  reader could act on. Making `attach_tag` supersede rather than
  coexist keeps `score_tags` from accumulating a class of row with no
  well-defined meaning, at the cost of `attach_tag` doing a delete as part
  of what looks like a pure insert - considered acceptable since it's
  documented on the method and covered by
  `attach_tag_with_value_replaces_a_prior_valueless_attachment` and
  `attach_tag_without_value_rejects_when_a_valued_attachment_exists`
  (`src/catalogue/tag.rs`).
- **The query layer didn't need to know about any of this.** Multi-value
  support and the mutual-exclusion rule are both expressible entirely as
  constraints/procedures in the mutation layer; `compile_tag`'s join
  shape was already exactly what both needs required, which is why this
  decision has zero footprint in `src/catalogue/search/eval.rs`.

## Sources

- `src/catalogue/migration/m0002_people_and_tags.sql` - `tags`/
  `score_tags` schema and the indexes' own comments explaining each one's
  purpose.
- `src/catalogue/tag.rs` - `Catalogue::attach_tag`/`detach_tag`, the
  `score_tag_row_exists`/`score_tag_has_any_value`/`delete_score_tag_row`
  helpers implementing the mutual-exclusion rule, and the tests exercising
  multi-value and replace/reject behavior.
- `src/catalogue/search/eval.rs` - `compile_tag`, unaffected by this
  decision.
- `docs/decisions/0005-hand-rolled-search.md` - the name+value-on-`tags`
  sketch this decision revises.
