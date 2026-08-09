# Roles: a case-fold uniqueness key, like people - not tags

**Status:** Accepted

## Context

Each person attached to a score can now carry any number of roles
(composer, arranger, ...), stored in a `roles` lookup table plus a
`score_person_roles` junction (`src/catalogue/migration/m0003_roles.sql`).
Two questions needed deciding: how are role names deduplicated, and how
does a role attach to a specific person on a specific score.

The obvious template was `tags` (0007): dedupe by `name_normalized`, which
strips diacritics as well as case. But role names are closer to
free-typed human labels than to `tags`' short controlled vocabulary -
diacritics can be a deliberate spelling choice, not noise, and folding them
away at insert time would silently merge entries a user meant to keep
apart.

## Decision

- `roles.name_key` uses `normalize::case_fold` - case-insensitive, but
  diacritic-preserving - the same function and reasoning as
  `people.display_name_key` (0006), not `tags.name_normalized`.
  "Composer"/"COMPOSER" collide; "compóser" doesn't.
- No diacritic-stripped `name_normalized` column yet: nothing reads it
  (autocomplete is future work). When it's needed, add it the way
  `person_words` was added for people - a write-time search index, not a
  column that duplicates what search would derive anyway.
- `score_person_roles` has a **composite foreign key**
  `(score_id, person_id) REFERENCES score_people` - a role can only attach
  to a person already on the score, and detaching/deleting that person (or
  the score) cascades role attachments away automatically. This is the
  first multi-column FK in the schema; `m0003`'s migration tests confirm
  `turso` 0.6.1 enforces it and cascades correctly, both directly and
  two-hop through `scores`/`people`.
- Renaming a role (`Catalogue::rename_role`) is a single `UPDATE` on
  `roles.name`/`name_key`, since the name lives in exactly one row.
  Merging two roles that turn out to be duplicates is not implemented -
  only in-place rename is, for now.

## Why

Case is data-entry noise; diacritics can be part of a name's real spelling.
0006 already drew this line for people - roles draw it the same way,
because the same failure mode applies: an author typing a role with
diacritics shouldn't have it silently treated as identical to a
diacritic-free variant they didn't type.

## Sources

- `src/catalogue/normalize.rs` - `case_fold`.
- `src/catalogue/migration/m0003_roles.sql` - schema and its own comments.
- `src/catalogue/migration/m0003_roles.rs` - composite-FK/cascade tests.
- `src/catalogue/role.rs` - `Catalogue::create_role`/`rename_role`.
- `docs/decisions/0006-person-uniqueness-key.md` - the precedent this
  reuses.
