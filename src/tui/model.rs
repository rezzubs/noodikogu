use ratatui::{
    Frame,
    layout::{Constraint, Layout},
    style::Color,
    widgets::{Block, BorderType, Paragraph, Wrap},
};

use crate::tui::{Command, Message};

const NORMAL_COLOR: Color = Color::Reset;
const SELECTION_COLOR: Color = Color::Green;

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
            Message::WriteCharacter(char) => {
                self.input.push(*char);
            }
            Message::DeleteCharacterBefore => {
                self.input.pop();
            }
            Message::DeleteCharacterOn => {}
            Message::Quit => return Some(Command::Quit),
            Message::CommandLineEOF => {
                if self.input.is_empty() {
                    return Some(Command::Quit);
                }
            }
        }

        None
    }

    pub fn view(&self, frame: &mut Frame) {
        let area = frame.area();

        let browser = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(match self.focus {
                Focus::CommandLine => NORMAL_COLOR,
                Focus::Browser => SELECTION_COLOR,
            });

        let command_line = Paragraph::new(self.input.as_str())
            .wrap(Wrap { trim: false })
            .block(
                Block::bordered()
                    .border_type(BorderType::Rounded)
                    .border_style(match self.focus {
                        Focus::CommandLine => SELECTION_COLOR,
                        Focus::Browser => NORMAL_COLOR,
                    })
                    .title("Search"),
            );

        // `line_count` measures against the width available for text, which
        // is narrower than the block's own width by one column of border on
        // each side.
        let command_line_height_max = area.height / 2;
        let command_line_height_desired = command_line.line_count(area.width.saturating_sub(2));
        let command_line_height = u16::try_from(command_line_height_desired)
            .unwrap_or(u16::MAX)
            .min(command_line_height_max);

        let [browser_area, command_line_area] =
            Layout::vertical([Constraint::Fill(1), Constraint::Length(command_line_height)])
                .areas(area);

        frame.render_widget(browser, browser_area);
        frame.render_widget(command_line, command_line_area);
    }
}
