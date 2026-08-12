use std::ops::{Deref, DerefMut};

/// Owns the terminal for as long as the management interface runs.
///
/// Restoring the terminal (raw mode, alternate screen) has to happen no
/// matter how `run` exits - including the early returns `?` takes on a
/// drawing error - so it's tied to this guard's [`Drop`] impl rather than
/// called explicitly at the end of `run`.
pub struct TerminalGuard(ratatui::DefaultTerminal);

impl TerminalGuard {
    pub fn new() -> std::io::Result<Self> {
        ratatui::try_init().map(Self)
    }
}

impl Deref for TerminalGuard {
    type Target = ratatui::DefaultTerminal;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for TerminalGuard {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        ratatui::restore();
    }
}
