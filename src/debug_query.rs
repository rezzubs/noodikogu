use ratatui::Frame;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

/// All mutable state for the debug-query TUI session.
#[derive(Debug, Clone)]
struct App {
    /// Text currently in the input field.
    input: String,
    /// Cursor position within `input` (byte offset, always on a char boundary).
    cursor: usize,
    /// Vertical scroll offset for the output pane (in rendered lines).
    scroll: u16,
    /// Visible line count of the output pane (excluding borders), updated each draw.
    output_height: u16,
    /// Whether the user has requested to quit.
    should_quit: bool,
}

impl App {
    /// Creates a new [`App`] with empty input.
    fn new() -> Self {
        Self {
            input: String::new(),
            cursor: 0,
            scroll: 0,
            output_height: 0,
            should_quit: false,
        }
    }

    fn run(
        &mut self,
        terminal: &mut ratatui::DefaultTerminal,
    ) -> Result<(), Box<dyn std::error::Error>> {
        loop {
            terminal.draw(|frame| self.draw(frame))?;

            if let Event::Key(key) = event::read()? {
                self.handle_key(key);
            }

            if self.should_quit {
                break;
            }
        }
        Ok(())
    }

    /// Returns the maximum sensible scroll offset based on content and pane height.
    fn max_scroll(&self) -> u16 {
        let line_count = parse_output(&self.input).lines().count() as u16;
        line_count.saturating_sub(self.output_height)
    }

    fn draw(&mut self, frame: &mut Frame) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(3)])
            .split(frame.area());

        // Update output height (inner area, excluding borders) for scroll clamping.
        self.output_height = chunks[0].height.saturating_sub(2);

        let output = Paragraph::new(parse_output(&self.input))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Parse output "),
            )
            .wrap(Wrap { trim: false })
            .scroll((self.scroll, 0));
        frame.render_widget(output, chunks[0]);

        let input_widget = Paragraph::new(self.input.as_str())
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Query input "),
            )
            .style(Style::default().fg(Color::Yellow));
        frame.render_widget(input_widget, chunks[1]);

        // Place cursor at the cursor byte offset, inside the border.
        let cursor_x = chunks[1].x + 1 + self.cursor as u16;
        let cursor_y = chunks[1].y + 1;
        frame.set_cursor_position((cursor_x, cursor_y));
    }

    fn handle_key(&mut self, key: KeyEvent) {
        if key.kind != KeyEventKind::Press {
            return;
        }

        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        match key.code {
            KeyCode::Esc => self.should_quit = true,
            KeyCode::Char('c') if ctrl => self.should_quit = true,

            // Output pane scrolling
            KeyCode::Up | KeyCode::Char('y') if ctrl => {
                self.scroll = self.scroll.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('e') if ctrl => {
                self.scroll = self.scroll.saturating_add(1).min(self.max_scroll());
            }
            KeyCode::PageUp | KeyCode::Char('u') if ctrl => {
                self.scroll = self.scroll.saturating_sub(10);
            }
            KeyCode::PageDown | KeyCode::Char('d') if ctrl => {
                self.scroll = self.scroll.saturating_add(10).min(self.max_scroll());
            }
            KeyCode::Up => self.scroll = self.scroll.saturating_sub(1),
            KeyCode::Down => self.scroll = self.scroll.saturating_add(1).min(self.max_scroll()),
            KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(10),
            KeyCode::PageDown => {
                self.scroll = self.scroll.saturating_add(10).min(self.max_scroll());
            }

            // Cursor movement
            KeyCode::Left | KeyCode::Char('b') if ctrl => {
                self.cursor = prev_char_boundary(&self.input, self.cursor);
            }
            KeyCode::Right | KeyCode::Char('f') if ctrl => {
                self.cursor = next_char_boundary(&self.input, self.cursor);
            }
            KeyCode::Left => self.cursor = prev_char_boundary(&self.input, self.cursor),
            KeyCode::Right => self.cursor = next_char_boundary(&self.input, self.cursor),

            KeyCode::Char(c) if !ctrl => {
                self.input.insert(self.cursor, c);
                self.cursor += c.len_utf8();
                self.scroll = self.scroll.min(self.max_scroll());
            }
            KeyCode::Backspace => {
                if self.cursor > 0 {
                    let prev = prev_char_boundary(&self.input, self.cursor);
                    self.input.drain(prev..self.cursor);
                    self.cursor = prev;
                    self.scroll = self.scroll.min(self.max_scroll());
                }
            }

            _ => {}
        }
    }
}

/// Returns the byte offset of the character before `pos` in `s`.
fn prev_char_boundary(s: &str, pos: usize) -> usize {
    let mut i = pos.saturating_sub(1);
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Returns the byte offset of the character after `pos` in `s`.
fn next_char_boundary(s: &str, pos: usize) -> usize {
    if pos >= s.len() {
        return s.len();
    }
    let mut i = pos + 1;
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

/// Formats the parse result: Display error (if any) followed by the debug print.
fn parse_output(input: &str) -> String {
    match noodikogu::query::Query::parse(input, 0) {
        Ok(query) => format!("{query:#?}"),
        Err(err) => format!("{err}\n\n{err:#?}"),
    }
}

/// Runs the interactive debug-query TUI. Enters an alternate screen and returns
/// when the user quits with Esc or Ctrl+C.
pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut terminal = ratatui::init();
    let mut app = App::new();
    let result = app.run(&mut terminal);
    ratatui::restore();
    result
}
