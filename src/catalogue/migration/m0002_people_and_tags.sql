CREATE TABLE people (
    id INTEGER PRIMARY KEY,
    display_name TEXT NOT NULL,
    display_name_key TEXT NOT NULL
);

-- No two people may have the same full name, case-insensitively - avoids a
-- confusing UX where search can't tell two catalogue entries apart. If two
-- real people share a name, the catalogue's maintainer adds a
-- distinguishing name component.
--
-- Deliberately keyed on `display_name_key` (case-folded, diacritics
-- preserved), not a diacritic-stripped normalization - "Marten Roots" and
-- "Märten Roots" are different people and must both be insertable, even
-- though a search for "Marten" is intended to find both (see
-- `person_words` below, whose diacritic-stripped tokenization exists only
-- as ephemeral values derived from `display_name` at write time, never
-- persisted on `people` itself since nothing reads it back from there).
CREATE UNIQUE INDEX people_display_name_key_idx ON people (display_name_key);

-- Search words used to identify a person.
CREATE TABLE person_words (
    person_id INTEGER NOT NULL REFERENCES people (id) ON DELETE CASCADE,
    word TEXT NOT NULL,
    PRIMARY KEY (person_id, word)
);

CREATE INDEX person_words_word_idx ON person_words (word);

CREATE TABLE score_people (
    score_id INTEGER NOT NULL REFERENCES scores (id) ON DELETE CASCADE,
    person_id INTEGER NOT NULL REFERENCES people (id) ON DELETE CASCADE,
    PRIMARY KEY (score_id, person_id)
);

CREATE INDEX score_people_person_id_idx ON score_people (person_id);

-- A tag is just a key (e.g. "difficulty"); its value, if any, is a
-- per-score concept assigned when the tag is attached to a score - see
-- `score_tags` - not part of the tag's own identity. This lets one score
-- carry `difficulty:5` and another `difficulty:easy` while both still
-- refer to the same "difficulty" tag, and lets `#difficulty` alone match
-- any attachment regardless of its value (including none).
CREATE TABLE tags (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    name_normalized TEXT NOT NULL
);

CREATE UNIQUE INDEX tags_name_normalized_idx ON tags (name_normalized);

-- No `id`/composite `PRIMARY KEY` here on purpose: the natural key is
-- `(score_id, tag_id, value)` itself (see the unique index below), and
-- nothing else needs to reference an individual attachment by a surrogate
-- id.
CREATE TABLE score_tags (
    score_id INTEGER NOT NULL REFERENCES scores (id) ON DELETE CASCADE,
    tag_id INTEGER NOT NULL REFERENCES tags (id) ON DELETE CASCADE,
    value TEXT,
    value_normalized TEXT
);

-- A score may carry the same tag with several *different* values at once
-- (e.g. `#laulupidu:2020` and `#laulupidu:2025` on the same score) - so
-- uniqueness is on the full `(score_id, tag_id, value)` triple, not just
-- `(score_id, tag_id)`. `COALESCE` collapses a missing value to '' so
-- SQLite doesn't treat every NULL as distinct and admit more than one
-- valueless row per `(score_id, tag_id)` - mirrors `tags_name_normalized_idx`'s
-- sibling index on `tags` itself. Valueless and valued are mutually
-- exclusive states for a given `(score, tag)` pair - enforced procedurally
-- in `tag.rs`, not by this index alone.
CREATE UNIQUE INDEX score_tags_score_tag_value_idx
    ON score_tags (score_id, tag_id, COALESCE(value_normalized, ''));

-- Composite rather than a bare `tag_id` index: a single tag (e.g.
-- "difficulty") can end up attached to most of the catalogue, so a
-- `#difficulty:5`-style query benefits from filtering by value inside the
-- index itself rather than a residual scan over every attachment of that
-- tag. A bare `#difficulty` query (no value) still uses this index too,
-- via its leftmost `tag_id` column.
CREATE INDEX score_tags_tag_id_value_idx ON score_tags (tag_id, value_normalized);
