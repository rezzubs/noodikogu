//! A grapheme-aware text buffer with a cursor, plus the soft-wrap layout it
//! needs for rendering, scrolling, and vertical cursor movement.

use std::ops::Range;

use ratatui::{
    layout::Rect,
    style::Color,
    text::Line,
    widgets::{Block, BorderType, Paragraph},
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

const NORMAL_COLOR: Color = Color::Reset;
const SELECTION_COLOR: Color = Color::Green;

/// Rows of context kept between the cursor and the box's top/bottom edge
/// while scrolling - see [`scroll`].
const SCROLLOFF: usize = 2;

/// A grapheme-aware text buffer with a cursor.
// Fields are private so the invariant on `cursor` (see below) can only be
// broken from inside this module.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct CommandLine {
    /// The full text content of the command line.
    content: String,
    /// A byte offset into `content`, always sitting at an extended grapheme
    /// cluster boundary.
    ///
    /// Every method that can move this has to preserve that - see
    /// [`snap_forward`] for why insertion and deletion can't just do byte
    /// arithmetic and call it done.
    cursor: usize,
}

impl CommandLine {
    pub fn content(&self) -> &str {
        &self.content
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn is_empty(&self) -> bool {
        self.content.is_empty()
    }

    /// Inserts `character` at the cursor and advances past it.
    pub fn insert_char(&mut self, character: char) {
        // relies on cursor never leaving `content` and always being on a char
        // boundary.
        self.content.insert(self.cursor, character);

        // `character` can combine with whatever follows it (e.g. a base
        // character inserted right before an existing combining mark pulls
        // that mark backward onto it), so "right after the inserted byte"
        // isn't guaranteed to still be a boundary - see `snap_forward`.
        self.cursor = snap_forward(&self.content, self.cursor + character.len_utf8());
    }

    /// Deletes the grapheme cluster immediately before the cursor
    /// (Backspace). Does nothing at the start of the buffer.
    pub fn delete_before(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let start = prev_grapheme_boundary(&self.content, self.cursor);
        self.content.drain(start..self.cursor);
        // Deleting a cluster can newly-adjacent two clusters that combine
        // (the one now ending where `start` is, and whatever now begins
        // there) - see `snap_forward`.
        self.cursor = snap_forward(&self.content, start);
    }

    /// Deletes the grapheme cluster at the cursor (Delete). Does nothing at
    /// the end of the buffer.
    pub fn delete_on(&mut self) {
        if self.cursor >= self.content.len() {
            return;
        }
        let end = next_grapheme_boundary(&self.content, self.cursor);
        self.content.drain(self.cursor..end);
        // Same re-combination risk as `delete_before`, even though the
        // byte value of `self.cursor` itself doesn't change here.
        self.cursor = snap_forward(&self.content, self.cursor);
    }

    /// Move the cursor one grapheme to the left.
    pub fn move_left(&mut self) {
        self.cursor = prev_grapheme_boundary(&self.content, self.cursor);
    }

    /// Move the cursor one grapheme to the right.
    pub fn move_right(&mut self) {
        self.cursor = next_grapheme_boundary(&self.content, self.cursor);
    }

    /// Move the cursor to the closest matching column on the wrapped visual
    /// line above, given the current wrap `width`.
    ///
    /// Does nothing if the cursor is already on the first visual line - that
    /// edge is reserved for command-history navigation.
    pub fn move_up(&mut self, width: u16) {
        // There is no "goal column" memory across repeated calls: each call
        // bases its target column on the cursor's position just before it,
        // not on some earlier, remembered column. Simpler than a full editor's
        // persistent goal column. Revisit only if it feels wrong in practice.

        let lines = wrap(&self.content, width);
        let (current_line, target_column) =
            resolve_cursor_line(&self.content, &lines, self.cursor, width);
        let Some(target_line) = current_line.checked_sub(1) else {
            return;
        };
        self.cursor = byte_offset_at_column(&self.content, &lines[target_line], target_column);
    }

    /// Symmetric with [`Self::move_up`].
    pub fn move_down(&mut self, width: u16) {
        let lines = wrap(&self.content, width);
        let (current_line, target_column) =
            resolve_cursor_line(&self.content, &lines, self.cursor, width);
        let target_line = current_line + 1;
        if target_line >= lines.len() {
            return;
        }
        self.cursor = byte_offset_at_column(&self.content, &lines[target_line], target_column);
    }

    /// How many visual lines this content needs, at `width` columns, to show
    /// its cursor.
    ///
    /// For a caller to decide how tall to make the box *before* laying it out.
    /// At that point there's no [`Rect`] yet for [`Self::view`] to derive this
    /// from, so it has to be asked for up front instead (see `Model::view`).
    /// Recomputes the same [`wrap`]/[`cursor_display`] that `view` computes
    /// again once the area is known - cheap enough for a command line's worth
    /// of text that it's not worth threading the result through instead.
    pub fn desired_height(&self, width: u16) -> usize {
        let lines = wrap(&self.content, width);
        cursor_display(&self.content, &lines, self.cursor, width).total_lines
    }

    /// Builds this command line's widget, and where its cursor should sit on
    /// screen, for a box already laid out into `area` (border included) at
    /// `width` content columns.
    pub fn view(&self, width: u16, area: Rect, focused: bool) -> View<'_> {
        // Re-derived from the actual, already-laid-out `area` rather than
        // trusting a caller's earlier height request: robust against `Layout`
        // having had to shrink that request on an extreme terminal.
        let inner_height = usize::from(area.height.saturating_sub(2));

        let line_boundaries = wrap(&self.content, width);
        let cursor = cursor_display(&self.content, &line_boundaries, self.cursor, width);
        let scroll_offset = scroll(cursor.line, cursor.total_lines, inner_height, SCROLLOFF);

        let text: Vec<Line> = line_boundaries
            .iter()
            .map(|line| Line::raw(&self.content[visible_range(&self.content, line)]))
            .collect();

        let widget = Paragraph::new(text)
            // No `.wrap()` call: every line here is already exactly as wide
            // as `width` allows, so this just selects the visible slice of
            // our own pre-wrapped rows.
            .scroll((u16::try_from(scroll_offset).unwrap_or(u16::MAX), 0))
            .block(
                Block::bordered()
                    .border_type(BorderType::Rounded)
                    .border_style(if focused {
                        SELECTION_COLOR
                    } else {
                        NORMAL_COLOR
                    })
                    .title("Search"),
            );

        let cursor_position = (focused && inner_height > 0).then(|| {
            // Always < inner_height: `scroll` is constructed above so the
            // cursor's line never falls outside the visible window.
            let row_in_window =
                u16::try_from(cursor.line.saturating_sub(scroll_offset)).unwrap_or(u16::MAX);
            (
                area.x + 1 + cursor.column, // +1 = left border
                area.y + 1 + row_in_window, // +1 = top border
            )
        });

        View {
            widget,
            cursor_position,
        }
    }
}

/// The rendered widget for a [`CommandLine`], and where its cursor should be
/// shown on screen - `None` when it shouldn't be shown at all (unfocused, or
/// the box has no visible rows).
pub struct View<'a> {
    pub widget: Paragraph<'a>,
    pub cursor_position: Option<(u16, u16)>,
}

/// A visual (soft-wrapped) line: every byte of `content` it is responsible for,
/// plus why it starts/ends where it does.
#[derive(Debug, Clone, PartialEq, Eq)]
struct VisualLine {
    /// Every byte of `content` this line is responsible for.
    range: Range<usize>,
    /// This line's leading grapheme is whitespace that overflowed the
    /// previous line and was absorbed here rather than being real content -
    /// see the "word that exactly fills a line" case in [`wrap`]'s doc
    /// comment. [`visible_range`] trims it from what actually renders.
    leading_whitespace_absorbed: bool,
    /// This line ends because the user typed an explicit newline, not
    /// because of a wrap. [`visible_range`] trims the trailing newline
    /// grapheme from what renders and the cursor-redirect logic in
    /// [`cursor_display`]/[`resolve_cursor_line`] treats a cursor sitting right
    /// after it as belonging to the start of the next line rather than "behind"
    /// the invisible newline on this one.
    ends_with_newline: bool,
}

/// Soft-wraps `content` to `width` terminal columns, breaking preferentially
/// at whitespace and falling back to a hard break mid-word when a word (or a
/// single grapheme) is wider than `width`. Also hard-breaks on every explicit
/// newline the user typed, regardless of `width`.
///
/// Whitespace is never trimmed or dropped - every byte of `content` ends up
/// in exactly one returned [`VisualLine`]. [`VisualLine`] ranges are always
/// contiguous: `result[0].range.start == 0`, `result[i].range.end == result[i
/// \+ 1].range.start` and `result.last().range.end == content.len()`. This is
/// what lets a caller map a cursor byte offset to a visual line unambiguously
/// (see [`locate_line`]).
///
/// Always returns at least one line - `0..0` for empty `content` - so you
/// still get one row with nothing typed. A trailing newline likewise always
/// leaves one fresh, empty line after it, for the same reason.
///
/// `width == 0` returns a single unwrapped line spanning all of `content`:
/// there is no usable width to wrap against.
///
/// A word that exactly fills a line pushes the whitespace right after it onto
/// a new line of its own: `"hello world"` at width 5 wraps to `["hello", "
/// world"]`, not `["hello ", "world"]` - "hello" alone already uses the full
/// width, so the space has nowhere to go but a new line. [`visible_range`] is
/// then used to trim the leading whitespace for display so "world" appears as
/// the full line.
fn wrap(content: &str, width: u16) -> Vec<VisualLine> {
    if width == 0 {
        return vec![VisualLine {
            range: 0..content.len(),
            leading_whitespace_absorbed: false,
            ends_with_newline: false,
        }];
    }

    let mut lines: Vec<VisualLine> = Vec::new();
    let mut current_line_start = 0;

    // Terminal columns taken up by the line that's currently being built.
    //
    // A grapheme that becomes an invisible (leading space) is deliberately
    // never added (it will be trimmed by [`visible_range`]).
    let mut current_line_width: u16 = 0;

    // Whether the line currently being built starts with an absorbed
    // whitespace grapheme - carried from the moment that line starts
    // (see the overflow handling below) to whenever it's finally pushed,
    // and consumed into `VisualLine::leading_whitespace_absorbed` there.
    let mut current_line_leading_whitespace_absorbed = false;

    // The most recent point since `line_start` where a word began right after
    // whitespace - a candidate place to break without splitting a word. Only
    // usable if it's strictly after `line_start`: if the word that's currently
    // overflowing started exactly at `line_start`, the whole line so far is one
    // contiguous word with no earlier break point, and a hard break has to be
    // used instead (see the overflow handling below).
    let mut break_at: Option<(usize, u16)> = None;

    // start-of-line counts as "after whitespace"
    let mut previous_grapheme_was_space = true;

    for (byte_offset, grapheme) in content.grapheme_indices(true) {
        let current_grapheme_is_space = grapheme.chars().all(char::is_whitespace);
        if previous_grapheme_was_space && !current_grapheme_is_space {
            break_at = Some((byte_offset, current_line_width));
        }
        previous_grapheme_was_space = current_grapheme_is_space;

        let grapheme_width = u16::try_from(grapheme.width()).unwrap_or(u16::MAX);

        // `current_line_width > 0` guarantees forward progress even when a
        // single grapheme alone is wider than `width` (e.g. a 2-wide emoji
        // in a 1-column box): such a grapheme is still placed - there is no
        // narrower placement for it - and only the *next* grapheme trips
        // the overflow check, so every iteration either places a grapheme
        // or breaks a line, never both zero times.
        //
        // Whether this iteration just hard-broke on an overflowing
        // whitespace grapheme, making that grapheme the sole leading
        // (and, per `visible_range`, invisible) character of a new line -
        // see the comment below on why such a grapheme must not be
        // charged against that line's budget.
        let mut leading_space_absorbed = false;

        if current_line_width > 0 && current_line_width.saturating_add(grapheme_width) > width {
            match break_at {
                // Only relocate a whole word via `break_at` when the word
                // itself is what doesn't fit (the overflowing grapheme is
                // non-whitespace). If the overflowing grapheme is whitespace,
                // always hard-break right before it instead, even when an
                // earlier `break_at` is usable - otherwise a lone trailing
                // space drags the word before it onto the next line too,
                // even though that word fit on the line just fine (see
                // `a_word_before_an_overflowing_space_is_not_relocated` below).
                Some((break_byte, break_column))
                    if break_byte > current_line_start && !current_grapheme_is_space =>
                {
                    lines.push(VisualLine {
                        range: current_line_start..break_byte,
                        leading_whitespace_absorbed: current_line_leading_whitespace_absorbed,
                        ends_with_newline: false,
                    });
                    current_line_start = break_byte;
                    current_line_width -= break_column;
                    // A word-boundary relocation always starts the new line
                    // on the word itself, never on whitespace.
                    current_line_leading_whitespace_absorbed = false;
                }
                _ => {
                    lines.push(VisualLine {
                        range: current_line_start..byte_offset,
                        leading_whitespace_absorbed: current_line_leading_whitespace_absorbed,
                        ends_with_newline: false,
                    });
                    current_line_start = byte_offset;
                    current_line_width = 0;
                    leading_space_absorbed = current_grapheme_is_space;
                    current_line_leading_whitespace_absorbed = leading_space_absorbed;
                }
            }
            break_at = None;
        }

        // A grapheme that just became a new line's sole leading whitespace
        // this way is exactly what `visible_range` trims - it will never be
        // rendered as part of this line. Charging it here anyway would give
        // this line only `width - 1` usable columns for its real content
        // while every other line gets the full `width`, wrapping this
        // line's content one column earlier than it visually needs to -
        // the same class of `full`/`visible` mismatch fixed elsewhere in
        // this module, just in `wrap`'s own budget instead of in cursor
        // placement or rendering.
        if !leading_space_absorbed {
            current_line_width = current_line_width.saturating_add(grapheme_width);
        }

        // An explicit newline always breaks the line right after itself,
        // regardless of how much width is left - it can never itself trigger
        // the overflow check above (its width is 0), so this has to be its own,
        // unconditional check.
        if grapheme.contains('\n') {
            lines.push(VisualLine {
                range: current_line_start..byte_offset + grapheme.len(),
                leading_whitespace_absorbed: current_line_leading_whitespace_absorbed,
                ends_with_newline: true,
            });
            current_line_start = byte_offset + grapheme.len();
            current_line_width = 0;
            current_line_leading_whitespace_absorbed = false;
            break_at = None;
        }
    }

    lines.push(VisualLine {
        range: current_line_start..content.len(),
        leading_whitespace_absorbed: current_line_leading_whitespace_absorbed,
        ends_with_newline: false,
    });
    lines
}

/// The sub-range of `full.range` that actually renders and counts toward
/// columns: `full.range` itself, minus a leading grapheme absorbed as
/// overflow ([`VisualLine::leading_whitespace_absorbed`]) and/or a trailing
/// newline grapheme ([`VisualLine::ends_with_newline`]).
fn visible_range(content: &str, full: &VisualLine) -> Range<usize> {
    let visible_start = if full.leading_whitespace_absorbed {
        content[full.range.clone()]
            .grapheme_indices(true)
            .next()
            .map_or(full.range.start, |(_, grapheme)| {
                full.range.start + grapheme.len()
            })
    } else {
        full.range.start
    };

    let visible_end = if full.ends_with_newline {
        content[full.range.clone()]
            .grapheme_indices(true)
            .next_back()
            .map_or(full.range.end, |(relative_offset, _)| {
                full.range.start + relative_offset
            })
    } else {
        full.range.end
    };

    visible_start..visible_end
}

/// Finds which visual line contains `cursor`.
///
/// A cursor sitting exactly at a wrap boundary is treated as belonging to the
/// *end* of the previous line rather than the start of the next one: scanning
/// from the front and stopping at the first line whose end reaches `cursor`
/// gives exactly that. Only the last line's end can equal `content.len()`, so
/// a cursor at the true end of the buffer still resolves correctly without a
/// separate case.
///
/// # Panics
///
/// Panics if `lines` is empty. Call this with the output of [`wrap`], which
/// never returns an empty `Vec`.
fn locate_line(lines: &[VisualLine], cursor: usize) -> usize {
    lines
        .iter()
        .position(|line| cursor <= line.range.end)
        .unwrap_or(lines.len().checked_sub(1).expect("empty lines"))
}

/// The column of `cursor` within `line`'s [`visible_range`], in terminal cells
/// (via `unicode-width`) rather than bytes or graphemes.
///
/// `cursor` is assumed to be at or after the visible range's start, which
/// [`locate_line`] guarantees: the only way to resolve to this line is to be
/// past the wrap boundary that precedes it, and a leading absorbed-whitespace
/// grapheme is itself part of that boundary region. `cursor` may fall past
/// the visible range's *end* though (inside a trimmed trailing newline) -
/// harmless here since that grapheme is always 0-width.
fn cursor_column(content: &str, line: &VisualLine, cursor: usize) -> u16 {
    let visible = visible_range(content, line);
    u16::try_from(content[visible.start..cursor].width()).unwrap_or(u16::MAX)
}

/// Where to show the cursor, and how many visual lines the box needs in
/// order to show it there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CursorDisplay {
    line: usize,
    column: u16,
    /// Usually `lines.len()`, except when a fresh line has to be counted
    /// for the cursor to land on - see [`line_is_full`].
    total_lines: usize,
}

/// Computes where `cursor` should render within `lines`, and how many visual
/// lines the box needs to show it there.
fn cursor_display(content: &str, lines: &[VisualLine], cursor: usize, width: u16) -> CursorDisplay {
    let line = locate_line(lines, cursor);
    let line_column = cursor_column(content, &lines[line], cursor);

    if redirects_to_next_line(&lines[line], line_column, cursor, width) {
        // No free cell after this line's last character to show the cursor -
        // rendering it here would put it right on the box's border (or, for
        // an explicit newline, right on top of the invisible newline
        // character itself). Redirect to the start of whatever comes next:
        // the real following line if the cursor isn't on the last one, or
        // one fresh (empty) extra line, the same way the box already grows
        // as typed text overflows, if it is.
        let next_line = line + 1;
        CursorDisplay {
            line: next_line,
            column: 0,
            total_lines: lines.len().max(next_line + 1),
        }
    } else {
        CursorDisplay {
            line,
            column: line_column,
            total_lines: lines.len(),
        }
    }
}

/// True when a line has used up every column, leaving no free cell after its
/// last character.
///
/// This applies regardless of whether more content follows on a later line or
/// the cursor is at the true end of the buffer - either way, rendering a cursor
/// positioned at the end of *this* line would put it right on the box's border,
/// which [`cursor_display`] avoids by moving on to the next line instead. `>=`
/// (not `==`) also covers a grapheme wide enough to push `line_column` past
/// `width`.
fn line_is_full(line_column: u16, width: u16) -> bool {
    width > 0 && line_column >= width
}

/// Whether a cursor sitting at `cursor` on `line` should be treated as
/// belonging to the start of the line after it instead.
fn redirects_to_next_line(line: &VisualLine, line_column: u16, cursor: usize, width: u16) -> bool {
    line_is_full(line_column, width) || (line.ends_with_newline && cursor == line.range.end)
}

/// The visual line and in-line column that line-to-line movement
/// (`CommandLine::move_up`/`move_down`) should treat `cursor` as occupying.
///
/// Unlike `cursor_display`, this never redirects onto a phantom line past the
/// end of `lines`: movement has nothing to move onto past the real last line,
/// so there the plain [`locate_line`] result is already correct.
fn resolve_cursor_line(
    content: &str,
    lines: &[VisualLine],
    cursor: usize,
    width: u16,
) -> (usize, u16) {
    let line = locate_line(lines, cursor);
    let line_column = cursor_column(content, &lines[line], cursor);

    if redirects_to_next_line(&lines[line], line_column, cursor, width) {
        let next_line = line + 1;
        if next_line < lines.len() {
            return (next_line, 0);
        }
    }

    (line, line_column)
}

/// The first visual line to show, keeping `cursor_line` within `scrolloff`
/// rows of the box's top and bottom edge - automatically shrinking the
/// margin when `inner_height` is too short to fit it on both sides, since
/// `inner_height >= 2 * scrolloff + 1` is exactly the condition that makes
/// the top-margin and bottom-margin bounds on the result compatible.
///
/// Pure and stateless: no scroll position needs to be persisted between
/// frames. Clamping `cursor_line - scrolloff` into `[0, max_scroll]` always
/// picks the unique value that satisfies both margins whenever one is
/// achievable at all, and relaxes the margin gracefully at the true
/// start/end of the buffer, where it can't be satisfied on both sides at
/// once (see the tests below for the worked cases this relies on).
fn scroll(cursor_line: usize, total_lines: usize, inner_height: usize, scrolloff: usize) -> usize {
    let effective_scrolloff = scrolloff.min(inner_height.saturating_sub(1) / 2);
    let max_scroll = total_lines.saturating_sub(inner_height);
    cursor_line
        .saturating_sub(effective_scrolloff)
        .min(max_scroll)
}

/// The byte offset within `line`'s [`visible_range`] whose column is the
/// closest to `target_column` without exceeding it, landing on a grapheme
/// boundary.
///
/// The visible range's end is itself always a grapheme boundary so falling
/// through to it when `target_column` exceeds the line's own width still
/// returns a valid boundary - the closest available position before a trimmed
/// trailing newline, never inside or past it.
fn byte_offset_at_column(content: &str, line: &VisualLine, target_column: u16) -> usize {
    let visible = visible_range(content, line);
    let mut column_used = 0u16;
    for (relative_offset, grapheme) in content[visible.clone()].grapheme_indices(true) {
        let grapheme_width = u16::try_from(grapheme.width()).unwrap_or(u16::MAX);
        if column_used.saturating_add(grapheme_width) > target_column {
            return visible.start + relative_offset;
        }
        column_used += grapheme_width;
    }
    visible.end
}

/// The byte offset of the grapheme cluster immediately before `cursor`.
///
/// Requires `cursor` to already be a grapheme boundary (true by
/// [`CommandLine`]'s invariant): that's what guarantees the prefix's last
/// cluster is exactly the one immediately preceding `cursor`, not some
/// fragment of it.
fn prev_grapheme_boundary(content: &str, cursor: usize) -> usize {
    content[..cursor]
        .grapheme_indices(true)
        .next_back()
        .map_or(0, |(start, _)| start)
}

/// The byte offset immediately after the grapheme cluster that starts at
/// `cursor`. Requires `cursor` to already be a grapheme boundary, for the
/// same reason as [`prev_grapheme_boundary`].
fn next_grapheme_boundary(content: &str, cursor: usize) -> usize {
    content[cursor..]
        .grapheme_indices(true)
        .nth(1)
        .map_or(content.len(), |(relative_offset, _)| {
            cursor + relative_offset
        })
}

/// Snaps `offset` forward to the nearest actual grapheme boundary of
/// `content`.
///
/// Needed after every edit because the buffer is not normalized. Inserting or
/// deleting next to an unnormalized combining mark can change which cluster
/// a given byte offset falls in (e.g. inserting a base character immediately
/// before a combining mark pulls that mark backward onto it, so a position that
/// used to sit right after the insertion point can end up strictly inside the
/// newly-formed cluster). Without this snap, `cursor` could end up violating
/// its own invariant.
fn snap_forward(content: &str, offset: usize) -> usize {
    if offset >= content.len() {
        return content.len();
    }
    content
        .grapheme_indices(true)
        .map(|(start, _)| start)
        .find(|&start| start >= offset)
        .unwrap_or(content.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_contiguous(lines: &[VisualLine], content: &str) {
        assert_eq!(lines[0].range.start, 0, "first line must start at 0");
        assert_eq!(
            lines
                .last()
                .expect("wrap never returns an empty Vec")
                .range
                .end,
            content.len(),
            "last line must end at content.len()"
        );
        for pair in lines.windows(2) {
            assert_eq!(
                pair[0].range.end, pair[1].range.start,
                "lines must be contiguous, no gaps or overlaps"
            );
        }
    }

    fn full_text<'a>(content: &'a str, line: &VisualLine) -> &'a str {
        &content[line.range.clone()]
    }

    fn visible_text<'a>(content: &'a str, line: &VisualLine) -> &'a str {
        &content[visible_range(content, line)]
    }

    #[test]
    fn empty_content_wraps_to_one_empty_line() {
        let lines = wrap("", 10);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].range, 0..0);
    }

    #[test]
    fn width_zero_returns_one_unwrapped_line() {
        let lines = wrap("hello world", 0);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].range, 0..11);
    }

    #[test]
    fn wraps_at_whitespace_when_it_fits() {
        let content = "hello world foo";
        let lines = wrap(content, 9);
        assert_contiguous(&lines, content);
        assert_eq!(full_text(content, &lines[0]), "hello ");
        assert_eq!(full_text(content, &lines[1]), "world foo");
        // This is a normal word-boundary break so "world" is at the beginning
        // of the second line line and nothing is trimmed.
        assert_eq!(visible_text(content, &lines[1]), "world foo");
    }

    #[test]
    fn a_word_that_exactly_fills_a_line_does_not_strand_its_trailing_space() {
        let content = "hello world";
        let lines = wrap(content, 5);
        assert_contiguous(&lines, content);
        assert_eq!(full_text(content, &lines[0]), "hello");
        // The overflowing space is absorbed onto a new line same as always,
        // but since it isn't charged against that line's budget (see the
        // `leading_space_absorbed` comment in `wrap`) "world" still fits right
        // after it rather than needing a line of its own.
        assert_eq!(full_text(content, &lines[1]), " world");
        assert_eq!(visible_text(content, &lines[1]), "world");
    }

    #[test]
    fn a_word_before_an_overflowing_space_is_not_relocated() {
        let content = "ab cd ";
        let lines = wrap(content, 5);
        assert_contiguous(&lines, content);
        assert_eq!(full_text(content, &lines[0]), "ab cd"); // word "cd" stays put
        assert_eq!(full_text(content, &lines[1]), " "); // only the space moves
        assert_eq!(visible_text(content, &lines[1]), "");
    }

    #[test]
    fn a_line_that_exactly_fills_with_multiple_words_is_not_split_by_a_later_space() {
        // Motivating bug: "hello world" (11 chars) exactly fills width 11
        // on its own. Typing a trailing space used to retroactively relocate
        // "world" (the most recent word boundary) onto a new line, even though
        // it fit perfectly well where it already was.
        let content = "hello world foo";
        let lines = wrap(content, 11);
        assert_contiguous(&lines, content);
        assert_eq!(full_text(content, &lines[0]), "hello world");
        assert_eq!(full_text(content, &lines[1]), " foo");
        // The leading space in `full` is the absorbed overflow trigger, not
        // something the user meant to indent "foo" with.
        assert_eq!(visible_text(content, &lines[1]), "foo");
    }

    #[test]
    fn a_line_starting_with_an_absorbed_space_gets_the_full_width_like_any_other_line() {
        // Motivating bug: "aaaa bbbb" (9 chars) fills width 9 exactly so the
        // following space is absorbed onto its own line the same way as in
        // `a_line_that_exactly_fills_with_multiple_words_is_not_split_by_a_later_space`
        // above. "cccc dddd" (9 chars) is typed right after it (also 9
        // characters) so it must wrap identically. The absorbed leading space
        // is invisible (trimmed by `visible_range`) so it must not itself eat
        // into the budget available for the real content that follows it on
        // that line.
        let content = "aaaa bbbb cccc dddd";
        let width = 9;
        let lines = wrap(content, width);
        assert_contiguous(&lines, content);
        assert_eq!(lines.len(), 2, "must not spill onto a third line");
        assert_eq!(full_text(content, &lines[0]), "aaaa bbbb");
        assert_eq!(full_text(content, &lines[1]), " cccc dddd");
        assert_eq!(visible_text(content, &lines[1]), "cccc dddd");
    }

    #[test]
    fn inserting_a_word_after_a_full_line_does_not_indent_the_continuation() {
        // Motivating bug: "test test test" (14 chars) exactly fills width 14.
        // Inserting " test" must not show up as "| test|" on the next line.
        let content = "test test test";
        let width = 14;
        assert_eq!(u16::try_from(content.len()).unwrap(), width);

        let inserted = format!("{content} test");
        let lines = wrap(&inserted, width);
        assert_contiguous(&lines, &inserted);
        assert_eq!(full_text(&inserted, &lines[0]), "test test test");
        assert_eq!(full_text(&inserted, &lines[1]), " test");
        assert_eq!(visible_text(&inserted, &lines[1]), "test");
    }

    #[test]
    fn a_word_wider_than_the_line_hard_breaks() {
        let content = "AAAAA";
        let lines = wrap(content, 3);
        assert_contiguous(&lines, content);
        assert_eq!(full_text(content, &lines[0]), "AAA");
        assert_eq!(full_text(content, &lines[1]), "AA");
        // A mid-word hard break never starts on whitespace, so nothing to
        // trim - `visible` matches `full`.
        assert_eq!(visible_text(content, &lines[1]), "AA");
    }

    #[test]
    fn a_wide_grapheme_narrower_than_the_box_still_gets_its_own_line() {
        // U+1F600 (grinning face) is 2 columns wide.
        let content = "a\u{1F600}b";
        let lines = wrap(content, 1);
        assert_contiguous(&lines, content);
        assert_eq!(lines.len(), 3);
        assert_eq!(full_text(content, &lines[0]), "a");
        assert_eq!(full_text(content, &lines[1]), "\u{1F600}");
        assert_eq!(full_text(content, &lines[2]), "b");
    }

    #[test]
    fn an_explicit_newline_forces_a_break_regardless_of_width() {
        let content = "hi\nbye";
        let lines = wrap(content, 80); // plenty of room, no soft-wrap involved
        assert_contiguous(&lines, content);
        assert_eq!(lines.len(), 2);
        assert_eq!(full_text(content, &lines[0]), "hi\n");
        // The newline itself never renders - a literal `\n` inside a
        // ratatui `Line` wouldn't start a new row on screen.
        assert_eq!(visible_text(content, &lines[0]), "hi");
        assert_eq!(full_text(content, &lines[1]), "bye");
        assert_eq!(visible_text(content, &lines[1]), "bye");
    }

    #[test]
    fn a_trailing_newline_leaves_a_fresh_empty_line_after_it() {
        let content = "hi\n";
        let lines = wrap(content, 80);
        assert_contiguous(&lines, content);
        assert_eq!(lines.len(), 2);
        assert_eq!(visible_text(content, &lines[0]), "hi");
        assert_eq!(visible_text(content, &lines[1]), "");
    }

    #[test]
    fn real_leading_whitespace_right_after_a_newline_is_not_trimmed() {
        // Motivating bug this test guards against: before `VisualLine`
        // tracked *why* a line starts with whitespace, `visible_range`
        // guessed from "is this the first line" alone - a guess that was
        // only safe because a genuine word-wrap break can never start a
        // line on whitespace. An explicit newline breaks that assumption:
        // the line right after it can start with a real, intentionally
        // typed space, which must render, not get silently eaten like an
        // absorbed word-wrap space would be.
        let content = "hello\n world";
        let lines = wrap(content, 80);
        assert_contiguous(&lines, content);
        assert_eq!(lines.len(), 2);
        assert_eq!(visible_text(content, &lines[0]), "hello");
        assert_eq!(visible_text(content, &lines[1]), " world");
    }

    #[test]
    fn consecutive_newlines_produce_a_blank_line_between_them() {
        let content = "a\n\nb";
        let lines = wrap(content, 80);
        assert_contiguous(&lines, content);
        assert_eq!(lines.len(), 3);
        assert_eq!(visible_text(content, &lines[0]), "a");
        assert_eq!(visible_text(content, &lines[1]), "");
        assert_eq!(visible_text(content, &lines[2]), "b");
    }

    #[test]
    fn locate_line_at_an_exact_boundary_picks_the_previous_line() {
        let content = "hello world";
        let lines = wrap(content, 5); // ["hello", " world"]
        let boundary = lines[0].range.end;
        assert_eq!(boundary, lines[1].range.start);
        // Locate line will put it at the end of the first line which is
        // correct in most cases. The only case where it doesn't make sense is
        // when the line is completely full. That case has special handling in
        // [`cursor_display`].
        assert_eq!(locate_line(&lines, boundary), 0);
    }

    #[test]
    fn locate_line_at_true_buffer_end_picks_the_last_line() {
        let content = "hello world";
        let lines = wrap(content, 5);
        assert_eq!(locate_line(&lines, content.len()), lines.len() - 1);
    }

    #[test]
    fn insert_and_delete_around_a_multi_codepoint_cluster_are_atomic() {
        // Family emoji: four codepoints joined by ZWJ, one grapheme cluster.
        let family = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}";
        let mut buffer = CommandLine::default();
        buffer.insert_char('a');
        for character in family.chars() {
            buffer.insert_char(character);
        }
        buffer.insert_char('b');
        assert_eq!(buffer.content(), format!("a{family}b"));

        buffer.delete_before(); // removes 'b'
        assert_eq!(buffer.content(), format!("a{family}"));

        buffer.delete_before(); // removes the whole cluster, not one codepoint
        assert_eq!(buffer.content(), "a");
    }

    #[test]
    fn move_up_is_a_no_op_on_the_first_visual_line() {
        let mut buffer = CommandLine::default();
        for character in "hi".chars() {
            buffer.insert_char(character);
        }
        buffer.move_left();
        let cursor_before = buffer.cursor();
        buffer.move_up(80);
        assert_eq!(buffer.cursor(), cursor_before);
    }

    #[test]
    fn move_down_is_a_no_op_on_the_last_visual_line() {
        let mut buffer = CommandLine::default();
        for character in "hi".chars() {
            buffer.insert_char(character);
        }
        let cursor_before = buffer.cursor();
        buffer.move_down(80);
        assert_eq!(buffer.cursor(), cursor_before);
    }

    #[test]
    fn move_up_then_down_returns_to_the_original_column_when_the_line_above_has_room() {
        let content = "hello world";
        // Lines: "hello " (6 wide) / "world" (5 wide). Line 0 has
        // room for every column line 1 can ask for, so neither
        // move clamps - this is what makes the round trip exact.
        // `move_up`/`move_down` have no goal-column memory of their own; see
        // `move_up_then_down_does_not_restore_the_original_column_when_the_line_above_is_shorter`
        // for what happens when that assumption doesn't hold.
        let width = 6;

        let mut buffer = CommandLine::default();
        for character in content.chars() {
            buffer.insert_char(character);
        }
        let end = buffer.cursor();

        buffer.move_up(width);
        assert_eq!(
            buffer.cursor(),
            5,
            "column 5 on \"hello \" is the trailing space, right before \"world\""
        );
        buffer.move_down(width);
        assert_eq!(buffer.cursor(), end);

        // Same property at the opposite edge: column 0.
        for _ in "world".chars() {
            buffer.move_left();
        }
        let line_start = buffer.cursor();
        assert_eq!(line_start, 6, "sanity check: at the start of \"world\"");

        buffer.move_up(width);
        assert_eq!(
            buffer.cursor(),
            0,
            "column 0 on \"hello \" is its first character"
        );
        buffer.move_down(width);
        assert_eq!(buffer.cursor(), line_start);
    }

    #[test]
    fn move_up_then_down_does_not_restore_the_original_column_when_the_line_above_is_shorter() {
        let content = "hi hello";
        // Lines: "hi " (3 wide) / "hello" (5 wide). Column 5 doesn't exist on
        // line 0, so `move_up` clamps to its end (column 3) instead. Nothing
        // remembers the original column 5 was requested - the clamped column
        // 3 is what `move_down` asks for on the way back - so the round trip
        // lands short of where it started.
        let width = 5;

        let mut buffer = CommandLine::default();
        for character in content.chars() {
            buffer.insert_char(character);
        }
        let end = buffer.cursor();

        buffer.move_up(width);
        assert_eq!(
            buffer.cursor(),
            3,
            "column 5 doesn't exist on \"hi \": clamped to its end instead"
        );

        buffer.move_down(width);
        assert_ne!(
            buffer.cursor(),
            end,
            "the clamped column (3) is what gets requested on the way back \
             down too, landing on \"hel|lo\" instead of the original end of \
             \"hello\""
        );
        assert_eq!(buffer.cursor(), 6);
    }

    #[test]
    fn move_down_onto_a_trimmed_line_lands_on_its_first_visible_character() {
        // Movement uses the same `visible` ranges as rendering, so landing
        // "at column 0" of a continuation line means landing on its first
        // *rendered* character, not on the invisible absorbed space before it -
        // otherwise Up/Down would disagree with what's on screen.
        let content = "test test test test";
        let width = 14;
        // "test test test" exactly fills 14, absorbing the following
        // space onto line 0 rather than word-relocating - see
        // `inserting_a_word_after_a_full_line_does_not_indent_the_continuation`.
        let lines = wrap(content, width);
        let visible = visible_range(content, &lines[1]);
        assert_ne!(
            lines[1].range, visible,
            "sanity check: line 1 must actually have something trimmed"
        );
        assert_eq!(visible_text(content, &lines[1]), "test");

        let mut buffer = CommandLine::default();
        for character in content.chars() {
            buffer.insert_char(character);
        }
        for _ in content.chars() {
            buffer.move_left(); // idempotent once at 0
        }
        assert_eq!(buffer.cursor(), 0, "sanity check: cursor at the very start");

        buffer.move_down(width);
        assert_eq!(buffer.cursor(), visible.start);
    }

    #[test]
    fn move_up_and_down_cross_a_typed_newline_like_any_other_line_boundary() {
        let mut buffer = CommandLine::default();
        for character in "hi\nbye".chars() {
            buffer.insert_char(character);
        }
        let width = 80;

        buffer.move_left();
        buffer.move_left();
        buffer.move_left();
        assert_eq!(buffer.cursor(), 3, "sanity check: start of \"bye\"");

        buffer.move_up(width);
        assert_eq!(buffer.cursor(), 0, "column 0 of \"hi\"");

        buffer.move_down(width);
        assert_eq!(buffer.cursor(), 3, "back to the start of \"bye\"");
    }

    #[test]
    fn resolve_cursor_line_redirects_onto_a_real_next_line_when_the_current_one_is_full() {
        // Mirrors `cursor_display_redirects_to_a_real_next_line_when_the_current_one_is_full`:
        // byte 5 is the boundary right after "hello", which `locate_line` alone
        // would still call part of line 0 (a completely full line). Movement
        // must agree with what actually renders there instead.
        let content = "hello world";
        let lines = wrap(content, 5); // ["hello", " world"]
        assert_eq!(resolve_cursor_line(content, &lines, 5, 5), (1, 0));
    }

    #[test]
    fn resolve_cursor_line_does_not_redirect_past_the_real_last_line() {
        // "hello" alone is a completely full single line - there is no real
        // next line to redirect onto, unlike `cursor_display`'s phantom
        // line (which only exists for rendering, not for movement to land
        // on).
        let content = "hello";
        let lines = wrap(content, 5);
        assert_eq!(resolve_cursor_line(content, &lines, 5, 5), (0, 5));
    }

    #[test]
    fn move_up_from_a_cursor_that_renders_on_the_next_line_moves_relative_to_it() {
        // Motivating bug: "hello world" at width 5 wraps to ["hello", " world"].
        //
        // The space separating them is absorbed onto line 1 but is
        // invisible there (see [`visible_range`]), so a cursor sitting right
        // after "hello" (byte 5, "on" that invisible space) renders, per
        // `cursor_display`, at the start of line 1, right before "world".
        //
        // Pressing Up from there used to silently no-op, because `locate_line`
        // alone still considered byte 5 part of line 0 (the already-first line)
        // even though the cursor appears on line 1.        let content = "hello world";
        let content = "hello world";
        let width = 5;
        let mut buffer = CommandLine::default();
        for character in content.chars() {
            buffer.insert_char(character);
        }
        for _ in content.chars() {
            buffer.move_left(); // back to the very start
        }
        for _ in 0..5 {
            buffer.move_right(); // right after "hello"
        }
        assert_eq!(
            buffer.cursor(),
            5,
            "sanity check: cursor right after \"hello\""
        );

        buffer.move_up(width);
        assert_eq!(
            buffer.cursor(),
            0,
            "should land on column 0 of line 0, matching where the cursor visually was on line 1"
        );
    }

    #[test]
    fn cursor_display_reports_the_cursors_own_line_and_column_normally() {
        let content = "hi";
        let lines = wrap(content, 10);
        let display = cursor_display(content, &lines, content.len(), 10);
        assert_eq!(display.line, 0);
        assert_eq!(display.column, 2);
        assert_eq!(display.total_lines, 1);
    }

    #[test]
    fn cursor_display_reserves_a_fresh_line_when_the_current_line_is_completely_full() {
        let content = "hello";
        let lines = wrap(content, 5); // one line, exactly full
        let display = cursor_display(content, &lines, content.len(), 5);
        assert_eq!(display.line, 1); // one past the last real line
        assert_eq!(display.column, 0);
        assert_eq!(display.total_lines, 2); // box grows to show it
    }

    #[test]
    fn cursor_display_redirects_to_a_real_next_line_when_the_current_one_is_full() {
        // "helloworld" hard-breaks into ["hello", "world"] at width 5; byte 5
        // is the wrap boundary between them, not the true end of the
        // buffer - "hello" is still completely full, so the cursor must
        // move to the start of "world" (an already-real line) rather than
        // rendering on the border at the end of "hello".
        let content = "helloworld";
        let lines = wrap(content, 5);
        let display = cursor_display(content, &lines, 5, 5);
        assert_eq!(display.line, 1);
        assert_eq!(display.column, 0);
        assert_eq!(display.total_lines, 2); // no fresh line needed, "world" already exists
    }

    #[test]
    fn cursor_display_does_not_redirect_a_true_interior_cursor() {
        // Byte 2 is inside "hello", nowhere near a wrap boundary - nothing
        // about this line is full yet.
        let content = "helloworld";
        let lines = wrap(content, 5);
        let display = cursor_display(content, &lines, 2, 5);
        assert_eq!(display.line, 0);
        assert_eq!(display.column, 2);
        assert_eq!(display.total_lines, 2);
    }

    #[test]
    fn cursor_display_redirects_when_moving_back_over_an_absorbed_space() {
        // Motivating bug: "hello" fills width 5 exactly; typing a trailing
        // space wraps it onto its own (trimmed-to-blank) line. Moving the
        // cursor back to right before that space must keep it on the blank
        // line, not put it on the border at the end of "hello".
        let content = "hello ";
        let lines = wrap(content, 5);
        let before_space = lines[0].range.end;
        let display = cursor_display(content, &lines, before_space, 5);
        assert_eq!(display.line, 1);
        assert_eq!(display.column, 0);
        assert_eq!(display.total_lines, 2);
    }

    #[test]
    fn cursor_display_redirects_a_cursor_positioned_right_after_a_typed_newline() {
        // "hi\n" fits width 80 with room to spare, so nothing here is
        // column-full - the redirect has to fire purely because of the
        // newline, mirroring the column-full case above but for a
        // different reason.
        let content = "hi\n";
        let lines = wrap(content, 80);
        let display = cursor_display(content, &lines, content.len(), 80);
        assert_eq!(display.line, 1); // the fresh, empty line after the newline
        assert_eq!(display.column, 0);
        assert_eq!(display.total_lines, 2);
    }

    #[test]
    fn cursor_display_does_not_redirect_a_cursor_positioned_right_before_a_typed_newline() {
        // Byte 2 is right after "hi" but before the newline itself - typing
        // here inserts onto line 0, not line 1, so it must render there.
        let content = "hi\nbye";
        let lines = wrap(content, 80);
        let display = cursor_display(content, &lines, 2, 80);
        assert_eq!(display.line, 0);
        assert_eq!(display.column, 2);
        assert_eq!(display.total_lines, 2);
    }

    #[test]
    fn scroll_keeps_scrolloff_lines_of_margin_in_the_interior() {
        assert_eq!(scroll(10, 20, 8, 2), 8);
    }

    #[test]
    fn scroll_relaxes_the_top_margin_at_the_start_of_the_buffer() {
        assert_eq!(scroll(0, 20, 8, 2), 0);
    }

    #[test]
    fn scroll_relaxes_the_bottom_margin_at_the_end_of_the_buffer() {
        assert_eq!(scroll(19, 20, 8, 2), 12);
    }

    #[test]
    fn scroll_stays_at_zero_when_everything_already_fits() {
        assert_eq!(scroll(3, 5, 8, 2), 0);
    }

    #[test]
    fn scroll_shrinks_the_margin_when_the_box_is_too_short_to_fit_it() {
        // inner_height 1 can't fit any margin (needs >= 2*scrolloff + 1), so
        // it shrinks to 0 and the window tracks the cursor exactly.
        assert_eq!(scroll(5, 20, 1, 2), 5);
    }
}
