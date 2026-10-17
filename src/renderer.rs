//! Converts pulldown-cmark events into styled, word-wrapped terminal lines.
//!
//! Inline content is collected as [`Run`]s (text plus style). Block layout
//! wraps runs into [`Line`]s of spans measured by display width
//! (`unicode-width`), and escape codes are produced only when a line is
//! finalised by [`StyledWriter`], so every output line opens the
//! styles it needs and ends in the plain state: a page can start at any line.

use std::collections::VecDeque;

use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, LinkType, Tag, TagEnd};
use unicode_width::UnicodeWidthChar;

use crate::style::{self, Line, Span, Style, StyledWriter};
use crate::theme::Theme;

/// A piece of inline content in one style.
struct Run {
    style: Style,
    text: String,
    /// Inline code: never broken at spaces or between CJK characters, only
    /// by width when it is longer than a whole line.
    atomic: bool,
}

struct RenderState {
    theme: Theme,
    no_wrap: bool,
    width: usize,
    lines: Vec<String>,
    /// True when the last emitted line is blank (or nothing has been emitted
    /// yet), so consecutive blank lines collapse into one.
    last_blank: bool,
    /// Inline content of the current paragraph / heading / item text.
    inline: Vec<Run>,
    /// Nesting of block containers (quotes, lists, items), outermost first.
    /// The line prefix is derived from this stack whenever a line is emitted.
    containers: Vec<Container>,
    bold: bool,
    italic: bool,
    strikethrough: bool,
    /// Buffered text of the fenced/indented code block being read.
    code_block: Option<String>,
    in_heading: Option<u8>,
    link: Option<LinkState>,
    table_cell: Vec<Run>,
    table_row: Vec<Vec<Run>>,
    table_alignments: Vec<Alignment>,
    table_rows: Vec<Vec<Vec<Run>>>,
    in_table_cell: bool,
    /// Inside a YAML front matter block (skipped).
    in_metadata: bool,
    /// Item counts of the ordered lists not yet started, in document order
    /// (filled by [`buffer_ordered_list`]).
    ordered_counts: VecDeque<usize>,
    /// Buffered raw HTML of the HTML block being read.
    html_block: Option<String>,
    image: Option<ImageState>,
}

enum Container {
    BlockQuote,
    List {
        /// Next number for ordered lists, `None` for bullet lists.
        next: Option<u64>,
        /// Digits of the widest number in an ordered list, so that numbers
        /// are right-aligned (` 9.` / `10.`).
        num_width: usize,
    },
    Item {
        /// Styled marker (e.g. `• `) printed on the item's first line.
        marker: Span,
        /// Display width of `marker`; continuation lines indent by this much.
        width: usize,
        /// True until the marker has been printed.
        pending: bool,
    },
}

/// The link currently being rendered.
struct LinkState {
    url: String,
    /// False for autolinks/email links, whose text already is the URL.
    show_url: bool,
    /// Plain text of the link so far, to detect `[url](url)`.
    text: String,
}

/// The image placeholder currently being rendered.
struct ImageState {
    /// Index in the inline buffer of the `[image: ` label run.
    start: usize,
    /// Whether any non-blank alt text was seen.
    has_alt: bool,
}

impl RenderState {
    fn new(width: u16, theme: &Theme, no_wrap: bool) -> Self {
        Self {
            theme: *theme,
            no_wrap,
            width: width.saturating_sub(2) as usize, // margin
            lines: Vec::new(),
            last_blank: true,
            inline: Vec::new(),
            containers: Vec::new(),
            bold: false,
            italic: false,
            strikethrough: false,
            code_block: None,
            in_heading: None,
            link: None,
            table_cell: Vec::new(),
            table_row: Vec::new(),
            table_alignments: Vec::new(),
            table_rows: Vec::new(),
            in_table_cell: false,
            in_metadata: false,
            ordered_counts: VecDeque::new(),
            html_block: None,
            image: None,
        }
    }

    /// Emit a finished line, prefixed with the current container prefix.
    /// The first line emitted in a list item carries the item's marker.
    fn emit(&mut self, content: Line) {
        let mut w = StyledWriter::new(self.theme.color);
        self.write_prefix(&mut w, self.containers.len(), true, false);
        for c in &mut self.containers {
            if let Container::Item { pending, .. } = c {
                *pending = false;
            }
        }
        for span in &content.spans {
            w.push(span.style, &span.text);
        }
        self.lines.push(w.finish());
        self.last_blank = false;
    }

    /// Emit a line holding `text` in a single style.
    fn emit_text(&mut self, style: Style, text: &str) {
        let mut line = Line::new();
        line.push(style, text);
        self.emit(line);
    }

    /// Emit a separator line (once), keeping blockquote borders intact:
    /// quote borders only, no trailing whitespace.
    fn push_blank(&mut self) {
        self.flush_wrapped();
        if !self.last_blank {
            let mut w = StyledWriter::new(self.theme.color);
            if let Some(last_quote) = self
                .containers
                .iter()
                .rposition(|c| matches!(c, Container::BlockQuote))
            {
                self.write_prefix(&mut w, last_quote + 1, false, true);
            }
            self.lines.push(w.finish());
            self.last_blank = true;
        }
    }

    fn quote_bar(trailing_space: bool) -> &'static str {
        if trailing_space {
            "│ "
        } else {
            "│"
        }
    }

    /// Write the line prefix for `self.containers[..len]`.
    ///
    /// With `markers`, pending item markers are printed instead of their
    /// indentation. With `bare`, the last container (a blockquote) is drawn
    /// without its trailing space, for blank lines.
    fn write_prefix(&self, w: &mut StyledWriter, len: usize, markers: bool, bare: bool) {
        for (i, c) in self.containers[..len].iter().enumerate() {
            let in_item = i > 0 && matches!(self.containers[i - 1], Container::Item { .. });
            match c {
                Container::BlockQuote => {
                    if i == 0 {
                        w.pad(2);
                    }
                    w.push(
                        self.theme.quote_border,
                        Self::quote_bar(!(bare && i + 1 == len)),
                    );
                }
                Container::List { .. } => {
                    if !in_item {
                        w.pad(2);
                    }
                }
                Container::Item {
                    marker,
                    width,
                    pending,
                } => {
                    if markers && *pending {
                        w.push(marker.style, &marker.text);
                    } else {
                        w.pad(*width);
                    }
                }
            }
        }
    }

    /// Display width of the line prefix at the current nesting.
    fn prefix_width(&self) -> usize {
        let mut w = 0;
        for (i, c) in self.containers.iter().enumerate() {
            let in_item = i > 0 && matches!(self.containers[i - 1], Container::Item { .. });
            w += match c {
                Container::BlockQuote if i == 0 => 4,
                Container::BlockQuote => 2,
                Container::List { .. } if !in_item => 2,
                Container::List { .. } => 0,
                Container::Item { width, .. } => *width,
            };
        }
        w
    }

    /// Columns available for content after the prefix.
    fn available(&self) -> usize {
        self.width.saturating_sub(self.prefix_width())
    }

    /// Number of enclosing lists (used to pick the bullet glyph).
    fn list_depth(&self) -> usize {
        self.containers
            .iter()
            .filter(|c| matches!(c, Container::List { .. }))
            .count()
    }

    /// True when the innermost container is a list item.
    fn directly_in_item(&self) -> bool {
        matches!(self.containers.last(), Some(Container::Item { .. }))
    }

    /// Flush the inline buffer with word wrapping (or truncation in no_wrap
    /// mode). Returns the width of the widest line emitted.
    fn flush_wrapped(&mut self) -> usize {
        if self.inline.is_empty() {
            return 0;
        }
        let mut runs = std::mem::take(&mut self.inline);
        let lines = wrap_runs(&runs, self.available(), self.no_wrap);
        let mut widest = 0;
        for line in lines {
            widest = widest.max(line.width);
            self.emit(line);
        }
        runs.clear();
        self.inline = runs; // keep the allocation
        widest
    }

    /// The style of inline text at the current position.
    fn current_style(&self) -> Style {
        let t = &self.theme;
        let mut s = match self.in_heading {
            Some(level) => t.heading[usize::from(level.clamp(1, 6)) - 1],
            None => Style::PLAIN,
        };
        if self.bold {
            s = s.patch(t.bold);
        }
        if self.italic {
            s = s.patch(t.italic);
        }
        if self.strikethrough {
            s = s.patch(t.strike);
        }
        if self.image.is_some() {
            s = s.patch(t.image);
        }
        if self.link.is_some() {
            s = s.patch(t.link_text);
        }
        s
    }

    /// Where inline content goes: the current table cell or the paragraph.
    fn inline_buf(&mut self) -> &mut Vec<Run> {
        if self.in_table_cell {
            &mut self.table_cell
        } else {
            &mut self.inline
        }
    }

    fn push_run(&mut self, style: Style, text: String, atomic: bool) {
        self.inline_buf().push(Run {
            style,
            text,
            atomic,
        });
    }

    /// Append text with the active inline styles.
    fn push_inline(&mut self, text: &str) {
        if let Some(link) = self.link.as_mut() {
            link.text.push_str(text);
        }
        if let Some(image) = self.image.as_mut() {
            image.has_alt |= !text.trim().is_empty();
        }
        let style = self.current_style();
        self.push_run(style, text.to_string(), false);
    }
}

// ---------------------------------------------------------------------------
// Line breaking
// ---------------------------------------------------------------------------

/// A borrowed slice of a run.
struct Piece<'a> {
    style: Style,
    text: &'a str,
    width: usize,
}

/// A maximal unbreakable unit of inline content (`pieces[start..end]`).
/// A line may break before any segment.
struct Seg {
    /// `Some(style)` when the segment is preceded by (collapsed) whitespace,
    /// drawn as one space in that style; `None` for a zero-width break
    /// opportunity (e.g. between CJK characters) or the first segment.
    space: Option<Style>,
    start: usize,
    end: usize,
    width: usize,
}

/// Characters of scripts written without spaces, between which a line may
/// break (CJK ideographs, kana, bopomofo, fullwidth forms). Hangul is not
/// included: Korean separates words with spaces.
fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x2E80..=0x2FFF
        | 0x3000..=0x303F
        | 0x3040..=0x30FF
        | 0x3100..=0x312F
        | 0x3190..=0x33FF
        | 0x3400..=0x4DBF
        | 0x4E00..=0x9FFF
        | 0xA000..=0xA4CF
        | 0xF900..=0xFAFF
        | 0xFE30..=0xFE4F
        | 0xFF00..=0xFFEF
        | 0x20000..=0x3FFFF)
}

/// Closing punctuation and small kana that must not start a line.
fn no_break_before(c: char) -> bool {
    matches!(
        c,
        '、' | '。'
            | '，'
            | '．'
            | '：'
            | '；'
            | '！'
            | '？'
            | '）'
            | '］'
            | '｝'
            | '」'
            | '』'
            | '】'
            | '〉'
            | '》'
            | '〕'
            | '〗'
            | '〙'
            | '〛'
            | 'ー'
            | '々'
            | '・'
            | 'ゝ'
            | 'ゞ'
            | 'ヽ'
            | 'ヾ'
            | 'ぁ'
            | 'ぃ'
            | 'ぅ'
            | 'ぇ'
            | 'ぉ'
            | 'っ'
            | 'ゃ'
            | 'ゅ'
            | 'ょ'
            | 'ゎ'
            | 'ァ'
            | 'ィ'
            | 'ゥ'
            | 'ェ'
            | 'ォ'
            | 'ッ'
            | 'ャ'
            | 'ュ'
            | 'ョ'
            | 'ヮ'
            | 'ヵ'
            | 'ヶ'
            | ','
            | '.'
            | ':'
            | ';'
            | '!'
            | '?'
            | ')'
            | ']'
            | '}'
            | '%'
    ) || c.width() == Some(0)
}

/// Opening punctuation that must not end a line.
fn no_break_after(c: char) -> bool {
    matches!(
        c,
        '（' | '［'
            | '｛'
            | '「'
            | '『'
            | '【'
            | '〈'
            | '《'
            | '〔'
            | '〖'
            | '〘'
            | '〚'
            | '('
            | '['
            | '{'
    )
}

/// Whether a line may break between adjacent non-space characters.
fn breakable_between(a: char, b: char) -> bool {
    (is_cjk(a) || is_cjk(b)) && !no_break_before(b) && !no_break_after(a)
}

fn is_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n')
}

/// Splits runs into [`Seg`]s.
#[derive(Default)]
struct Segmenter<'a> {
    pieces: Vec<Piece<'a>>,
    segs: Vec<Seg>,
    open: Option<Seg>,
    space: Option<Style>,
    /// Last non-space character of the open segment (for CJK breaks).
    prev: Option<char>,
}

impl<'a> Segmenter<'a> {
    fn piece(&mut self, style: Style, text: &'a str, width: usize) {
        if text.is_empty() {
            return;
        }
        let start = self.pieces.len();
        self.pieces.push(Piece { style, text, width });
        let seg = self.open.get_or_insert_with(|| Seg {
            space: self.space.take(),
            start,
            end: start,
            width: 0,
        });
        seg.end = start + 1;
        seg.width += width;
    }

    fn close(&mut self) {
        if let Some(seg) = self.open.take() {
            self.segs.push(seg);
        }
    }

    fn run(&mut self, run: &'a Run) {
        if run.atomic {
            self.piece(run.style, &run.text, style::display_width(&run.text));
            self.prev = None;
            return;
        }
        let text = run.text.as_str();
        let (mut start, mut width) = (0, 0);
        for (i, ch) in text.char_indices() {
            if is_space(ch) {
                self.piece(run.style, &text[start..i], width);
                self.close();
                self.space.get_or_insert(run.style);
                self.prev = None;
                (start, width) = (i + ch.len_utf8(), 0);
                continue;
            }
            let both_ascii = ch.is_ascii() && self.prev.is_none_or(|p| p.is_ascii());
            if !both_ascii && self.prev.is_some_and(|p| breakable_between(p, ch)) {
                self.piece(run.style, &text[start..i], width);
                self.close();
                (start, width) = (i, 0);
            }
            width += if ch.is_ascii() {
                1 // control characters were sanitised away
            } else {
                ch.width().unwrap_or(0)
            };
            self.prev = Some(ch);
        }
        self.piece(run.style, &text[start..], width);
    }
}

/// Lay out runs in lines of at most `width` columns: break at spaces and
/// between CJK characters; a unit longer than a whole line starts on a new
/// line and is broken by display width (never inside a wide character).
/// With `no_wrap`, everything goes on one line truncated with `…`.
fn wrap_runs(runs: &[Run], width: usize, no_wrap: bool) -> Vec<Line> {
    let mut sg = Segmenter::default();
    for run in runs {
        sg.run(run);
    }
    sg.close();
    let (pieces, segs) = (sg.pieces, sg.segs);
    if segs.is_empty() {
        return Vec::new();
    }
    let width = width.max(1);

    if no_wrap {
        let mut line = Line::new();
        for seg in &segs {
            if let (Some(space), false) = (seg.space, line.spans.is_empty()) {
                line.push(space, " ");
            }
            for p in &pieces[seg.start..seg.end] {
                line.push_measured(p.style, p.text, p.width);
            }
        }
        return vec![line.truncate(width)];
    }

    let mut lines = Vec::new();
    let mut line = Line::new();
    for seg in &segs {
        if !line.spans.is_empty() {
            let space = seg.space.map_or(0, |_| 1);
            if line.width + space + seg.width > width {
                lines.push(std::mem::take(&mut line));
            } else if let Some(style) = seg.space {
                line.push(style, " ");
            }
        }
        let seg_pieces = &pieces[seg.start..seg.end];
        if line.width + seg.width <= width {
            for p in seg_pieces {
                line.push_measured(p.style, p.text, p.width);
            }
            continue;
        }
        // Longer than a whole line: break by display width.
        for p in seg_pieces {
            for ch in p.text.chars() {
                let w = ch.width().unwrap_or(0);
                if line.width > 0 && line.width + w > width {
                    lines.push(std::mem::take(&mut line));
                }
                line.push_char(p.style, ch, w);
            }
        }
    }
    if !line.spans.is_empty() {
        lines.push(line);
    }
    lines
}

/// Width of `runs` laid out on a single line.
fn natural_width(runs: &[Run]) -> usize {
    wrap_runs(runs, usize::MAX / 2, false)
        .first()
        .map_or(0, |l| l.width)
}

// ---------------------------------------------------------------------------
// HTML helpers
// ---------------------------------------------------------------------------

/// True for `<br>`, `<br/>` and `<br />` (any case).
fn is_br_tag(html: &str) -> bool {
    let inner = html.trim();
    let Some(inner) = inner.strip_prefix('<').and_then(|s| s.strip_suffix('>')) else {
        return false;
    };
    let inner = inner.trim_end_matches('/').trim_end();
    inner.eq_ignore_ascii_case("br")
}

/// Reduce raw HTML to its text: drop tags and comments, decode the common
/// entities. A `<` not followed by a tag-like character is kept literally.
fn strip_html(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(i) = rest.find('<') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let tag_like = rest[1..]
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || matches!(c, '/' | '!' | '?'));
        if !tag_like {
            out.push('<');
            rest = &rest[1..];
            continue;
        }
        let end = if rest.starts_with("<!--") {
            rest.find("-->").map(|j| j + 3)
        } else {
            rest.find('>').map(|j| j + 1)
        };
        match end {
            Some(j) => rest = &rest[j..],
            None => rest = "",
        }
    }
    out.push_str(rest);
    out.replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

// ---------------------------------------------------------------------------
// Event loop
// ---------------------------------------------------------------------------

fn digits(n: u64) -> usize {
    n.checked_ilog10().unwrap_or(0) as usize + 1
}

/// Read the rest of an ordered list (whose start event is `first`) into
/// `replay`, and append the item count of it and of every ordered list
/// nested in it to `counts`, in document order. Ordered lists need their
/// item count up front to right-align the numbers; everything else streams.
fn buffer_ordered_list<'a>(
    first: Event<'a>,
    events: &mut impl Iterator<Item = Event<'a>>,
    replay: &mut VecDeque<Event<'a>>,
    counts: &mut VecDeque<usize>,
) {
    let base = counts.len();
    // Index into `counts` of each open list (`None` for bullet lists).
    let mut open: Vec<Option<usize>> = Vec::new();
    let mut event = Some(first);
    while let Some(e) = event {
        match &e {
            Event::Start(Tag::List(start)) => {
                open.push(start.map(|_| {
                    counts.push_back(0);
                    counts.len() - 1
                }));
            }
            Event::End(TagEnd::List(_)) => {
                open.pop();
            }
            Event::Start(Tag::Item) => {
                if let Some(Some(i)) = open.last() {
                    counts[*i] += 1;
                }
            }
            _ => {}
        }
        replay.push_back(e);
        if open.is_empty() {
            break;
        }
        event = events.next();
    }
    debug_assert!(counts.len() > base);
}

/// Render a stream of markdown events into styled terminal lines.
pub fn render<'a>(
    events: impl IntoIterator<Item = Event<'a>>,
    width: u16,
    theme: &Theme,
    no_wrap: bool,
) -> Vec<String> {
    let mut state = RenderState::new(width, theme, no_wrap);
    let mut events = events.into_iter();
    let mut replay = VecDeque::new();

    loop {
        let event = match replay.pop_front() {
            Some(e) => e,
            None => match events.next() {
                Some(e @ Event::Start(Tag::List(Some(_)))) => {
                    buffer_ordered_list(e, &mut events, &mut replay, &mut state.ordered_counts);
                    continue;
                }
                Some(e) => e,
                None => break,
            },
        };
        handle_event(&mut state, event);
    }

    state.flush_wrapped();
    state.lines
}

fn handle_event(state: &mut RenderState, event: Event<'_>) {
    if state.in_metadata {
        // YAML front matter is not part of the rendered document.
        if let Event::End(TagEnd::MetadataBlock(_)) = event {
            state.in_metadata = false;
        }
        return;
    }
    match event {
        Event::Start(Tag::MetadataBlock(_)) => state.in_metadata = true,
        Event::Start(tag) => handle_start_tag(state, &tag),
        Event::End(tag) => handle_end_tag(state, &tag),
        Event::Text(text) => handle_text(state, &style::sanitize(&text)),
        Event::Code(code) => handle_code(state, &style::sanitize(&code)),
        Event::SoftBreak => handle_soft_break(state),
        Event::HardBreak => handle_hard_break(state),
        Event::Rule => handle_rule(state),
        Event::TaskListMarker(checked) => handle_task_marker(state, checked),
        Event::Html(html) => {
            if let Some(buf) = state.html_block.as_mut() {
                buf.push_str(&style::sanitize(&html));
            }
        }
        Event::InlineHtml(html) => {
            // Tags are dropped (their inner text arrives as Text events);
            // only <br> has a visible effect.
            if is_br_tag(&html) {
                if state.in_table_cell {
                    state.push_run(Style::PLAIN, " ".into(), false);
                } else {
                    handle_hard_break(state);
                }
            }
        }
        Event::FootnoteReference(label) => {
            let label = format!("[{}]", style::sanitize(&label));
            state.push_run(state.theme.footnote, label, false);
        }
        _ => {}
    }
}

fn handle_start_tag(state: &mut RenderState, tag: &Tag) {
    match tag {
        Tag::Heading { level, .. } => {
            state.push_blank();
            state.in_heading = Some(*level as u8);
        }
        Tag::Paragraph => {
            state.push_blank();
        }
        Tag::BlockQuote(_) => {
            state.push_blank();
            state.containers.push(Container::BlockQuote);
        }
        Tag::CodeBlock(kind) => {
            state.push_blank();
            let label = match kind {
                CodeBlockKind::Fenced(lang) if !lang.is_empty() => {
                    format!("  ╭─ {} ", style::sanitize(lang))
                }
                _ => "  ╭───".to_string(),
            };
            let mut line = Line::new();
            line.push(state.theme.code_label, &label);
            let line = line.truncate(state.available());
            state.emit(line);
            state.code_block = Some(String::new());
        }
        Tag::List(first) => {
            if state.directly_in_item() {
                // Tight nesting: print the parent item's text before the
                // indentation changes.
                state.flush_wrapped();
            } else {
                state.push_blank();
            }
            let num_width = first.map_or(0, |start| {
                let items = state.ordered_counts.pop_front().unwrap_or(1) as u64;
                digits(start.saturating_add(items.saturating_sub(1)))
            });
            state.containers.push(Container::List {
                next: *first,
                num_width,
            });
        }
        Tag::Item => {
            state.flush_wrapped();
            let depth = state.list_depth().saturating_sub(1);
            let (marker, style) = match state.containers.last_mut() {
                Some(Container::List {
                    next: Some(n),
                    num_width,
                }) => {
                    let s = format!("{:>w$}. ", n, w = *num_width);
                    *n = n.saturating_add(1);
                    (s, state.theme.ordered_marker)
                }
                _ => {
                    let glyph = ["• ", "◦ ", "▪ "][depth.min(2)];
                    (glyph.to_string(), state.theme.bullets[depth.min(2)])
                }
            };
            let width = style::display_width(&marker);
            state.containers.push(Container::Item {
                marker: Span {
                    style,
                    text: marker,
                },
                width,
                pending: true,
            });
        }
        Tag::FootnoteDefinition(label) => {
            // Rendered where it appears, as `[label]: body` with the body
            // hanging under itself like a list item.
            state.push_blank();
            let marker = format!("[{}]: ", style::sanitize(label));
            let width = style::display_width(&marker);
            state.containers.push(Container::Item {
                marker: Span {
                    style: state.theme.footnote,
                    text: marker,
                },
                width,
                pending: true,
            });
        }
        Tag::Emphasis => {
            state.italic = true;
        }
        Tag::Strong => {
            state.bold = true;
        }
        Tag::Strikethrough => {
            state.strikethrough = true;
        }
        Tag::Link {
            link_type,
            dest_url,
            ..
        } => {
            state.link = Some(LinkState {
                url: style::sanitize(dest_url).into_owned(),
                show_url: !matches!(link_type, LinkType::Autolink | LinkType::Email),
                text: String::new(),
            });
        }
        Tag::HtmlBlock => {
            state.flush_wrapped();
            state.html_block = Some(String::new());
        }
        Tag::Image { .. } => {
            let start = state.inline_buf().len();
            state.push_run(state.theme.image, "[image: ".into(), false);
            state.image = Some(ImageState {
                start,
                has_alt: false,
            });
        }
        Tag::Table(alignments) => {
            state.push_blank();
            state.table_alignments = alignments.clone();
            state.table_rows.clear();
        }
        Tag::TableHead | Tag::TableRow => {
            state.table_row.clear();
        }
        Tag::TableCell => {
            state.in_table_cell = true;
            state.table_cell.clear();
        }
        _ => {}
    }
}

fn handle_end_tag(state: &mut RenderState, tag: &TagEnd) {
    match tag {
        TagEnd::Heading(level) => {
            let widest = state.flush_wrapped();
            state.in_heading = None;
            // Without colour, H1/H2 keep their weight with an underline
            // as wide as the heading text.
            let underline = match level {
                HeadingLevel::H1 => "═",
                HeadingLevel::H2 => "─",
                _ => "",
            };
            if !state.theme.color && widest > 0 && !underline.is_empty() {
                state.emit_text(Style::PLAIN, &underline.repeat(widest));
            }
        }
        TagEnd::Paragraph => {
            state.flush_wrapped();
        }
        TagEnd::BlockQuote(_) => {
            state.flush_wrapped();
            state.containers.pop();
        }
        TagEnd::CodeBlock => {
            let code = state.code_block.take().unwrap_or_default();
            render_code_block(state, &code);
        }
        TagEnd::List(_) => {
            state.flush_wrapped();
            state.containers.pop();
        }
        TagEnd::Item | TagEnd::FootnoteDefinition => {
            state.flush_wrapped();
            if matches!(
                state.containers.last(),
                Some(Container::Item { pending: true, .. })
            ) {
                // Empty item: still show its marker.
                state.emit(Line::new());
            }
            state.containers.pop();
        }
        TagEnd::Emphasis => {
            state.italic = false;
        }
        TagEnd::Strong => {
            state.bold = false;
        }
        TagEnd::Strikethrough => {
            state.strikethrough = false;
        }
        TagEnd::Link => {
            if let Some(link) = state.link.take() {
                let text = link.text.trim();
                let redundant = text == link.url
                    || link.url.strip_prefix("mailto:") == Some(text)
                    || link.url.is_empty();
                if link.show_url && !redundant {
                    state.push_run(state.theme.link_url, format!(" ({})", link.url), false);
                }
            }
        }
        TagEnd::HtmlBlock => {
            let html = state.html_block.take().unwrap_or_default();
            let text = strip_html(&html);
            if !text.trim().is_empty() {
                state.push_blank();
                state.push_run(state.theme.html, text, false);
                state.flush_wrapped();
            }
        }
        TagEnd::Image => {
            if let Some(image) = state.image.take() {
                let style = state.theme.image;
                let buf = state.inline_buf();
                if image.has_alt {
                    buf.push(Run {
                        style,
                        text: "]".into(),
                        atomic: false,
                    });
                } else {
                    buf.truncate(image.start);
                    buf.push(Run {
                        style,
                        text: "[image]".into(),
                        atomic: false,
                    });
                }
            }
        }
        TagEnd::Table => {
            render_table(state);
        }
        TagEnd::TableHead | TagEnd::TableRow => {
            let row = std::mem::take(&mut state.table_row);
            state.table_rows.push(row);
        }
        TagEnd::TableCell => {
            let cell = std::mem::take(&mut state.table_cell);
            state.table_row.push(cell);
            state.in_table_cell = false;
        }
        _ => {}
    }
}

fn handle_text(state: &mut RenderState, text: &str) {
    if let Some(code) = state.code_block.as_mut() {
        code.push_str(text);
        return;
    }

    // Accumulate styled text; word wrapping happens at flush.
    state.push_inline(text);
}

/// Inline code is one unbreakable unit. In colour it is padded by a space
/// on each side inside its background; in plain mode it keeps backticks.
fn handle_code(state: &mut RenderState, code: &str) {
    if let Some(link) = state.link.as_mut() {
        link.text.push_str(code);
    }
    if let Some(image) = state.image.as_mut() {
        image.has_alt |= !code.trim().is_empty();
    }
    let text = if state.theme.color {
        format!(" {} ", code)
    } else {
        format!("`{}`", code)
    };
    let style = state.current_style().patch(state.theme.inline_code);
    state.push_run(style, text, true);
}

fn handle_soft_break(state: &mut RenderState) {
    let style = state.current_style();
    state.push_run(style, " ".into(), false);
}

/// Expand tabs to 4-column tab stops (measured in display columns).
fn expand_tabs(line: &str) -> String {
    const TAB: usize = 4;
    let mut out = String::with_capacity(line.len());
    let mut col = 0;
    for ch in line.chars() {
        if ch == '\t' {
            let n = TAB - col % TAB;
            out.extend(std::iter::repeat_n(' ', n));
            col += n;
        } else {
            out.push(ch);
            col += ch.width().unwrap_or(0);
        }
    }
    out
}

/// Emit a buffered code block: one row per source line, tabs expanded,
/// over-wide lines truncated with `…` (code is never word-wrapped).
fn render_code_block(state: &mut RenderState, code: &str) {
    let border = state.theme.code_border;
    let available = state.available().saturating_sub(4);
    for src in code.lines() {
        let mut text = Line::new();
        text.push(Style::PLAIN, &expand_tabs(src));
        if available > 0 {
            text = text.truncate(available);
        }
        let mut line = Line::new();
        line.push(border, "  │ ");
        line.append(text);
        state.emit(line);
    }
    state.emit_text(border, "  ╰───");
}

fn handle_hard_break(state: &mut RenderState) {
    state.flush_wrapped();
}

fn handle_rule(state: &mut RenderState) {
    state.push_blank();
    let rule = "─".repeat(state.available());
    state.emit_text(state.theme.rule, &rule);
    state.push_blank();
}

fn handle_task_marker(state: &mut RenderState, checked: bool) {
    let (text, style) = if checked {
        ("[✓]", state.theme.task_done)
    } else {
        ("[ ]", state.theme.task_todo)
    };
    // The checkbox replaces the item's bullet when it hasn't been printed.
    if let Some(Container::Item {
        marker,
        width,
        pending: true,
    }) = state.containers.last_mut()
    {
        *marker = Span {
            style,
            text: format!("{} ", text),
        };
        *width = 4;
        return;
    }
    state.push_run(style, text.into(), true);
    state.push_run(Style::PLAIN, " ".into(), false);
}

// ---------------------------------------------------------------------------
// Tables
// ---------------------------------------------------------------------------

/// Apply the table header style to every run of a header cell.
fn embolden(runs: &mut [Run], header: Style) {
    for r in runs {
        r.style = r.style.patch(header);
    }
}

fn render_table(state: &mut RenderState) {
    let mut rows = std::mem::take(&mut state.table_rows);
    let alignments = std::mem::take(&mut state.table_alignments);
    if let Some(header) = rows.first_mut() {
        header
            .iter_mut()
            .for_each(|c| embolden(c, state.theme.table_header));
    }

    let num_cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    if num_cols == 0 {
        return;
    }

    // Natural column widths (display width of each cell on one line).
    let mut col_widths = vec![3usize; num_cols];
    for row in &rows {
        for (i, cell) in row.iter().enumerate() {
            col_widths[i] = col_widths[i].max(natural_width(cell));
        }
    }

    // Shrink columns to fit within terminal width.
    // Overhead: 2 (margin) + 1 (left border) + 1 (right border)
    // + 2*num_cols (spaces) + (num_cols-1) (mid borders) = 3 + 3*num_cols
    let overhead = 3 + 3 * num_cols;
    let available = state.available();
    if available < overhead + 3 * num_cols {
        // Not even 3 columns per cell fit: lay the rows out vertically.
        render_table_vertical(state, rows, available);
        return;
    }
    let max_content = available - overhead;
    if col_widths.iter().sum::<usize>() > max_content {
        fit_columns(&mut col_widths, max_content);
    }

    let draw_separator = |state: &mut RenderState, left: &str, mid: &str, right: &str| {
        let mut rule = String::from(left);
        for (i, w) in col_widths.iter().enumerate() {
            rule.push_str(&"─".repeat(*w + 2));
            rule.push_str(if i + 1 < num_cols { mid } else { right });
        }
        let mut line = Line::new();
        line.pad(Style::PLAIN, 2);
        line.push(state.theme.table_border, &rule);
        state.emit(line);
    };
    let (bar, border) = ("│", state.theme.table_border);

    draw_separator(state, "┌", "┬", "┐");
    for (row_idx, row) in rows.iter().enumerate() {
        let mut wrapped: Vec<Vec<Line>> = (0..num_cols)
            .map(|i| match row.get(i) {
                Some(cell) => wrap_runs(cell, col_widths[i], state.no_wrap),
                None => Vec::new(),
            })
            .collect();
        let height = wrapped.iter().map(Vec::len).max().unwrap_or(0).max(1);

        for line_idx in 0..height {
            let mut line = Line::new();
            line.pad(Style::PLAIN, 2);
            line.push(border, bar);
            for (i, cell_lines) in wrapped.iter_mut().enumerate() {
                let content = cell_lines
                    .get_mut(line_idx)
                    .map(std::mem::take)
                    .unwrap_or_default();
                let pad = col_widths[i].saturating_sub(content.width);
                let (left, right) = match alignments.get(i).copied() {
                    Some(Alignment::Right) => (pad, 0),
                    Some(Alignment::Center) => (pad / 2, pad - pad / 2),
                    _ => (0, pad),
                };
                line.pad(Style::PLAIN, 1 + left);
                line.append(content);
                line.pad(Style::PLAIN, right + 1);
                line.push(border, bar);
            }
            state.emit(line);
        }

        if row_idx == 0 {
            draw_separator(state, "├", "┼", "┤");
        }
    }
    draw_separator(state, "└", "┴", "┘");
}

/// Shrink `widths` to sum to at most `max_content`: columns narrower than a
/// fair share keep their width, the rest split what remains evenly.
fn fit_columns(widths: &mut [usize], max_content: usize) {
    const MIN: usize = 3;
    let n = widths.len();
    if max_content <= MIN * n {
        widths.fill(MIN);
        return;
    }
    let mut remaining = max_content;
    let mut settled = vec![false; n];
    let mut unsettled = n;
    loop {
        if unsettled == 0 {
            break;
        }
        let fair_share = remaining / unsettled;
        let mut any_settled = false;
        for (w, done) in widths.iter().zip(settled.iter_mut()) {
            if !*done && *w <= fair_share {
                *done = true;
                unsettled -= 1;
                remaining -= *w;
                any_settled = true;
            }
        }
        if !any_settled {
            // All remaining columns exceed the fair share: split evenly.
            let share = remaining / unsettled;
            let mut extra = remaining % unsettled;
            for (w, done) in widths.iter_mut().zip(&settled) {
                if !*done {
                    *w = share + usize::from(extra > 0);
                    extra = extra.saturating_sub(1);
                }
            }
            break;
        }
    }
    for w in widths.iter_mut() {
        *w = (*w).max(MIN);
    }
}

/// Fallback for tables too wide for the terminal: each body row becomes a
/// block of `header: value` lines, with rows separated by a dim rule.
fn render_table_vertical(state: &mut RenderState, rows: Vec<Vec<Vec<Run>>>, available: usize) {
    let (no_wrap, border, header_style) = (
        state.no_wrap,
        state.theme.table_border,
        state.theme.table_header,
    );
    let mut rows = rows.into_iter();
    let Some(headers) = rows.next() else {
        return;
    };
    let body: Vec<_> = rows.collect();
    let rule = "─".repeat(available);

    // Header cells with a bold `:` appended, and their natural widths.
    let labels: Vec<(Vec<Run>, usize)> = headers
        .into_iter()
        .map(|mut h| {
            h.push(Run {
                style: header_style,
                text: ":".into(),
                atomic: false,
            });
            let w = natural_width(&h);
            (h, w)
        })
        .collect();
    let label_width = labels
        .iter()
        .map(|(_, w)| *w)
        .max()
        .unwrap_or(0)
        .min(available / 2);

    state.emit_text(border, &rule);
    if body.is_empty() {
        for (label, _) in &labels {
            for line in wrap_runs(label, available, no_wrap) {
                state.emit(line);
            }
        }
        state.emit_text(border, &rule);
        return;
    }
    for row in &body {
        for (i, (label, natural)) in labels.iter().enumerate() {
            let value: &[Run] = row.get(i).map_or(&[], Vec::as_slice);
            let label_cols = (*natural).max(label_width) + 1;
            let value_width = available.saturating_sub(label_cols);
            if value_width >= 8 {
                let mut lines = wrap_runs(value, value_width, no_wrap).into_iter();
                let mut first = wrap_runs(label, usize::MAX / 2, false)
                    .pop()
                    .unwrap_or_default();
                first.pad(Style::PLAIN, label_cols - first.width);
                if let Some(v) = lines.next() {
                    first.append(v);
                }
                state.emit(first);
                for v in lines {
                    let mut line = Line::new();
                    line.pad(Style::PLAIN, label_cols);
                    line.append(v);
                    state.emit(line);
                }
            } else {
                // Too narrow for side-by-side: value below its label.
                for line in wrap_runs(label, available, no_wrap) {
                    state.emit(line);
                }
                for v in wrap_runs(value, available.saturating_sub(2), no_wrap) {
                    let mut line = Line::new();
                    line.pad(Style::PLAIN, 2);
                    line.append(v);
                    state.emit(line);
                }
            }
        }
        state.emit_text(border, &rule);
    }
}

#[cfg(test)]
mod snapshot_tests;

#[cfg(test)]
mod tests;
