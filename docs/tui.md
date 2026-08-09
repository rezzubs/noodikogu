# Management TUI

Design for the local terminal UI: layout, navigation model, and the
command grammar it exposes. This is a living document, unlike ADRs.

## Scope

The TUI is the local interface to the *catalogue*, and is treated as an
implicit "admin" user: no login, no per-user permissions. A web app with real
user accounts is separate and nothing here designs for it beyond noting the
places where its existence matters (concurrency, below). All core catalogue
functionality (scores, people, tags, roles, titles) should be reachable from
the TUI.

## Layout

Two panels:

- **Command line** (bottom) - a single control surface for search queries,
  free-text editing, and `/` commands. In addition to regular search, any text
  editing the Browser delegates (a description, a title) is also done by the
  command line.
- **Browser** (top) - the display and navigation surface. Shows lists,
  drills into detail, doesn't accept free text directly.

`Tab` switches focus between them.

## Command line

### Grammar

- No prefix - a score query (boolean search over titles/people/tags, see [ADR
  0005](decisions/0005-hand-rolled-search.md) and `src/query.rs`).
- `@@` - a people query, by name-component prefix.
- `##` - a tags query, by name prefix.
- `/` - a command, e.g. `/new <title>` to create a score. Tags, people, and
  roles are not created standalone; they only come into existence attached to a
  score, via the Browser's score-editing sub-view (see below). More commands get
  added as management actions surface that don't fit as a Browser action.

### Contextual sub-modes

Some Browser actions borrow the Command line rather than duplicating a search
UI - e.g. "attach a person" from inside a score's People section reuses the
people-query grammar. Rather than silently changing what plain text means,
the Command line inserts the mode's prefix (`@@` or `##`) as a locked, dimmed
segment at the start of the line: visible so the mode is never ambiguous,
immutable so the user can't accidentally delete it. Backspace at the start
of the editable region stops at that boundary instead of eating into it. The
Command line's title (see Browser breadcrumbs below) also reflects the active
mode, e.g. "Attach person" vs. "Search".

Text editing actions like updating the description of a score will also use the
command line.

### Sizing

Starts at 1 row. Grows as typed text overflows, up to half the terminal's
height; past that it scrolls internally rather than growing further, so the
Browser always keeps at least half the screen.

### Long text

A keybind (exact binding TBD at implementation time) opens the current contents
of the command line in `$EDITOR`, for anything long enough that inline editing
stops being pleasant.

## Browser

### Navigation model

Layered - a level is always a list of rows to select from:

1. Top level: lexicographic list of scores (default), or of people/tags under an
   active `@@`/`##` query, or query results under any other query.
2. Activating a row descends into that item's sections (title, description,
   people, tags, ...) plus an actions block.
3. Activating a section reveals its full content (trimming lifted) and, if the
   section is itself list-shaped (people, tags), descends into that list.
4. Selecting an entry in a list-shaped section (e.g. one attached person)
   descends further into that entry's own sections (their roles on this score,
   etc.).
5. This can be done recursively forever. For example: select a score -> select an
   author -> display author scores -> select a score -> ...

### Rows

A row is a single selectable unit, and can be more than one line: a bold section
header with its content indented below, or a non-interactable sub-list (e.g. an
author list previewed inside a score row). What a row can never have is internal
horizontal navigation between fields; left and right are reserved for level
navigation (below), so anything a row shows has to be reachable by descending
into it, not by moving sideways within it.

The top-level score list is one row per score, title left-aligned and
flex-width. A trailing badge area, right-aligned in fixed-width slots, is
planned but not yet built: it depends on a not-yet-implemented tag importance
ranking, which would let a manually-ranked tag (e.g. a catalogue `id` tag)
surface here for at-a-glance scanning. Intended behavior once that ranking
exists: show as many of the highest-ranked tags as fit, a `+n` overflow
indicator for the rest, and a contextual badge or two (e.g. "has a PDF
attachment") if there's still room - degrading to just the overflow count, or
dropping the badge area entirely, on a narrow terminal.

### Actions

Each level's list can end in an actions block: operations scoped to that level
(e.g. "Delete score" at a score's top section list, "Add tag" at its tags
section). Pinned to the bottom, kept in a visually and positionally distinct
group from data rows so selecting one is never confused with selecting data.

### Breadcrumbs

Full path, top-left of the Browser (`Block`'s `title_top`), e.g.
`Scores > Päike > People > Valter Soosalu > Scores`.
Leftmost segments abbreviate first when the path doesn't fit.

For example:

`S>P> Valter Soosalu > Scores`

For especially long recursions which don't fit even when abbreviated an ellipsis
will be used to mark "many layers".

`S>...> Scores`

### Keys

Three equivalent bindings, all live wherever they make sense (vi-style doesn't
apply inside the Command line, since it isn't a modal editor):

| Action           | vi      | emacs             | arrows       |
|------------------|---------|-------------------|--------------|
| up / down        | `k`/`j` | `ctrl-p`/`ctrl-n` | Up/Down      |
| back             | `h`     | `ctrl-b`          | Left, Esc    |
| forward / select | `l`     | `ctrl-f`          | Right, Enter |

Back moves up exactly one level per keypress; no multi-level jump. `Shift` can
be used together with back/forward to move to the top/bottom layer (the bottom
is remembered until another tree is entered).

### Fetching

A level backed by a query only fetches as many results as fit the current
screen; scrolling past the fetched page fetches the next one. The buffers for
next/previous screens should also be kept in memory. The Browser's title area
shows pagination info on the right. Resizing the terminal re-fetches to match
the new page size. Selection is tracked by item identity, not row index or page
number: re-entering a level you navigated out of, or resizing mid-view, restores
the same item as selected rather than resetting to the top.

## Cross-cutting concerns

### Destructive actions

Confirmation prompt before anything destructive (deleting a score, detaching a
person or tag). A staged/transactional mode, where several changes are queued
and committed together at the end, is worth keeping in mind but explicitly not
part of the initial design - it adds real complexity (a pending-changes model,
conflict handling between staged and live state) for a workflow that doesn't
exist yet.

### Orphan cleanup

Detaching the last score referencing a person, tag, or role does not imply
deleting it. Whether and how orphaned entities get garbage collected (automatic
vs. manual, immediate vs. batched) is undecided and explicitly out of scope for
the TUI design - it's a catalogue-level policy question, not a UI one.

### Concurrency

No optimistic-concurrency handling. Realistic usage has few simultaneous
writers, and the mutations that exist already check the target still exists
before writing (e.g. `SetDescriptionError::ScoreNotFound`), so a race can
overwrite a concurrent edit but can't corrupt data. An audit log (separately
planned, not designed here) would improve visibility into this even without
solving it outright.
