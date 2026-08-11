use crate::tui::{Command, Message};

/// Declares which of the two main panels is currently focused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Focus {
    #[default]
    CommandLine,
    Browser,
}

impl Focus {
    fn switch(&mut self) {
        *self = self.other();
    }

    fn other(self) -> Self {
        match self {
            Focus::CommandLine => Focus::Browser,
            Focus::Browser => Focus::CommandLine,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct Model {
    /// The input buffer in the command line.
    pub input: String,
    /// The currently focused panel.
    pub focus: Focus,
}

impl Model {
    /// Update the model based on this message.
    ///
    /// Returns the effect the runtime should carry out, if any. Most messages
    /// only change the model, so [`None`] is the common answer.
    #[allow(
        clippy::match_same_arms,
        reason = "one arm per message while the bodies are still unimplemented; merging them would hide which messages are outstanding"
    )]
    pub fn update(&mut self, message: &Message) -> Option<Command> {
        match message {
            Message::Left => {}
            Message::Right => {}
            Message::Up => {}
            Message::Down => {}
            Message::SwitchFocus => self.focus.switch(),
            Message::WriteCharacter(_) => {}
            Message::DeleteCharacterBefore => {}
            Message::DeleteCharacterOn => {}
            Message::Quit => return Some(Command::Quit),
        }

        None
    }
}
