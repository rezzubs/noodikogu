use crate::tui::{
    message::Message,
    model::{Focus, Model},
};
use ratatui::crossterm;

/// Maps a crossterm key event to a message.
pub fn keymap(model: &Model, event: &crossterm::event::KeyEvent) -> Option<Message> {
    use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};

    // Repeat and Release only arrive under the kitty keyboard protocol; other
    // terminals report a held key as ordinary repeated presses. A repeat is the
    // same activation happening again, so only releases are discarded.
    if event.kind == KeyEventKind::Release {
        return None;
    }

    if let Some(focus_message) = match model.focus {
        Focus::Browser => browser(event),
        Focus::CommandLine => command_line(event),
    } {
        return Some(focus_message);
    }

    match event.modifiers {
        KeyModifiers::CONTROL => match event.code {
            KeyCode::Char('c') => Some(Message::Quit),
            KeyCode::Char('b') => Some(Message::Left),
            KeyCode::Char('f') => Some(Message::Right),
            KeyCode::Char('p') => Some(Message::Up),
            KeyCode::Char('n') => Some(Message::Down),
            _ => None,
        },
        KeyModifiers::NONE => match event.code {
            KeyCode::Tab => Some(Message::SwitchFocus),
            KeyCode::Left => Some(Message::Left),
            KeyCode::Right => Some(Message::Right),
            KeyCode::Up => Some(Message::Up),
            KeyCode::Down => Some(Message::Down),
            _ => None,
        },
        _ => None,
    }
}

/// Bindings which apply while the browser is focused.
fn browser(event: &crossterm::event::KeyEvent) -> Option<Message> {
    use crossterm::event::{KeyCode, KeyModifiers};

    if event.modifiers != KeyModifiers::NONE {
        return None;
    }

    match event.code {
        KeyCode::Char('h') => Some(Message::Left),
        KeyCode::Char('j') => Some(Message::Down),
        KeyCode::Char('k') => Some(Message::Up),
        KeyCode::Char('l') => Some(Message::Right),
        _ => None,
    }
}

/// Bindings which apply while the command line is focused.
///
/// Vi-style movement is deliberately absent: the command line isn't modal, so
/// those keys have to stay available as text.
fn command_line(event: &crossterm::event::KeyEvent) -> Option<Message> {
    use crossterm::event::{KeyCode, KeyModifiers};

    match event.code {
        // Shift accompanies uppercase characters, which are already cased
        // correctly in the code, so both count as literal input. Any other
        // modifier marks a command rather than text.
        KeyCode::Char(character)
            if matches!(event.modifiers, KeyModifiers::NONE | KeyModifiers::SHIFT) =>
        {
            Some(Message::WriteCharacter(character))
        }
        KeyCode::Backspace if event.modifiers == KeyModifiers::NONE => {
            Some(Message::DeleteCharacterBefore)
        }
        KeyCode::Delete if event.modifiers == KeyModifiers::NONE => {
            Some(Message::DeleteCharacterOn)
        }
        _ => None,
    }
}

/// Bindings which apply when no more specific context shadows it.
fn global(event: &crossterm::event::KeyEvent) -> Option<Message> {
    use crossterm::event::{KeyCode, KeyModifiers};

    match event.modifiers {
        KeyModifiers::CONTROL => match event.code {
            KeyCode::Char('c') => Some(Message::Quit),
            KeyCode::Char('b') => Some(Message::Left),
            KeyCode::Char('f') => Some(Message::Right),
            KeyCode::Char('p') => Some(Message::Up),
            KeyCode::Char('n') => Some(Message::Down),
            _ => None,
        },
        KeyModifiers::NONE => match event.code {
            KeyCode::Tab => Some(Message::SwitchFocus),
            KeyCode::Left => Some(Message::Left),
            KeyCode::Right => Some(Message::Right),
            KeyCode::Up => Some(Message::Up),
            KeyCode::Down => Some(Message::Down),
            _ => None,
        },
        _ => None,
    }
}
