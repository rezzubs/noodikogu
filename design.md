# Terminology

- **Catalogue** — the system users interact with (not "database", to avoid confusion with the backing store)
- **Database** — the internal SQLite/turso backing store, an implementation detail

# Overview

A music catalogue for a choir. Stores metadata about physical sheet music and optionally their digital counterparts (PDFs, audio). The primary problem it solves is making a large collection of scores searchable with powerful, flexible queries.

- Built as a Rust library
- Backed by the `turso` crate (pure Rust SQLite-compatible database)
- Web interface: axum + htmx
- CLI: REPL-style interface for the librarian
- Target scale: 1000–2000 scores currently, designed to remain usable up to ~50k

# Access & Auth

- The web interface is members-only. Authentication is delegated to an existing choir web app (node/react). The library is open source so alternative auth mechanisms must be possible.
- Only the librarian can add or edit scores. Regular members can search and view.
- Public sharing of PDFs via shareable links (optionally password-protected), to handle requests from external conductors.

# Architecture

```
[ turso database ]  [ file system ]
         \               /
      [ Storage trait (turso impl) ]
                 |
          [ Catalogue library ]
          /               \
  [ axum web server ]   [ CLI ]
  [ htmx frontend   ]   [ REPL ]
```

- The library is a full abstraction over the storage layer. The web server and CLI have no knowledge of the database.
- The `Storage` trait allows alternative backend implementations and makes testing with in-memory SQLite straightforward.
- The library is fully async using tokio.
- CLI operates in local mode (library directly). Remote mode (HTTP client to the web server) is a future addition — for now the librarian SSHes into the VPS to use the CLI.
- Self-hosted on a VPS as a single native Rust binary.
- Backups via borg (daily). No undo system — backups are the recovery mechanism.

# Database Schema

```
scores          id, description?, created_at
titles          id, score_id, value, value_normalized, is_primary
people          id
person_names    id, person_id, value, is_abbreviation, position
score_people    score_id, person_id, role, is_primary_author
tags            id, name, name_normalized (unique)
score_tags      id, score_id, tag_id, value?, value_normalized?
file_sources    id, score_id, kind (pdf/audio/...), path, checksum, description?, is_primary
link_sources    id, score_id, url, description?, is_primary
related_scores  score_id_a, score_id_b (a < b enforced), explanation?
titles_fts      FTS5 virtual table over titles.value_normalized

users           id, username, password_hash?, oidc_subject?, oidc_provider?, role (member/librarian), created_at
sessions        id, user_id, token, expires_at, created_at
share_links     id, source_id, token (random, unique), password_hash?, expires_at?, created_at
```

## Notes

- `is_primary` per `(score_id, kind)` in `file_sources` is enforced with a partial unique index: `UNIQUE(score_id, kind) WHERE is_primary = 1`
- A tag on a score is either valued or unvalued — not both simultaneously. This is enforced by the library, not the database.
- `UNIQUE(score_id, tag_id, value)` is enforced at the library level since SQLite treats NULLs as distinct in unique indexes.
- `related_scores` enforces undirected uniqueness by requiring `score_id_a < score_id_b`.
- `created_at` / `updated_at` on other tables deferred to when the edit log feature is implemented.
- Edit log (field-level audit trail) is a v2 feature. Schema: operation, entity id, field, old value, new value, timestamp, user.

# File Storage

- Files are stored in a library-controlled directory on the filesystem. Paths are recorded in the database.
- SHA-256 checksums are stored per file for integrity verification.
- A `heal` command scans the directory, recomputes checksums, and reports or fixes discrepancies (missing files, unexpected files, corrupted files).
- Files are NOT stored as BLOBs in the database.

# People

- People are global entities linked to scores via `score_people` with a role string.
- One person per score is flagged as `is_primary_author` regardless of their role.
- Names are split into components by whitespace and `-`. Each component is stored separately with an `is_abbreviation` flag and an ordering position.
  - Example: `V. Soosalu` → `[{value: "V", is_abbreviation: true}, {value: "Soosalu", is_abbreviation: false}]`
- A person with three name components can be found by any combination of their names in any order.

# Tags

- Tag names are stored in a separate `tags` table (one row per unique tag name).
- Tag names and values support alphanumeric unicode characters, `_`, and `-`.
- Tag names and values are normalized for searching (unicode case-folding, NFC normalization, diacritic stripping for Latin scripts).
- A score can have the same tag multiple times with different values but not the same `(tag, value)` pair twice.
- Tags replace dedicated fields for things like collection ID, musical key, time signature, instrumentation, etc.

# Search

## Search Modes

The top-level AST node encodes the search mode. Only one mode is active per query.

- **Score mode** (default): results are scores. All boolean syntax applies.
- **People mode** (`@@`): results are people. `@@Vettik` finds people with a name component starting with "Vettik". No other search terms are allowed.
- **Tag mode** (`##`): results are tags. `##difficulty` finds tags with names starting with "difficulty". No other search terms are allowed.
- `@@` or `##` alone (no prefix term) returns all people or all tags respectively.
- People and tag modes work the same on both the web interface and CLI.

## Query Syntax

Title terms, person terms, and tag terms can be combined with boolean logic (score mode only):

- Bare words are title search terms: `Pseudo Yoik` (consecutive bare words are passed as one FTS5 query)
- `@Name1.Name2` matches people by name component prefixes
- `#tag_name` or `#tag_name:value` matches tags
- Space between terms is implicit AND
- `|` is OR, `!` is NOT, `()` for grouping
- Precedence: `NOT > AND > OR`
- Quoted strings (e.g. `"#literal"`) bypass special character interpretation and search as title text

### Tag boolean syntax
- `#tag_name:(value1 value2)` — score must have the tag with both values
- `#tag_name:(value1 | value2)` — score must have the tag with either value
- `#tag_name:!value` — score must have the tag but NOT with this value (other values are fine)
- `!#tag_name:value` — exclude scores that have this specific tag+value pair (other values for the tag are still fine)

## Ranking

- Title matching uses FTS5 with BM25 ranking (built-in).
- FTS5 tokenizes title text into words, enabling word-level prefix search efficiently.
- Text is normalized (lowercased, diacritics stripped for Latin, NFC) before indexing and querying.
- Boolean logic across entity types (titles, people, tags) is handled at the SQL level via `INTERSECT` / `UNION` / `EXCEPT` on score ID sets. FTS5 is used only for per-term matching and scoring.
- Default sort for equal-ranked results: lexicographic by primary title.

## Completion

- Completion is driven by `(query_string, cursor_position)`.
- The parser returns a cursor context node describing what the cursor is inside (tag name, tag value for a specific tag, person name component, title word).
- The completion engine queries the database for prefix matches based on that context.

# Library API

```rust
// Core types
struct Pagination { limit: usize, offset: usize }
struct ScoreSummary  { id, primary_title, primary_author, available_source_kinds }
struct PersonSummary { id, name, score_count }
struct TagSummary    { id, name, score_count, unique_value_count }
struct ScoreDetail   { /* all fields */ }

// The search mode is encoded in the QueryAst top-level node;
// the library dispatches internally and the caller matches on the result.
enum SearchResult {
    Scores(Vec<ScoreSummary>),
    People(Vec<PersonSummary>),
    Tags(Vec<TagSummary>),
}

// Main methods
async fn search(query: QueryAst, pagination: Pagination) -> Result<SearchResult>
async fn browse(letter: Option<char>, pagination: Pagination) -> Result<Vec<ScoreSummary>>
async fn get_score(id: ScoreId) -> Result<ScoreDetail>
```

- The parser is exposed separately as a standalone module. It takes `(query: &str, cursor: usize)` and returns both a `QueryAst` and a `CursorContext` for completions. 
- A convenience method converts a raw query string to a `QueryAst`.
- `browse` is alphabetically sorted; `letter` filters to scores whose primary title starts with that letter.
- Both `search` and `browse` support infinite scroll via `Pagination`.

# Auth

Three auth mechanisms, all producing the same session cookie upon success:

1. **Native** — username + password, argon2 hashing. Librarian manages accounts. Works out of the box for any self-hoster.
2. **OIDC** — configurable via config (discovery URL, client ID, client secret). Supports Google, the choir's existing service (if it adds OIDC), or any other provider.

The choir's existing auth service should implement OIDC to allow members to log in with existing accounts — making the integration reusable for any choir without custom code in this project.

# Web Interface

## UI

- Main view: search bar + score list (infinite scroll). Search updates results as you type.
- Browse mode (empty search): scores filtered by starting letter via a letter bar, infinite scroll.
- Score cards show: primary title, primary author, icons for available source kinds.
- Score detail page: full metadata (description, all titles, contributors with roles, tags, related scores, sources).
- Primary action: access the PDF.

## Routes

All routes except `/share/` and auth routes require a valid session. Responses are HTML (htmx fragments where applicable).

```
GET  /                          main page (search bar + initial browse results)
GET  /search?q=<query>&offset=  htmx fragment: search or browse results
GET  /scores/:id                score detail page
GET  /scores/:id/sources/:id    serve file (PDF/audio) — streams from controlled directory

GET  /share/:token              public share page or file (no auth required)
POST /share/:token/verify       verify password for password-protected share links

GET  /login                     login page (native + OIDC options)
POST /login                     native login (username + password)
GET  /login/oidc/:provider      initiate OIDC flow
GET  /login/oidc/:provider/cb   OIDC callback
POST /logout                    clear session
```

## Notes

- No JSON endpoints — the web server returns HTML only. A future remote CLI mode would add a separate API layer at that time.
- File serving streams directly from the controlled directory with auth checked in the handler.
- Share link handler checks password (if set) and expiry before serving.

# CLI

Built with ratatui (or a lighter alternative — to be decided at implementation time).

## Search & Navigation

- Live search as you type, same syntax as the web interface including `@@` and `##` modes.
- Completion candidate shown as dim text at the end of the query; confirmed with a keybind.
- Enter confirms the search and enables result selection.
- Results are displayed in an aligned table format:
  - **Scores**: `#  Title  Author  Sources`
  - **People**: `#  Name  Scores`
  - **Tags**: `#  Tag  Scores  Unique values`
- Selection via arrow keys, emacs-style controls, or number shortcuts.
- Pagination determined by available terminal height with a configurable maximum. Prev/next page appear as selectable entries.
- Selecting an entry navigates "inside" it (score, person, or tag context). Back navigation restores the previous search results and position.

## Contexts

Navigation has three levels:

```
Top level     commands only — no ambiguity with search queries
    ↓  search [query]
Search mode   live results, Escape returns to top level
    ↓  select entry
Entity        score/person/tag commands, back returns to search results
```

At the **top level** (root command prompt):
- `search [query]` — enter search mode, optionally pre-populated with a query
- `add title [title]` — create a new score with a primary title and enter it immediately. Will prompt for the name if not given
- `add person [name]`, `add tag [name]` — create standalone people or tags. 

In **search mode**:
- Live results update as you type (same syntax as web, including `@@`/`##` modes)
- Escape returns to top level
- Enter / arrow keys / number shortcuts to select an entry and enter its context

Inside a **score context** (prompt shows score title):
- `add title [<value>]` — add an alternate title; prompted if not given
- `add person [<name> [<role>]]` — attach a person (creates if not found, showing similar existing people as selectable suggestions); leading `@` stripped and optional; prompted if not given
- `add tag [<name>[:<value>]]` — attach a tag (creates if not found, showing similar suggestions); leading `#` stripped and optional; prompted if not given
- `add source [<path or url>]` — add a source; file type inferred from extension; prompted if not given
- `add related` — enter search mode to find and link a related score
- `set description [<value>]` — set description inline; prompted if not given
- `set primary title` — prompts with current titles to select the primary
- `set primary author` — prompts with currently attached people
- `set primary source` — prompts with currently attached sources, grouped by kind
- `remove <attribute>` — Remove an existing attribute. Prompts with available choices.
- `edit <attribute>` — open the attribute in `$EDITOR`.
- `delete` — delete the score; asks for confirmation

Inside a **person** or **tag context**: similar `set`/`edit`/`delete` commands for their respective fields.

## Non-interactive mode

Simple operations can be run without entering the REPL via flags:

```
noodikogu add --title "Score name" --description "..."
```

Full editing workflows (multi-field, interactive) are REPL-only.

# Out of Scope for v1

- Remote CLI (HTTP client mode) — SSH into VPS for now
- Web management UI — CLI only
- Edit log / undo system
- Stop words (and, or, ...) — adding them after the fact requires re-indexing all FTS5 entries and making the stop word list configurable; deferred until there is a concrete need
- i18n
