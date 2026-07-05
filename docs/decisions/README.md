# Architecture decision records

Short technical decisions for `noodikogu`, numbered sequentially
(`NNNN-title.md`). Written when a decision isn't obvious from the code
alone — usually because it hinges on an external library's behavior, a
tradeoff that was considered and rejected, or something discovered by
reading source/docs rather than visible in a diff.

## Format

- **Status** — `Accepted`, `Superseded by NNNN`, or `Deprecated`.
- **Context** — what question this answers and why it needed deciding.
- **Decision** — what was decided.
- **Why** — the reasoning. Cite sources inline (`file:line`, doc URLs)
  rather than asserting.
- **Sources** — full citations for anything referenced above.

Keep these terse. Section names above are the default skeleton, not a rigid
template: split "Why" into descriptively-named subsections if there's more
than one distinct reasoning thread (see 0001).

An accepted ADR is a historical record, not a living document: it captures
what was decided and why, as understood at the time. If a decision changes
later, write a new ADR and change the *old* one's `Status` to
`Superseded by NNNN` — don't edit the old one's body to match the new
reality. The code will drift from an old ADR's specifics (a renamed
identifier, a refactored snippet) and that's expected; the ADR isn't meant
to be re-synced, it's meant to explain a choice that was made.

## Index

- [0001](0001-turso-connection-design.md) - `Catalogue` owns a
  `turso::Database`, not a `Connection`; no connection pool.
- [0002](0002-turso-per-connection-pragmas.md) - which per-connection
  PRAGMAs `Catalogue::connect()` sets vs. leaves at their default, and why.
- [0003](0003-migrations.md) - hand-rolled migrations tracked via
  `PRAGMA user_version`, no framework dependency.
- [0004](0004-checked-arithmetic.md) - Checked arithmetic in release builds
- [0005](0005-hand-rolled-search.md) - turso has no FTS5; search is
  hand-rolled (title/person/tag atoms all resolve to plain SQL) instead.
