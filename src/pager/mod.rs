//! A `more`-style pager: displays lines one page at a time with keyboard
//! navigation. The pure state lives in [`state`]; this module does the I/O.

mod guard;
mod state;
mod text;

use crossterm::{
    cursor,
    event::{self, Event, KeyEventKind},
    queue,
    style::Print,
    terminal::{self, BeginSynchronizedUpdate, ClearType, EndSynchronizedUpdate},
};
use std::io::{self, Write};

use crate::style;
use guard::TerminalGuard;
use state::{Frame, Outcome, Pager};

/// Run the pager over `lines` and return the process exit status.
///
/// `rerender` is accepted for resize support; it is not used yet.
pub fn run(
    lines: Vec<String>,
    filename: &str,
    _rerender: Option<&dyn Fn(u16) -> Vec<String>>,
) -> io::Result<i32> {
    let (width, height) = crate::terminal::size();
    let page_height = height.saturating_sub(1) as usize;

    // Terminal too small, or the content fits on one screen: just print it.
    if page_height == 0 || fits_on_screen(&lines, width as usize, page_height) {
        return match print_all(&lines) {
            Err(e) if e.kind() == io::ErrorKind::BrokenPipe => Ok(0),
            Err(e) => Err(e),
            Ok(()) => Ok(0),
        };
    }

    let guard = TerminalGuard::enter()?;
    let result = event_loop(&guard, Pager::new(lines, filename, page_height), width);
    drop(guard);
    result
}

/// Whether `lines` take at most `rows` physical rows at `width` columns.
fn fits_on_screen(lines: &[String], width: usize, rows: usize) -> bool {
    if lines.len() > rows {
        return false;
    }
    let mut used = 0;
    for line in lines {
        used += text::physical_rows(line, width);
        if used > rows {
            return false;
        }
    }
    true
}

fn print_all(lines: &[String]) -> io::Result<()> {
    let stdout = io::stdout();
    let mut out = io::BufWriter::new(stdout.lock());
    for line in lines {
        writeln!(out, "{}", line)?;
    }
    out.flush()
}

fn event_loop(guard: &TerminalGuard, mut pager: Pager, width: u16) -> io::Result<i32> {
    let mut stdout = io::stdout();
    draw(&mut stdout, &pager.frame(width as usize), true)?;

    loop {
        let outcome = match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => pager.handle_key(&key),
            _ => Outcome::Nothing,
        };
        match outcome {
            Outcome::Nothing => {}
            Outcome::Draw => draw(&mut stdout, &pager.frame(width as usize), false)?,
            Outcome::Redraw => draw(&mut stdout, &pager.frame(width as usize), true)?,
            Outcome::Suspend => {
                guard.suspend()?;
                draw(&mut stdout, &pager.frame(width as usize), true)?;
            }
            Outcome::Quit(code) => return Ok(code),
        }
    }
}

/// Write one frame with a single flush. Each row is cleared and rewritten in
/// place (no full-screen clear unless `clear_all`), and every row and the
/// status line start from a reset so styles cannot leak between them.
fn draw(out: &mut impl Write, frame: &Frame, clear_all: bool) -> io::Result<()> {
    let mut buf: Vec<u8> = Vec::with_capacity(16 * 1024);
    queue!(buf, BeginSynchronizedUpdate)?;
    if clear_all {
        queue!(buf, terminal::Clear(ClearType::All))?;
    }
    for (row, line) in frame.rows.iter().enumerate() {
        queue!(
            buf,
            cursor::MoveTo(0, row as u16),
            Print(style::RESET),
            terminal::Clear(ClearType::UntilNewLine),
            Print(line),
            Print(style::RESET),
        )?;
    }
    queue!(
        buf,
        cursor::MoveTo(0, frame.rows.len() as u16),
        Print(style::RESET),
        terminal::Clear(ClearType::UntilNewLine),
        Print(style::REVERSE),
        Print(&frame.status),
        Print(style::RESET),
        EndSynchronizedUpdate,
    )?;
    out.write_all(&buf)?;
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fits_counts_physical_rows() {
        let lines = vec!["a".repeat(15), "b".to_string()];
        assert!(fits_on_screen(&lines, 10, 3));
        assert!(!fits_on_screen(&lines, 10, 2));
        assert!(!fits_on_screen(&lines, 80, 1));
        assert!(fits_on_screen(&[], 80, 1));
    }

    #[test]
    fn draw_resets_around_every_row_and_status() {
        let frame = Frame {
            rows: vec!["\x1b[1mbold".to_string(), "plain".to_string()],
            status: " status ".to_string(),
        };
        let mut out = Vec::new();
        draw(&mut out, &frame, false).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("\x1b[1mbold\x1b[0m"));
        assert!(s.contains("\x1b[0m\x1b[K\x1b[7m status \x1b[0m"));
        assert!(!s.contains("\x1b[2J"));
        let mut out = Vec::new();
        draw(&mut out, &frame, true).unwrap();
        assert!(String::from_utf8(out).unwrap().contains("\x1b[2J"));
    }
}
