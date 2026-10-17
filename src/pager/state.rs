//! Pure pager state: key → command mapping, scrolling math and frame
//! composition. Nothing in here touches the terminal, so it is unit-testable.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::text;

/// A pager command, independent of which key produced it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Quit,
    /// Ctrl-C: quit with exit status 130.
    Interrupt,
    /// Ctrl-Z: suspend to the shell.
    Suspend,
    /// Ctrl-L: repaint the whole screen.
    Redraw,
    LineDown,
    LineUp,
    PageDown,
    PageUp,
    HalfPageDown,
    HalfPageUp,
    /// Top of the document, or line N with a count.
    Top,
    /// Bottom of the document, or line N with a count.
    Bottom,
    /// Jump to N percent (top without a count).
    Percent,
    /// A digit of a numeric count prefix.
    Digit(u8),
}

/// Map a key event to a command. Plain keys accept no modifiers other than
/// Shift; Ctrl bindings are matched explicitly.
pub fn map_key(key: &KeyEvent) -> Option<Command> {
    use Command::*;

    if key.modifiers == KeyModifiers::CONTROL {
        return match key.code {
            KeyCode::Char('c') => Some(Interrupt),
            KeyCode::Char('z') => Some(Suspend),
            KeyCode::Char('l') => Some(Redraw),
            KeyCode::Char('f' | 'v') => Some(PageDown),
            KeyCode::Char('b') => Some(PageUp),
            KeyCode::Char('d') => Some(HalfPageDown),
            KeyCode::Char('u') => Some(HalfPageUp),
            KeyCode::Char('e' | 'n' | 'j') => Some(LineDown),
            KeyCode::Char('y' | 'p' | 'k') => Some(LineUp),
            _ => None,
        };
    }
    if !key.modifiers.difference(KeyModifiers::SHIFT).is_empty() {
        return None;
    }
    let cmd = match key.code {
        KeyCode::Char('q' | 'Q') | KeyCode::Esc => Quit,
        KeyCode::Char(' ' | 'f' | 'z') | KeyCode::PageDown => PageDown,
        KeyCode::Char('b' | 'w') | KeyCode::PageUp => PageUp,
        KeyCode::Enter | KeyCode::Down | KeyCode::Char('j' | 'e') => LineDown,
        KeyCode::Up | KeyCode::Char('k' | 'y') => LineUp,
        KeyCode::Char('d') => HalfPageDown,
        KeyCode::Char('u') => HalfPageUp,
        KeyCode::Char('g' | '<') | KeyCode::Home => Top,
        KeyCode::Char('G' | '>') | KeyCode::End => Bottom,
        KeyCode::Char('%' | 'p') => Percent,
        KeyCode::Char('r') => Redraw,
        KeyCode::Char(c @ '0'..='9') => Digit(c as u8 - b'0'),
        _ => return None,
    };
    Some(cmd)
}

/// What the event loop should do after a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Nothing visible changed.
    Nothing,
    /// Redraw the frame.
    Draw,
    /// Clear the screen and redraw everything.
    Redraw,
    /// Quit with this exit status.
    Quit(i32),
    /// Suspend (Ctrl-Z), then redraw on resume.
    Suspend,
}

/// One screenful, ready to print.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    /// Exactly `page_height` rows, each clipped to the terminal width.
    pub rows: Vec<String>,
    /// Plain status text, padded/clipped to exactly the terminal width.
    pub status: String,
}

const HINT_FULL: &str = "[Space] next  [b] back  [q] quit";
const HINT_SHORT: &str = "[q] quit";

/// The pager's state.
pub struct Pager {
    lines: Vec<String>,
    filename: String,
    offset: usize,
    page_height: usize,
    count: Option<usize>,
}

impl Pager {
    pub fn new(lines: Vec<String>, filename: &str, page_height: usize) -> Self {
        Pager {
            lines,
            filename: filename.to_string(),
            offset: 0,
            page_height: page_height.max(1),
            count: None,
        }
    }

    #[cfg(test)]
    pub fn offset(&self) -> usize {
        self.offset
    }

    pub fn total(&self) -> usize {
        self.lines.len()
    }

    pub fn max_offset(&self) -> usize {
        self.total().saturating_sub(self.page_height)
    }

    pub fn at_end(&self) -> bool {
        self.offset >= self.max_offset()
    }

    /// Adapt to a new page height and, optionally, freshly rendered lines
    /// (after a width change). The top line keeps its relative position in
    /// the document; a view at the end stays at the end.
    pub fn resize(&mut self, page_height: usize, lines: Option<Vec<String>>) {
        let was_at_end = self.offset > 0 && self.at_end();
        if let Some(lines) = lines {
            let old_total = self.total();
            if old_total > 0 {
                let scaled = self.offset as u128 * lines.len() as u128 / old_total as u128;
                self.offset = usize::try_from(scaled).unwrap_or(usize::MAX);
            }
            self.lines = lines;
        }
        self.page_height = page_height.max(1);
        self.offset = if was_at_end {
            self.max_offset()
        } else {
            self.offset.min(self.max_offset())
        };
    }

    /// Move the top line to `offset` (clamped). Returns whether it changed.
    fn scroll_to(&mut self, offset: usize) -> bool {
        let new = offset.min(self.max_offset());
        let changed = new != self.offset;
        self.offset = new;
        changed
    }

    fn scroll_by(&mut self, delta: isize) -> bool {
        let target = if delta < 0 {
            self.offset.saturating_sub(delta.unsigned_abs())
        } else {
            self.offset.saturating_add(delta as usize)
        };
        self.scroll_to(target)
    }

    pub fn handle_key(&mut self, key: &KeyEvent) -> Outcome {
        match map_key(key) {
            Some(cmd) => self.apply(cmd),
            None => {
                self.count = None;
                Outcome::Nothing
            }
        }
    }

    /// Apply a command, consuming any pending count prefix.
    pub fn apply(&mut self, cmd: Command) -> Outcome {
        if let Command::Digit(d) = cmd {
            let n = self.count.unwrap_or(0);
            self.count = Some(n.saturating_mul(10).saturating_add(d as usize));
            return Outcome::Nothing;
        }
        let count = self.count.take();
        let times = count.unwrap_or(1).max(1);
        let times_i = isize::try_from(times).unwrap_or(isize::MAX);
        let page = isize::try_from(self.page_height).unwrap_or(isize::MAX);
        let half = (page / 2).max(1);

        let moved = match cmd {
            Command::Quit => return Outcome::Quit(0),
            Command::Interrupt => return Outcome::Quit(130),
            Command::Suspend => return Outcome::Suspend,
            Command::Redraw => return Outcome::Redraw,
            Command::Digit(_) => unreachable!("handled above"),
            Command::LineDown => self.scroll_by(times_i),
            Command::LineUp => self.scroll_by(-times_i),
            Command::PageDown => self.scroll_by(page.saturating_mul(times_i)),
            Command::PageUp => self.scroll_by(-page.saturating_mul(times_i)),
            Command::HalfPageDown => self.scroll_by(half.saturating_mul(times_i)),
            Command::HalfPageUp => self.scroll_by(-half.saturating_mul(times_i)),
            Command::Top => self.scroll_to(count.map_or(0, |n| n.saturating_sub(1))),
            Command::Bottom => match count {
                Some(n) => self.scroll_to(n.saturating_sub(1)),
                None => self.scroll_to(usize::MAX),
            },
            Command::Percent => {
                let pct = count.unwrap_or(0).min(100);
                self.scroll_to(self.total() * pct / 100)
            }
        };
        if moved {
            Outcome::Draw
        } else {
            Outcome::Nothing
        }
    }

    /// Compose the screen for a terminal `width` columns wide.
    pub fn frame(&self, width: usize) -> Frame {
        let mut rows = Vec::with_capacity(self.page_height);
        for i in 0..self.page_height {
            match self.lines.get(self.offset + i) {
                Some(line) => rows.push(text::clip(line, width)),
                None => rows.push(text::clip("~", width)),
            }
        }
        Frame {
            rows,
            status: self.status_line(width),
        }
    }

    fn position_text(&self) -> String {
        let total = self.total();
        if total == 0 {
            return "(empty)".to_string();
        }
        let first = self.offset + 1;
        let last = (self.offset + self.page_height).min(total);
        if self.at_end() {
            format!("lines {first}-{last} of {total} (END)")
        } else {
            format!("lines {first}-{last} of {total} ({}%)", last * 100 / total)
        }
    }

    fn status_line(&self, width: usize) -> String {
        status_line(&self.filename, &self.position_text(), width)
    }
}

/// Lay out ` name | position ` on the left and a key hint on the right,
/// exactly `width` columns wide. The hint is shortened or dropped first,
/// then the name is elided from the left.
pub fn status_line(name: &str, position: &str, width: usize) -> String {
    let pos_part = format!(" | {position} ");
    let name_w = text::display_width(name);
    for hint in [HINT_FULL, HINT_SHORT, ""] {
        let right = if hint.is_empty() {
            String::new()
        } else {
            format!(" {hint} ")
        };
        let fixed = 1 + text::display_width(&pos_part) + text::display_width(&right);
        if fixed + name_w.min(8) <= width {
            let name = text::elide_left(name, width - fixed);
            let left = format!(" {name}{pos_part}");
            let pad = width - text::display_width(&left) - text::display_width(&right);
            return format!("{left}{}{right}", " ".repeat(pad));
        }
    }
    pad_to(&format!(" {position} "), width)
}

/// Clip or pad plain text to exactly `width` columns.
pub fn pad_to(s: &str, width: usize) -> String {
    let mut out = text::clip(s, width);
    let w = text::display_width(&out);
    out.extend(std::iter::repeat_n(' ', width - w));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEvent;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ch(c: char) -> KeyEvent {
        key(KeyCode::Char(c))
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn pager(n: usize, height: usize) -> Pager {
        let lines = (1..=n).map(|i| format!("line {i}")).collect();
        Pager::new(lines, "doc.md", height)
    }

    fn press(p: &mut Pager, keys: &[KeyEvent]) -> Outcome {
        let mut last = Outcome::Nothing;
        for k in keys {
            last = p.handle_key(k);
        }
        last
    }

    #[test]
    fn mapping_basic_keys() {
        assert_eq!(map_key(&ch('q')), Some(Command::Quit));
        assert_eq!(map_key(&key(KeyCode::Esc)), Some(Command::Quit));
        assert_eq!(map_key(&ch(' ')), Some(Command::PageDown));
        assert_eq!(map_key(&ch('f')), Some(Command::PageDown));
        assert_eq!(map_key(&ch('z')), Some(Command::PageDown));
        assert_eq!(map_key(&ch('w')), Some(Command::PageUp));
        assert_eq!(map_key(&ch('y')), Some(Command::LineUp));
        assert_eq!(map_key(&ch('e')), Some(Command::LineDown));
        assert_eq!(map_key(&key(KeyCode::Enter)), Some(Command::LineDown));
        assert_eq!(map_key(&key(KeyCode::Home)), Some(Command::Top));
        assert_eq!(map_key(&key(KeyCode::End)), Some(Command::Bottom));
        assert_eq!(map_key(&ch('%')), Some(Command::Percent));
        assert_eq!(map_key(&ch('7')), Some(Command::Digit(7)));
    }

    #[test]
    fn mapping_shifted_and_ctrl_keys() {
        let shift_g = KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT);
        assert_eq!(map_key(&shift_g), Some(Command::Bottom));
        assert_eq!(map_key(&ctrl('c')), Some(Command::Interrupt));
        assert_eq!(map_key(&ctrl('z')), Some(Command::Suspend));
        assert_eq!(map_key(&ctrl('l')), Some(Command::Redraw));
        assert_eq!(map_key(&ctrl('f')), Some(Command::PageDown));
        assert_eq!(map_key(&ctrl('b')), Some(Command::PageUp));
        assert_eq!(map_key(&ctrl('d')), Some(Command::HalfPageDown));
        assert_eq!(map_key(&ctrl('u')), Some(Command::HalfPageUp));
        // Unbound Ctrl / Alt combinations do nothing.
        assert_eq!(map_key(&ctrl('q')), None);
        let alt_b = KeyEvent::new(KeyCode::Char('b'), KeyModifiers::ALT);
        assert_eq!(map_key(&alt_b), None);
    }

    #[test]
    fn page_and_line_movement_clamps() {
        let mut p = pager(100, 10);
        assert_eq!(press(&mut p, &[ch(' ')]), Outcome::Draw);
        assert_eq!(p.offset(), 10);
        press(&mut p, &[ch('j'), ch('j')]);
        assert_eq!(p.offset(), 12);
        press(&mut p, &[ch('k')]);
        assert_eq!(p.offset(), 11);
        press(&mut p, &[ch('b'), ch('b')]);
        assert_eq!(p.offset(), 0);
        assert_eq!(press(&mut p, &[ch('k')]), Outcome::Nothing);
        press(&mut p, &[ch('G')]);
        assert_eq!(p.offset(), 90);
        assert!(p.at_end());
        assert_eq!(press(&mut p, &[ch(' ')]), Outcome::Nothing);
        press(&mut p, &[ch('g')]);
        assert_eq!(p.offset(), 0);
        assert_eq!(press(&mut p, &[ch('g')]), Outcome::Nothing);
    }

    #[test]
    fn half_pages() {
        let mut p = pager(100, 10);
        press(&mut p, &[ch('d')]);
        assert_eq!(p.offset(), 5);
        press(&mut p, &[ctrl('d')]);
        assert_eq!(p.offset(), 10);
        press(&mut p, &[ctrl('u'), ch('u')]);
        assert_eq!(p.offset(), 0);
    }

    #[test]
    fn short_document_never_scrolls() {
        let mut p = pager(5, 10);
        assert_eq!(p.max_offset(), 0);
        assert_eq!(
            press(&mut p, &[ch(' '), ch('j'), ch('G')]),
            Outcome::Nothing
        );
        assert_eq!(p.offset(), 0);
    }

    #[test]
    fn empty_document() {
        let mut p = pager(0, 10);
        assert_eq!(press(&mut p, &[ch(' ')]), Outcome::Nothing);
        let f = p.frame(40);
        assert_eq!(f.rows.len(), 10);
        assert!(f.status.contains("(empty)"));
    }

    #[test]
    fn count_prefix_repeats_and_jumps() {
        let mut p = pager(100, 10);
        press(&mut p, &[ch('1'), ch('5'), ch('j')]);
        assert_eq!(p.offset(), 15);
        press(&mut p, &[ch('3'), ch('k')]);
        assert_eq!(p.offset(), 12);
        press(&mut p, &[ch('2'), ch('5'), ch('g')]);
        assert_eq!(p.offset(), 24);
        press(&mut p, &[ch('5'), ch('G')]);
        assert_eq!(p.offset(), 4);
        press(&mut p, &[ch('2'), ch(' ')]);
        assert_eq!(p.offset(), 24);
        // Count is consumed: the next j moves one line.
        press(&mut p, &[ch('j')]);
        assert_eq!(p.offset(), 25);
    }

    #[test]
    fn count_is_cleared_by_unbound_key() {
        let mut p = pager(100, 10);
        press(&mut p, &[ch('9'), ch('x'), ch('j')]);
        assert_eq!(p.offset(), 1);
    }

    #[test]
    fn huge_count_saturates() {
        let mut p = pager(100, 10);
        let keys: Vec<_> = std::iter::repeat_n(ch('9'), 40).chain([ch('j')]).collect();
        press(&mut p, &keys);
        assert_eq!(p.offset(), 90);
        let keys: Vec<_> = std::iter::repeat_n(ch('9'), 40).chain([ch('k')]).collect();
        press(&mut p, &keys);
        assert_eq!(p.offset(), 0);
    }

    #[test]
    fn percent_jumps() {
        let mut p = pager(200, 10);
        press(&mut p, &[ch('5'), ch('0'), ch('%')]);
        assert_eq!(p.offset(), 100);
        press(&mut p, &[ch('p')]);
        assert_eq!(p.offset(), 0);
        press(&mut p, &[ch('1'), ch('0'), ch('0'), ch('p')]);
        assert_eq!(p.offset(), 190);
    }

    #[test]
    fn quit_suspend_redraw_outcomes() {
        let mut p = pager(10, 5);
        assert_eq!(p.handle_key(&ch('q')), Outcome::Quit(0));
        assert_eq!(p.handle_key(&ctrl('c')), Outcome::Quit(130));
        assert_eq!(p.handle_key(&ctrl('z')), Outcome::Suspend);
        assert_eq!(p.handle_key(&ctrl('l')), Outcome::Redraw);
    }

    #[test]
    fn resize_changes_page_height_and_clamps() {
        let mut p = pager(100, 10);
        press(&mut p, &[ch('5'), ch('0'), ch('g')]);
        assert_eq!(p.offset(), 49);
        p.resize(20, None);
        assert_eq!(p.offset(), 49);
        assert_eq!(p.frame(80).rows.len(), 20);
        p.resize(200, None);
        assert_eq!(p.offset(), 0);
        p.resize(0, None);
        assert_eq!(p.frame(80).rows.len(), 1);
    }

    #[test]
    fn resize_with_rerender_keeps_relative_position() {
        let mut p = pager(100, 10);
        press(&mut p, &[ch('5'), ch('1'), ch('g')]);
        assert_eq!(p.offset(), 50);
        let relaid: Vec<String> = (0..200).map(|i| i.to_string()).collect();
        p.resize(10, Some(relaid));
        assert_eq!(p.total(), 200);
        assert_eq!(p.offset(), 100);
        let relaid: Vec<String> = (0..50).map(|i| i.to_string()).collect();
        p.resize(10, Some(relaid));
        assert_eq!(p.offset(), 25);
    }

    #[test]
    fn resize_at_end_stays_at_end() {
        let mut p = pager(100, 10);
        press(&mut p, &[ch('G')]);
        let relaid: Vec<String> = (0..150).map(|i| i.to_string()).collect();
        p.resize(12, Some(relaid));
        assert_eq!(p.offset(), 138);
        assert!(p.at_end());
    }

    #[test]
    fn frame_clips_rows_and_pads_with_tilde() {
        let lines = vec!["\x1b[1mabcdefghij\x1b[0m".to_string(), "short".to_string()];
        let p = Pager::new(lines, "x.md", 4);
        let f = p.frame(5);
        assert_eq!(f.rows, vec!["\x1b[1mabcde", "short", "~", "~"]);
        assert_eq!(text::display_width(&f.status), 5);
    }

    #[test]
    fn status_shows_percent_then_end() {
        let mut p = pager(100, 10);
        let s = p.frame(80).status;
        assert!(s.contains("doc.md | lines 1-10 of 100 (10%)"), "{s}");
        assert_eq!(text::display_width(&s), 80);
        press(&mut p, &[ch('G')]);
        let s = p.frame(80).status;
        assert!(s.contains("lines 91-100 of 100 (END)"), "{s}");
    }

    #[test]
    fn status_fits_narrow_widths() {
        let long = "/a/very/long/directory/structure/for/testing/README.md";
        for width in [0, 1, 5, 10, 20, 30, 40, 60, 80, 120] {
            let s = status_line(long, "lines 1-10 of 100 (10%)", width);
            assert_eq!(text::display_width(&s), width, "width {width}: {s:?}");
        }
        let s = status_line(long, "lines 1-10 of 100 (10%)", 50);
        assert!(s.contains("…"), "{s}");
        assert!(s.contains("README.md"), "{s}");
        let s = status_line("a.md", "lines 1-10 of 100 (10%)", 80);
        assert!(s.contains(HINT_FULL), "{s}");
    }
}
