# 0009 - Grapheme-aware text buffer

**Status:** Accepted

## Context

The TUI's command line (`docs/tui.md`) is the single text-entry surface: search
queries, `/` commands, and delegated edits such as a score's description.
Descriptions are free-form and may contain emoji, so the buffer cannot assume
one code point is one visible character.

Three things needed deciding before writing the editor: how the buffer is
stored, what the cursor is an offset into, and what unit movement and deletion
operate on.

## Decision

- The buffer is a `String`.
- The cursor is a byte offset into it, held at an extended grapheme cluster
  boundary at all times (and never below the locked mode prefix).
- Movement and deletion operate on whole grapheme clusters, via
  `unicode-segmentation`.
- The rendered cursor column is `unicode-width`'s width of the slice before
  the cursor.
- The buffer is not normalized while editing.

This adds `unicode-segmentation` and `unicode-width` as direct dependencies.

## Why

### `String` over `Vec<char>` or a rope

`Vec<char>` makes indexing trivial, but every boundary in the system speaks
`&str`: the query parser (`src/query.rs`), prefix detection for the
`@@`/`##`/`/` modes, and the `$EDITOR` round-trip. Search runs on every
debounced keystroke, so conversions would not be confined to rendering. It also
doesn't solve the actual problem, since a `char` is a code point rather than a
visible character - emoji break under `Vec<char>` exactly as they do under
`String`.

A rope or gap buffer is sized for documents with frequent interior edits.
`docs/tui.md` routes anything long to `$EDITOR`, capping this buffer at a query
or a short description, so every operation is O(n) over a few hundred bytes.

### Graphemes, not code points

A user-perceived character is an extended grapheme cluster (UAX #29), which may
span several code points - a ZWJ emoji sequence is seven code points and one
cluster. Moving or deleting by `char` steps into the middle of such a sequence
and leaves mangled fragments. This is also why deletion drains a cluster range
rather than calling `String::remove`, which removes a single code point.

Slicing at the cursor is safe because of the boundary invariant: grapheme
boundaries within a prefix agree with those in the full string when the split
point is itself a boundary.

### Width matched to ratatui

`Cell` stores its symbol as a string rather than a `char` precisely so a cell
can carry a cluster - "this accepts unicode grapheme clusters which might
take up more than one cell" (`ratatui-core` 0.1.0, `src/buffer/cell.rs:12`).
When writing a string into the buffer, ratatui splits it with
`UnicodeSegmentation::graphemes(.., true)` and measures with `unicode-width`
(`src/buffer/buffer.rs:349`), which is the same function and the same extended
cluster flag used here. Computing the cursor column that way keeps the cursor
aligned with what ratatui actually drew.

This matching only holds if both sides resolve to one `unicode-width`.
`ratatui-core` constrains it to `>=0.2.0, <=0.2.2`, so the direct dependency
is declared as `0.2` - cargo then unifies both to a single copy. Requesting a
semver-incompatible major instead (`0.1`) would put two copies in the tree, each
free to score the same string differently.

That is alignment with ratatui's buffer, not with the terminal. `unicode-width`
scores a ZWJ family emoji at 8 columns (four emoji at 2, joiners at 0) while
a terminal rendering it as one glyph occupies 2. Nothing at this layer can
reconcile the two; self-consistency with the buffer is the achievable property.

### No normalization while editing

Normalizing per keystroke shifts byte offsets and invalidates the cursor.
Grapheme-aware movement is already correct over unnormalized text, so
normalization buys nothing during editing. Where a value needs it, it happens at
the catalogue boundary (see 0006, 0008) rather than in the editor.

## Sources

- [UAX #29, Unicode Text Segmentation](https://unicode.org/reports/tr29/) -
  definition of extended grapheme clusters.
- [`unicode-segmentation`](https://docs.rs/unicode-segmentation) -
  `graphemes(true)` selects extended clusters.
- `ratatui-core` 0.1.0, `src/buffer/cell.rs:12` - a `Cell`'s symbol holds a
  grapheme cluster; `src/buffer/buffer.rs:349` - text is split with
  `graphemes(.., true)` and measured with `unicode-width`; `Cargo.toml` pins
  `unicode-width` to `>=0.2.0, <=0.2.2`.
- `docs/tui.md` - command line scope, the locked mode prefix, and the `$EDITOR`
  escape hatch.
