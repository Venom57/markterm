//! Terminal capability detection and dimension queries.

use crossterm::terminal;

/// Returns (width, height) of the terminal, with sensible defaults.
pub fn size() -> (u16, u16) {
    terminal::size().unwrap_or((80, 24))
}
