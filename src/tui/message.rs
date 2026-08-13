/// A message for the [`update`](crate::tui::Model::update) function.
///
/// Messages describe what happened in terms the application understands, not
/// which key produced them. Deciding what a message does is
/// [`Model::update`](crate::tui::Model::update)'s job, and the answer may well
/// be nothing.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Message {
    /// A movement in the left direction.
    Left,
    /// A movement in the right direction.
    Right,
    /// A movement in the up direction.
    Up,
    /// A movement in the down direction.
    Down,
    /// A request to focus the other main panel.
    SwitchFocus,
    /// Write a character at the cursor position in the command line.
    WriteCharacter(char),
    /// Delete the character before the cursor,
    DeleteCharacterBefore,
    /// Delete the character on the cursor,
    DeleteCharacterOn,
    /// Move to the start of the current logical line.
    MoveToLineStart,
    /// Move to the end of the current logical line.
    MoveToLineEnd,
    /// Move to the start of the previous word.
    MoveWordLeft,
    /// Move to the end of the next word.
    MoveWordRight,
    /// Delete the word before the cursor.
    DeleteWordBefore,
    /// Delete the word after the cursor.
    DeleteWordAfter,
    /// Delete from the start of the current logical line up to the cursor.
    DeleteToLineStart,
    /// Delete from the cursor up to the end of the current logical line.
    DeleteToLineEnd,
    /// A request to exit the application.
    Quit,
    /// Ctrl+d: forward-delete like the Delete key, or exit if the command
    /// line is already empty (traditional EOF behavior) - the same dual
    /// meaning Ctrl+d has in most shells.
    CommandLineEOF,
    /// The terminal was resized to (width, height).
    Resize(u16, u16),
}
