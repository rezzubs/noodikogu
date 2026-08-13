mod command_line;

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Size},
    style::Color,
    widgets::{Block, BorderType},
};

use crate::tui::{Command, Message};
use command_line::CommandLine;

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
    /// The command line's text buffer and cursor.
    pub command_line: CommandLine,
    /// The currently focused panel.
    pub focus: Focus,
    /// The terminal's current size, kept in sync via [`Message::Resize`].
    ///
    /// `update` needs this to compute the command line's wrap width for
    /// vertical cursor movement (`Message::Up`/`Down`) - unlike `view`, it
    /// has no other access to layout information. `App::run` seeds this
    /// once at startup.
    pub terminal_size: Size,
}

impl Model {
    /// Update the model based on this message.
    ///
    /// Returns the effect the runtime should carry out, if any. Most messages
    /// only change the model, so [`None`] is the common answer.
    pub fn update(&mut self, message: &Message) -> Option<Command> {
        match message {
            Message::Left => {
                if self.focus == Focus::CommandLine {
                    self.command_line.move_left();
                }
            }
            Message::Right => {
                if self.focus == Focus::CommandLine {
                    self.command_line.move_right();
                }
            }
            Message::Up => {
                if self.focus == Focus::CommandLine {
                    self.command_line
                        .move_up(command_line_inner_width(self.terminal_size));
                }
            }
            Message::Down => {
                if self.focus == Focus::CommandLine {
                    self.command_line
                        .move_down(command_line_inner_width(self.terminal_size));
                }
            }
            Message::SwitchFocus => self.focus.switch(),
            Message::WriteCharacter(character) => self.command_line.insert_char(*character),
            Message::DeleteCharacterBefore => self.command_line.delete_before(),
            Message::DeleteCharacterOn => self.command_line.delete_on(),
            Message::Quit => return Some(Command::Quit),
            Message::CommandLineEOF => {
                if self.command_line.is_empty() {
                    return Some(Command::Quit);
                }
            }
            Message::Resize(width, height) => {
                self.terminal_size = Size::new(*width, *height);
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

        let inner_width = command_line_inner_width(self.terminal_size);

        let outer_height_max = area.height / 2;
        // sub 2 for top and bottom borders.
        let inner_height_max = usize::from(outer_height_max.saturating_sub(2));
        let desired_inner_height = self
            .command_line
            .desired_height(inner_width)
            .min(inner_height_max);
        let outer_height =
            u16::try_from(desired_inner_height.saturating_add(2)).unwrap_or(u16::MAX);

        // The contained number in Fill has no meaning if only a single one
        // is used.
        let [browser_area, command_line_area] =
            Layout::vertical([Constraint::Fill(0), Constraint::Length(outer_height)]).areas(area);

        let command_line_view = self.command_line.view(
            inner_width,
            command_line_area,
            self.focus == Focus::CommandLine,
        );

        frame.render_widget(browser, browser_area);
        frame.render_widget(command_line_view.widget, command_line_area);

        if let Some(cursor_position) = command_line_view.cursor_position {
            frame.set_cursor_position(cursor_position);
        }
    }
}

/// Terminal columns available for command-line text, after its left/right
/// border.
///
/// Shared by `update` (vertical cursor movement) and `view` (rendering) so
/// the two can never silently disagree about wrap width - both derive it
/// from the same `terminal_size`, which is why `view` reads
/// `self.terminal_size` here rather than `frame.area().width` directly
/// (the two are numerically identical in practice, since only a vertical
/// split separates the browser and command line, but funneling both call
/// sites through one function removes a class of future divergence bugs).
fn command_line_inner_width(terminal_size: Size) -> u16 {
    // Subtracting 2 to account for block border on both sides.
    terminal_size.width.saturating_sub(2)
}
