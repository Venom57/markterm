//! A `more`-style pager: displays lines one page at a time with keyboard navigation.

use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{self, ClearType},
};
use std::io::{self, Write};

use crate::style;

/// Run the pager over `lines`. Returns the process exit code.
///
/// `rerender` is accepted for future resize support; it is not used yet.
pub fn run(
    lines: Vec<String>,
    filename: &str,
    _rerender: Option<&dyn Fn(u16) -> Vec<String>>,
) -> io::Result<i32> {
    let lines = &lines[..];
    let (_, term_height) = crate::terminal::size();
    let mut stdout = io::stdout();

    // Page height: terminal height minus 1 for the status line
    let page_height = (term_height.saturating_sub(1)) as usize;
    let total_lines = lines.len();

    // Terminal too small, or content fits on one screen: just print it
    if page_height == 0 || total_lines <= page_height {
        let result = print_all(lines);
        return match result {
            Err(e) if e.kind() == io::ErrorKind::BrokenPipe => Ok(0),
            Err(e) => Err(e),
            Ok(()) => Ok(0),
        };
    }

    // Enter raw mode for interactive paging
    terminal::enable_raw_mode()?;
    // Ensure we restore terminal on panic
    let result = run_pager_loop(&mut stdout, lines, page_height, total_lines, filename);
    terminal::disable_raw_mode()?;
    // Move to a new line after the status bar
    execute!(stdout, cursor::MoveToColumn(0))?;
    writeln!(stdout)?;

    result.map(|()| 0)
}

fn print_all(lines: &[String]) -> io::Result<()> {
    let stdout = io::stdout();
    let mut out = io::BufWriter::new(stdout.lock());
    for line in lines {
        writeln!(out, "{}", line)?;
    }
    out.flush()
}

fn run_pager_loop(
    stdout: &mut io::Stdout,
    lines: &[String],
    page_height: usize,
    total_lines: usize,
    filename: &str,
) -> io::Result<()> {
    let mut offset: usize = 0;

    // Initial draw
    draw_page(stdout, lines, offset, page_height, total_lines, filename)?;

    loop {
        if let Event::Key(key) = event::read()? {
            match key {
                // Quit
                KeyEvent {
                    code: KeyCode::Char('q'),
                    ..
                }
                | KeyEvent {
                    code: KeyCode::Esc, ..
                } => break,

                // Ctrl-C
                KeyEvent {
                    code: KeyCode::Char('c'),
                    modifiers: KeyModifiers::CONTROL,
                    ..
                } => break,

                // Next page (space, Page Down)
                KeyEvent {
                    code: KeyCode::Char(' '),
                    ..
                }
                | KeyEvent {
                    code: KeyCode::PageDown,
                    ..
                }
                | KeyEvent {
                    code: KeyCode::Char('f'),
                    modifiers: KeyModifiers::CONTROL,
                    ..
                } => {
                    let max_offset = total_lines.saturating_sub(page_height);
                    offset = (offset + page_height).min(max_offset);
                    draw_page(stdout, lines, offset, page_height, total_lines, filename)?;
                }

                // Previous page (b, Page Up)
                KeyEvent {
                    code: KeyCode::Char('b'),
                    ..
                }
                | KeyEvent {
                    code: KeyCode::PageUp,
                    ..
                } => {
                    offset = offset.saturating_sub(page_height);
                    draw_page(stdout, lines, offset, page_height, total_lines, filename)?;
                }

                // Next line (Enter, Down, j)
                KeyEvent {
                    code: KeyCode::Enter,
                    ..
                }
                | KeyEvent {
                    code: KeyCode::Down,
                    ..
                }
                | KeyEvent {
                    code: KeyCode::Char('j'),
                    ..
                } => {
                    let max_offset = total_lines.saturating_sub(page_height);
                    if offset < max_offset {
                        offset += 1;
                        draw_page(stdout, lines, offset, page_height, total_lines, filename)?;
                    }
                }

                // Previous line (Up, k)
                KeyEvent {
                    code: KeyCode::Up, ..
                }
                | KeyEvent {
                    code: KeyCode::Char('k'),
                    ..
                } => {
                    if offset > 0 {
                        offset -= 1;
                        draw_page(stdout, lines, offset, page_height, total_lines, filename)?;
                    }
                }

                // Go to top (g, Home)
                KeyEvent {
                    code: KeyCode::Char('g'),
                    ..
                }
                | KeyEvent {
                    code: KeyCode::Home,
                    ..
                } => {
                    offset = 0;
                    draw_page(stdout, lines, offset, page_height, total_lines, filename)?;
                }

                // Go to bottom (G, End)
                KeyEvent {
                    code: KeyCode::Char('G'),
                    ..
                }
                | KeyEvent {
                    code: KeyCode::End, ..
                } => {
                    offset = total_lines.saturating_sub(page_height);
                    draw_page(stdout, lines, offset, page_height, total_lines, filename)?;
                }

                // Half page down (d, Ctrl-d)
                KeyEvent {
                    code: KeyCode::Char('d'),
                    ..
                } => {
                    let max_offset = total_lines.saturating_sub(page_height);
                    offset = (offset + page_height / 2).min(max_offset);
                    draw_page(stdout, lines, offset, page_height, total_lines, filename)?;
                }

                // Half page up (u, Ctrl-u)
                KeyEvent {
                    code: KeyCode::Char('u'),
                    ..
                } => {
                    offset = offset.saturating_sub(page_height / 2);
                    draw_page(stdout, lines, offset, page_height, total_lines, filename)?;
                }

                _ => {}
            }
        }
    }

    Ok(())
}

fn draw_page(
    stdout: &mut io::Stdout,
    lines: &[String],
    offset: usize,
    page_height: usize,
    total_lines: usize,
    filename: &str,
) -> io::Result<()> {
    // Move cursor to top-left and clear screen
    execute!(
        stdout,
        cursor::MoveTo(0, 0),
        terminal::Clear(ClearType::All)
    )?;

    // Display lines for this page
    let end = (offset + page_height).min(total_lines);
    for line in &lines[offset..end] {
        writeln!(stdout, "{}\r", line)?;
    }

    // Pad remaining lines if page is not full
    for _ in (end - offset)..page_height {
        writeln!(stdout, "~\r")?;
    }

    // Status line
    let percentage = if total_lines == 0 {
        100
    } else {
        ((end as f64 / total_lines as f64) * 100.0) as usize
    };

    let status = format!(
        " {} | lines {}-{} of {} ({}%) ",
        filename,
        offset + 1,
        end,
        total_lines,
        percentage
    );

    let help = " [Space] next  [b] back  [q] quit ";

    // Get terminal width for padding
    let (term_width, _) = terminal::size().unwrap_or((80, 24));
    let status_len = style::visible_len(&status) + style::visible_len(help);
    let padding = if (term_width as usize) > status_len {
        " ".repeat(term_width as usize - status_len)
    } else {
        String::new()
    };

    write!(
        stdout,
        "{}{}{}{}{}",
        style::REVERSE,
        status,
        padding,
        help,
        style::RESET
    )?;

    stdout.flush()?;
    Ok(())
}
