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
use std::time::Duration;

use crate::style;
use guard::TerminalGuard;
use state::{Frame, Outcome, Pager};

/// Run the pager over `lines` and return the process exit status.
///
/// `rerender` re-renders the document at a new width; it is called after a
/// terminal resize changes the width. Pass `None` when the width is fixed
/// (e.g. `-w`), in which case a resize only changes the page height.
pub fn run(
    lines: Vec<String>,
    filename: &str,
    rerender: Option<&dyn Fn(u16) -> Vec<String>>,
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
    let mut session = Session {
        pager: Pager::new(lines, filename, page_height),
        width,
        rendered_width: width,
        rerender,
    };
    let result = session.event_loop(&guard);
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

/// The interactive pager: pure state plus the cached terminal width.
struct Session<'a> {
    pager: Pager,
    width: u16,
    /// Width the current lines were rendered at.
    rendered_width: u16,
    rerender: Option<&'a dyn Fn(u16) -> Vec<String>>,
}

impl Session<'_> {
    fn event_loop(&mut self, guard: &TerminalGuard) -> io::Result<i32> {
        let mut stdout = io::stdout();
        self.draw(&mut stdout, true)?;

        let mut pending: Option<Event> = None;
        loop {
            let ev = match pending.take() {
                Some(ev) => ev,
                None => event::read()?,
            };
            let outcome = match ev {
                Event::Key(key) if key.kind == KeyEventKind::Press => self.pager.handle_key(&key),
                Event::Resize(mut w, mut h) => {
                    // Coalesce a burst of resize events into one re-render.
                    while event::poll(Duration::ZERO)? {
                        match event::read()? {
                            Event::Resize(w2, h2) => (w, h) = (w2, h2),
                            other => {
                                pending = Some(other);
                                break;
                            }
                        }
                    }
                    self.resize(w, h);
                    Outcome::Redraw
                }
                _ => Outcome::Nothing,
            };
            match outcome {
                Outcome::Nothing => {}
                Outcome::Draw => self.draw(&mut stdout, false)?,
                Outcome::Redraw => self.draw(&mut stdout, true)?,
                Outcome::Suspend => {
                    guard.suspend()?;
                    // The terminal may have been resized while we were stopped.
                    let (w, h) = crate::terminal::size();
                    self.resize(w, h);
                    self.draw(&mut stdout, true)?;
                }
                Outcome::Quit(code) => return Ok(code),
            }
        }
    }

    fn resize(&mut self, width: u16, height: u16) {
        self.width = width;
        let lines = match self.rerender {
            Some(render) if width != self.rendered_width => {
                self.rendered_width = width;
                Some(render(width))
            }
            _ => None,
        };
        let page_height = height.saturating_sub(1) as usize;
        self.pager.resize(page_height, lines);
    }

    fn draw(&self, out: &mut impl Write, clear_all: bool) -> io::Result<()> {
        draw(out, &self.pager.frame(self.width as usize), clear_all)
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
