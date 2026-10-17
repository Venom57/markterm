//! Terminal styling: the [`Style`]/[`Span`] model the renderer builds lines
//! from, SGR escape generation, and the one ANSI-aware display-width
//! implementation shared by the renderer and the pager.

use std::borrow::Cow;

use unicode_width::UnicodeWidthChar;

pub const RESET: &str = "\x1b[0m";
pub const REVERSE: &str = "\x1b[7m";

/// A terminal colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Color {
    /// One of the 16 standard colours (0–7 normal, 8–15 bright).
    Ansi(u8),
    /// An entry of the 256-colour palette.
    Indexed(u8),
}

impl Color {
    fn write_sgr(self, out: &mut String, background: bool) {
        use std::fmt::Write;
        let _ = match self {
            Color::Ansi(n) if n < 8 => write!(out, "{}", if background { 40 } else { 30 } + n),
            Color::Ansi(n) => write!(
                out,
                "{}",
                if background { 100 } else { 90 } + (n.min(15) - 8)
            ),
            Color::Indexed(n) => write!(out, "{};5;{}", if background { 48 } else { 38 }, n),
        };
    }
}

/// Text attributes plus optional colours. `Style::default()` is "plain".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Style {
    attrs: u8,
    fg: Option<Color>,
    bg: Option<Color>,
}

const A_BOLD: u8 = 1;
const A_DIM: u8 = 1 << 1;
const A_ITALIC: u8 = 1 << 2;
const A_UNDERLINE: u8 = 1 << 3;
const A_REVERSE: u8 = 1 << 4;
const A_STRIKE: u8 = 1 << 5;

/// SGR parameter for each attribute bit, in emission order.
const ATTR_CODES: [(u8, &str); 6] = [
    (A_BOLD, "1"),
    (A_DIM, "2"),
    (A_ITALIC, "3"),
    (A_UNDERLINE, "4"),
    (A_REVERSE, "7"),
    (A_STRIKE, "9"),
];

impl Style {
    pub const PLAIN: Style = Style {
        attrs: 0,
        fg: None,
        bg: None,
    };

    const fn with(self, attr: u8) -> Style {
        Style {
            attrs: self.attrs | attr,
            ..self
        }
    }

    pub const fn bold(self) -> Style {
        self.with(A_BOLD)
    }
    pub const fn dim(self) -> Style {
        self.with(A_DIM)
    }
    pub const fn italic(self) -> Style {
        self.with(A_ITALIC)
    }
    pub const fn underline(self) -> Style {
        self.with(A_UNDERLINE)
    }
    pub const fn strike(self) -> Style {
        self.with(A_STRIKE)
    }
    pub const fn fg(self, c: Color) -> Style {
        Style {
            fg: Some(c),
            ..self
        }
    }
    pub const fn bg(self, c: Color) -> Style {
        Style {
            bg: Some(c),
            ..self
        }
    }

    pub fn is_plain(self) -> bool {
        self == Style::PLAIN
    }

    /// `self` with `over` layered on top: attributes are combined and the
    /// colours of `over`, where set, win.
    pub fn patch(self, over: Style) -> Style {
        Style {
            attrs: self.attrs | over.attrs,
            fg: over.fg.or(self.fg),
            bg: over.bg.or(self.bg),
        }
    }

    /// Append the SGR parameters of this style (`1;4;97`), `;`-separated.
    fn write_params(self, out: &mut String) {
        let mut first = true;
        let mut sep = |out: &mut String| {
            if !first {
                out.push(';');
            }
            first = false;
        };
        for (bit, code) in ATTR_CODES {
            if self.attrs & bit != 0 {
                sep(out);
                out.push_str(code);
            }
        }
        if let Some(c) = self.fg {
            sep(out);
            c.write_sgr(out, false);
        }
        if let Some(c) = self.bg {
            sep(out);
            c.write_sgr(out, true);
        }
    }

    /// The escape sequence that switches the terminal from `from` to `self`.
    fn write_transition(self, from: Style, out: &mut String) {
        if self == from {
            return;
        }
        if self.is_plain() {
            out.push_str(RESET);
            return;
        }
        out.push_str("\x1b[");
        if !from.is_plain() {
            out.push_str("0;");
        }
        self.write_params(out);
        out.push('m');
    }

    /// The escape sequence that turns this style on (empty for plain).
    #[cfg(test)]
    pub fn sgr(self) -> String {
        let mut s = String::new();
        self.write_transition(Style::PLAIN, &mut s);
        s
    }
}

/// A run of text drawn in one style.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub style: Style,
    pub text: String,
}

/// A line under construction: spans plus their total display width.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Line {
    pub spans: Vec<Span>,
    pub width: usize,
}

impl Line {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append `text` in `style`, merging with the last span when the style
    /// is the same.
    pub fn push(&mut self, style: Style, text: &str) {
        self.push_measured(style, text, display_width(text));
    }

    /// Append `text` whose display width the caller already knows.
    pub fn push_measured(&mut self, style: Style, text: &str, width: usize) {
        if text.is_empty() {
            return;
        }
        self.width += width;
        match self.spans.last_mut() {
            Some(last) if last.style == style => last.text.push_str(text),
            _ => self.spans.push(Span {
                style,
                text: text.to_string(),
            }),
        }
    }

    /// Append a single character (whose width the caller already knows).
    pub fn push_char(&mut self, style: Style, ch: char, width: usize) {
        self.width += width;
        match self.spans.last_mut() {
            Some(last) if last.style == style => last.text.push(ch),
            _ => self.spans.push(Span {
                style,
                text: ch.to_string(),
            }),
        }
    }

    /// Append `n` spaces in `style`.
    pub fn pad(&mut self, style: Style, n: usize) {
        if n > 0 {
            self.push(style, &" ".repeat(n));
        }
    }

    pub fn append(&mut self, other: Line) {
        for span in other.spans {
            self.push(span.style, &span.text);
        }
    }

    /// Shorten to at most `max` columns, ending in `…` when anything was cut.
    /// A wide character that would straddle the cut is dropped whole.
    pub fn truncate(self, max: usize) -> Line {
        if self.width <= max {
            return self;
        }
        let budget = max.saturating_sub(1);
        let mut out = Line::new();
        'spans: for span in &self.spans {
            for ch in span.text.chars() {
                let w = ch.width().unwrap_or(0);
                if out.width + w > budget {
                    break 'spans;
                }
                out.push_char(span.style, ch, w);
            }
        }
        if max > 0 {
            out.push(Style::PLAIN, "…");
        }
        out
    }
}

/// Builds one output line from styled text. With `color`, escape codes are
/// emitted only at style changes and [`finish`](Self::finish) always returns
/// to the plain state, so each line is self-contained and renders correctly
/// on its own.
pub struct StyledWriter {
    out: String,
    cur: Style,
    color: bool,
}

impl StyledWriter {
    pub fn new(color: bool) -> Self {
        Self {
            out: String::new(),
            cur: Style::PLAIN,
            color,
        }
    }

    pub fn push(&mut self, style: Style, text: &str) {
        if text.is_empty() {
            return;
        }
        if self.color {
            style.write_transition(self.cur, &mut self.out);
            self.cur = style;
        }
        self.out.push_str(text);
    }

    /// Append `n` unstyled spaces.
    pub fn pad(&mut self, n: usize) {
        if n > 0 {
            if self.color {
                Style::PLAIN.write_transition(self.cur, &mut self.out);
                self.cur = Style::PLAIN;
            }
            self.out.extend(std::iter::repeat_n(' ', n));
        }
    }

    pub fn finish(mut self) -> String {
        if self.color {
            Style::PLAIN.write_transition(self.cur, &mut self.out);
        }
        self.out
    }
}

/// Serialise spans to a self-contained string (see [`StyledWriter`]).
#[cfg(test)]
pub fn render_spans<'a>(spans: impl IntoIterator<Item = &'a Span>, color: bool) -> String {
    let mut w = StyledWriter::new(color);
    for span in spans {
        w.push(span.style, &span.text);
    }
    w.finish()
}

/// True for control characters that must never reach the terminal from
/// document content: C0 controls other than tab and newline, DEL, and C1.
fn is_unsafe_control(c: char) -> bool {
    matches!(c, '\0'..='\x08' | '\x0b'..='\x1f' | '\x7f'..='\u{9f}')
}

/// Replace unsafe control characters (ESC, BEL, CR, DEL, C1, ...) with
/// U+FFFD so untrusted input cannot inject terminal escape sequences.
pub fn sanitize(s: &str) -> Cow<'_, str> {
    if !s.chars().any(is_unsafe_control) {
        return Cow::Borrowed(s);
    }
    Cow::Owned(
        s.chars()
            .map(|c| if is_unsafe_control(c) { '\u{FFFD}' } else { c })
            .collect(),
    )
}

// ---------------------------------------------------------------------------
// ANSI-aware width helpers (shared with the pager)
// ---------------------------------------------------------------------------

/// Tab stops used when expanding tabs for display.
pub const TAB_WIDTH: usize = 8;

/// One piece of a styled string: an escape sequence or a single character.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Token<'a> {
    Esc(&'a str),
    Char(char),
}

/// Iterator over the [`Token`]s of a string.
pub struct Tokens<'a> {
    s: &'a str,
    pos: usize,
}

/// Split `s` into escape sequences and characters.
pub fn tokens(s: &str) -> Tokens<'_> {
    Tokens { s, pos: 0 }
}

impl<'a> Iterator for Tokens<'a> {
    type Item = Token<'a>;

    fn next(&mut self) -> Option<Token<'a>> {
        let rest = &self.s[self.pos..];
        let c = rest.chars().next()?;
        if c != '\x1b' {
            self.pos += c.len_utf8();
            return Some(Token::Char(c));
        }
        let len = escape_len(rest);
        self.pos += len;
        Some(Token::Esc(&rest[..len]))
    }
}

/// Byte length of the escape sequence at the start of `s` (which begins
/// with ESC). Handles CSI, OSC/DCS/APC/PM/SOS strings and two-byte escapes;
/// an unterminated sequence extends to the end of the string.
fn escape_len(s: &str) -> usize {
    let b = s.as_bytes();
    match b.get(1) {
        None => 1,
        Some(b'[') => {
            for (i, &c) in b.iter().enumerate().skip(2) {
                if (0x40..=0x7e).contains(&c) {
                    return i + 1;
                }
                if !(0x20..=0x3f).contains(&c) {
                    // Malformed: stop before the offending byte.
                    return i;
                }
            }
            b.len()
        }
        Some(b']' | b'P' | b'X' | b'^' | b'_') => {
            let mut i = 2;
            while i < b.len() {
                if b[i] == 0x07 {
                    return i + 1;
                }
                if b[i] == 0x1b && b.get(i + 1) == Some(&b'\\') {
                    return i + 2;
                }
                i += 1;
            }
            b.len()
        }
        Some(_) => 1 + s[1..].chars().next().map_or(0, char::len_utf8),
    }
}

/// Display width of `c` when printed at column `col`: 2 for wide (CJK,
/// emoji) characters, 0 for combining marks and other zero-width
/// characters, tab stops for `\t`.
pub fn char_width(c: char, col: usize) -> usize {
    if c == '\t' {
        TAB_WIDTH - col % TAB_WIDTH
    } else {
        c.width().unwrap_or(0)
    }
}

/// Display width of a (possibly styled) string.
pub fn display_width(s: &str) -> usize {
    if s.bytes().all(|b| (0x20..0x7f).contains(&b)) {
        // Fast path: printable ASCII is one column per byte.
        return s.len();
    }
    tokens(s).fold(0, |col, t| match t {
        Token::Char(c) => col + char_width(c, col),
        Token::Esc(_) => col,
    })
}

/// Remove all escape sequences from `s`.
pub fn strip_ansi(s: &str) -> String {
    tokens(s)
        .filter_map(|t| match t {
            Token::Char(c) => Some(c),
            Token::Esc(_) => None,
        })
        .collect()
}

/// Clip a styled line to at most `width` display columns. Escape sequences
/// are kept intact (never cut), tabs are expanded to spaces, and a wide
/// character that would straddle the edge is dropped. The caller is
/// responsible for emitting a reset afterwards.
pub fn clip(s: &str, width: usize) -> String {
    let mut out = String::with_capacity(s.len());
    let mut col = 0;
    for t in tokens(s) {
        match t {
            Token::Esc(e) => out.push_str(e),
            Token::Char(c) => {
                let w = char_width(c, col);
                if col + w > width {
                    if c == '\t' {
                        // Fill up to the edge with the part of the tab that fits.
                        out.extend(std::iter::repeat_n(' ', width - col));
                    }
                    break;
                }
                if c == '\t' {
                    out.extend(std::iter::repeat_n(' ', w));
                } else {
                    out.push(c);
                }
                col += w;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sgr_codes() {
        assert_eq!(Style::PLAIN.sgr(), "");
        assert_eq!(Style::PLAIN.bold().sgr(), "\x1b[1m");
        assert_eq!(
            Style::PLAIN
                .bold()
                .underline()
                .fg(Color::Ansi(15))
                .bg(Color::Indexed(236))
                .sgr(),
            "\x1b[1;4;97;48;5;236m"
        );
        assert_eq!(Style::PLAIN.fg(Color::Ansi(6)).sgr(), "\x1b[36m");
        assert_eq!(Style::PLAIN.bg(Color::Ansi(1)).sgr(), "\x1b[41m");
        assert_eq!(Style::PLAIN.fg(Color::Indexed(25)).sgr(), "\x1b[38;5;25m");
    }

    #[test]
    fn patch_layers_styles() {
        let a = Style::PLAIN.bold().fg(Color::Ansi(1));
        let b = Style::PLAIN.italic().fg(Color::Ansi(2));
        assert_eq!(a.patch(b), Style::PLAIN.bold().italic().fg(Color::Ansi(2)));
        assert_eq!(b.patch(Style::PLAIN), b);
    }

    #[test]
    fn render_spans_is_self_contained() {
        let bold = Style::PLAIN.bold();
        let dim = Style::PLAIN.dim();
        let spans = [
            Span {
                style: Style::PLAIN,
                text: "a ".into(),
            },
            Span {
                style: bold,
                text: "b".into(),
            },
            Span {
                style: dim,
                text: "c".into(),
            },
        ];
        assert_eq!(render_spans(&spans, true), "a \x1b[1mb\x1b[0;2mc\x1b[0m");
        assert_eq!(render_spans(&spans, false), "a bc");
        assert_eq!(render_spans(&spans[..1], true), "a ");
    }

    #[test]
    fn line_push_merges_and_measures() {
        let mut l = Line::new();
        l.push(Style::PLAIN, "日本");
        l.push(Style::PLAIN, "e\u{301}");
        assert_eq!(l.spans.len(), 1);
        assert_eq!(l.width, 5);
    }

    #[test]
    fn line_truncate_respects_wide_chars() {
        let mut l = Line::new();
        l.push(Style::PLAIN, "ab日本語");
        let t = l.clone().truncate(5);
        assert_eq!(render_spans(&t.spans, false), "ab日…");
        assert_eq!(t.width, 5);
        let t = l.clone().truncate(4);
        assert_eq!(render_spans(&t.spans, false), "ab…");
        assert_eq!(l.clone().truncate(8), l);
    }

    #[test]
    fn test_sanitize() {
        assert!(matches!(sanitize("plain\ttext\n"), Cow::Borrowed(_)));
        assert_eq!(
            sanitize("a\x1b]0;t\x07b\x7f\r\u{9b}"),
            "a\u{FFFD}]0;t\u{FFFD}b\u{FFFD}\u{FFFD}\u{FFFD}"
        );
    }

    #[test]
    fn tokenises_csi_and_chars() {
        let t: Vec<_> = tokens("\x1b[1mhi\x1b[0m").collect();
        assert_eq!(
            t,
            vec![
                Token::Esc("\x1b[1m"),
                Token::Char('h'),
                Token::Char('i'),
                Token::Esc("\x1b[0m"),
            ]
        );
    }

    #[test]
    fn tokenises_osc_with_bel_and_st() {
        let t: Vec<_> = tokens("\x1b]8;;http://x\x07a\x1b]8;;\x1b\\").collect();
        assert_eq!(
            t,
            vec![
                Token::Esc("\x1b]8;;http://x\x07"),
                Token::Char('a'),
                Token::Esc("\x1b]8;;\x1b\\"),
            ]
        );
    }

    #[test]
    fn tokenises_unterminated_and_lone_escape() {
        assert_eq!(
            tokens("\x1b[12").collect::<Vec<_>>(),
            [Token::Esc("\x1b[12")]
        );
        assert_eq!(tokens("\x1b").collect::<Vec<_>>(), [Token::Esc("\x1b")]);
        assert_eq!(
            tokens("\x1b(Bx").collect::<Vec<_>>(),
            [Token::Esc("\x1b("), Token::Char('B'), Token::Char('x')]
        );
    }

    #[test]
    fn malformed_csi_does_not_swallow_text() {
        let t: Vec<_> = tokens("\x1b[1é").collect();
        assert_eq!(t, vec![Token::Esc("\x1b[1"), Token::Char('é')]);
    }

    #[test]
    fn width_counts_wide_and_zero_width() {
        assert_eq!(display_width("abc"), 3);
        assert_eq!(display_width("\x1b[1m日本\x1b[0m"), 4);
        assert_eq!(display_width("e\u{301}"), 1);
        assert_eq!(display_width("a\tb"), 9);
        assert_eq!(display_width("🎉"), 2);
        assert_eq!(display_width("a\u{200d}b"), 2);
        assert_eq!(display_width("\x1b[1mab\x1b[0m"), 2);
    }

    #[test]
    fn strip_removes_all_escapes() {
        assert_eq!(strip_ansi("\x1b[1;31mred\x1b[0m plain"), "red plain");
        assert_eq!(strip_ansi("\x1b]8;;u\x07link\x1b]8;;\x07"), "link");
    }

    #[test]
    fn clip_short_line_unchanged() {
        assert_eq!(clip("\x1b[1mhi\x1b[0m", 10), "\x1b[1mhi\x1b[0m");
    }

    #[test]
    fn clip_never_cuts_escape() {
        let s = "\x1b[1mhello\x1b[0m world";
        let c = clip(s, 3);
        assert_eq!(c, "\x1b[1mhel");
        assert_eq!(display_width(&c), 3);
    }

    #[test]
    fn clip_drops_straddling_wide_char() {
        assert_eq!(clip("a日本", 2), "a");
        assert_eq!(clip("a日本", 3), "a日");
    }

    #[test]
    fn clip_expands_tabs() {
        assert_eq!(clip("a\tb", 20), "a       b");
        assert_eq!(clip("a\tb", 4), "a   ");
    }
}
