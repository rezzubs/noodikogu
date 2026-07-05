CREATE TABLE scores (
    id INTEGER PRIMARY KEY,
    description TEXT,
    created_at TEXT NOT NULL
);

CREATE TABLE titles (
    id INTEGER PRIMARY KEY,
    score_id INTEGER NOT NULL REFERENCES scores (id) ON DELETE CASCADE,
    value TEXT NOT NULL,
    value_normalized TEXT NOT NULL,
    is_primary INTEGER NOT NULL DEFAULT 0 CHECK (is_primary IN (0, 1))
);

-- Sole invariant on is_primary: only one title per score can be marked
-- primary at any time.
CREATE UNIQUE INDEX titles_primary_idx ON titles (score_id)
    WHERE is_primary = 1;

CREATE TABLE title_words (
    title_id INTEGER NOT NULL REFERENCES titles (id) ON DELETE CASCADE,
    word TEXT NOT NULL,
    PRIMARY KEY (title_id, word)
);

CREATE INDEX title_words_word_idx ON title_words (word);
