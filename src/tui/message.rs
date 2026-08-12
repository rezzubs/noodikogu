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
    /// A request to exit the application.
    Quit,
    /// Ctrl+d on an empty command line will be used to exit.
    CommandLineEOF,
}
