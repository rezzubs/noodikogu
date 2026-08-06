# People: a separate case-fold key for uniqueness, distinct from search normalization

**Status:** Accepted

## Context

`docs/decisions/0005-hand-rolled-search.md` sketched `Person` atoms as
prefix lookups against a `person_names` table, deferring the concrete
schema to the change that builds it. That change also needed to decide:
should two people ever be allowed to share a full name?

The answer settled on was no - two catalogue entries with the exact same
full name is a confusing UX (search can't tell them apart), so
`Catalogue::create_person` rejects a duplicate full name, returning the
existing person's id instead of creating a second row
(`src/catalogue/person.rs:87-95`).

The catalogue already has a text-normalization function for search,
`normalize::normalize_text` (`src/catalogue/normalize.rs:11-28`): Unicode
NFD decomposition, stripping combining diacritic marks, NFC recomposition,
then lowercasing - i.e. case- *and* diacritic-insensitive. The first
version of person uniqueness reused this same function for both jobs
(driving `person_words` tokenization for search, and the uniqueness
check), on the reasoning that it already existed and titles use one
normalized column for everything.

This broke on real data: Estonian names commonly differ only by
diacritics - "Marten" and "Märten" are both legitimate, distinct given
names. Under `normalize_text`, both collapse to `"marten"`, so creating a
person named "Märten Roots" after "Marten Roots" already existed would
have been rejected as a duplicate, even though they're different people.

## Decision

Two separate normalized forms of `display_name`, used for two different
purposes:

- `display_name_key` (`normalize::case_fold` -
  `src/catalogue/normalize.rs:37-45`): Unicode NFC canonicalization plus
  Unicode-aware lowercasing, **without** the diacritic-stripping step.
  Drives the uniqueness constraint
  (`people_display_name_key_idx UNIQUE (display_name_key)` -
  `src/catalogue/migration/m0002_people_and_tags.sql:7-19`) and the
  pre-check in `Catalogue::create_person`
  (`src/catalogue/person.rs:117-122`). Case-insensitive, diacritic-sensitive.
- A `normalize_text`-normalized form, computed at write time in
  `create_person` and tokenized into `person_words`
  (`src/catalogue/person.rs:116,132`) for prefix search. Diacritic-insensitive
  by design - a search for "Marten" is meant to surface "Märten Roots" too.
  Not persisted on `people` itself: nothing ever reads it back from that
  table.

One consequence of anchoring identity on a single `display_name` string
(joined from the ordered `Vec<PersonName>` components with spaces) rather
than a `person_names` table with one row per component, as ADR 0005's
`person_names` sketch originally implied: `person_words` (`person_id`,
`word`, composite PK) ends up structurally identical to `title_words` -
same tokenize-and-index shape, same self-join pattern in
`compile_person` as `compile_title` uses. Component order only matters for
display (preserved in `display_name`'s raw text), not for search, matching
how title word order already didn't matter for title search.

## Why

- **Case and diacritics are different kinds of variation for a name.**
  Casing differences ("JOHANN BACH" vs "Johann Bach") are data-entry noise
  - the same person, entered differently. Diacritics are not - they're
  part of the name's actual spelling and can distinguish two real people.
  Treating both the same way (as `normalize_text` does) is correct for
  *search* (a diacritic-insensitive query is a reasonable convenience) but
  wrong for *uniqueness* (it would silently merge two distinct people).
- **The diacritic-stripped form doesn't need to be a column on `people`.**
  It's derived from `display_name`, used once at write time to populate
  `person_words`, and never queried back from `people` afterward - unlike
  `titles.value_normalized`, which earns its persisted column via
  `search.rs`'s `hydrate()` sorting `ORDER BY titles.value_normalized`.
  `people` has no equivalent sort/lookup need, so persisting it there would
  have been dead weight copied from the `titles` pattern without checking
  whether the same justification applied.

## Sources

- `src/catalogue/normalize.rs` - `normalize_text`/`case_fold` definitions
  and their doc comments.
- `src/catalogue/migration/m0002_people_and_tags.sql` - `people`/
  `person_words` schema and the uniqueness index's own comment.
- `src/catalogue/person.rs` - `Catalogue::create_person`,
  `person_lookup_by_display_name_key`.
- `docs/decisions/0005-hand-rolled-search.md` - the `person_names`-per-component
  sketch this decision revises into `display_name` + `person_words`.
