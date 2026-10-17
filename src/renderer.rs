//! Converts pulldown-cmark events into ANSI-styled, word-wrapped terminal lines.

use pulldown_cmark::{Alignment, CodeBlockKind, Event, LinkType, Tag, TagEnd};

use crate::style;

struct RenderState {
    use_color: bool,
    no_wrap: bool,
    width: usize,
    lines: Vec<String>,
    /// True when the last emitted line is blank (or nothing has been emitted
    /// yet), so consecutive blank lines collapse into one.
    last_blank: bool,
    current_line: String,
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
    table_row: Vec<String>,
    table_cell_buf: String,
    table_alignments: Vec<Alignment>,
    table_rows: Vec<Vec<String>>,
    in_table_cell: bool,
    /// Inside a YAML front matter block (skipped).
    in_metadata: bool,
    /// Buffered raw HTML of the HTML block being read.
    html_block: Option<String>,
    /// Inline buffer offset where the current image placeholder starts.
    image_start: Option<usize>,
}

enum Container {
    BlockQuote,
    List {
        /// Next number for ordered lists, `None` for bullet lists.
        next: Option<u64>,
    },
    Item {
        /// Styled marker (e.g. `• `) printed on the item's first line.
        marker: String,
        /// Visible width of `marker`; continuation lines indent by this much.
        width: usize,
        /// True until the marker has been printed.
        pending: bool,
    },
}

impl RenderState {
    fn new(width: u16, use_color: bool, no_wrap: bool) -> Self {
        Self {
            use_color,
            no_wrap,
            width: width.saturating_sub(2) as usize, // margin
            lines: Vec::new(),
            last_blank: true,
            current_line: String::new(),
            containers: Vec::new(),
            bold: false,
            italic: false,
            strikethrough: false,
            code_block: None,
            in_heading: None,
            link: None,
            table_row: Vec::new(),
            table_cell_buf: String::new(),
            table_alignments: Vec::new(),
            table_rows: Vec::new(),
            in_table_cell: false,
            in_metadata: false,
            html_block: None,
            image_start: None,
        }
    }

    /// Emit a finished line, prefixed with the current container prefix.
    /// The first line emitted in a list item carries the item's marker.
    fn emit(&mut self, content: &str) {
        let prefix = self.take_prefix();
        self.lines.push(format!("{}{}", prefix, content));
        self.last_blank = false;
    }

    /// Emit a separator line (once), keeping blockquote borders intact.
    fn push_blank(&mut self) {
        self.flush_wrapped();
        if !self.last_blank {
            let bare = self.bare_prefix();
            self.lines.push(bare);
            self.last_blank = true;
        }
    }

    fn quote_bar(&self, trailing_space: bool) -> String {
        let bar = if self.use_color { "│" } else { "|" };
        let space = if trailing_space { " " } else { "" };
        style::styled(&format!("{}{}", bar, space), &[style::DIM], self.use_color)
    }

    /// Build the line prefix for `self.containers[..len]`.
    ///
    /// With `markers`, pending item markers are printed instead of their
    /// indentation. With `bare`, the last container (a blockquote) is drawn
    /// without its trailing space, for blank lines.
    fn build_prefix(&self, len: usize, markers: bool, bare: bool) -> String {
        let mut prefix = String::new();
        for (i, c) in self.containers[..len].iter().enumerate() {
            let in_item = i > 0 && matches!(self.containers[i - 1], Container::Item { .. });
            match c {
                Container::BlockQuote => {
                    if i == 0 {
                        prefix.push_str("  ");
                    }
                    prefix.push_str(&self.quote_bar(!(bare && i + 1 == len)));
                }
                Container::List { .. } => {
                    if !in_item {
                        prefix.push_str("  ");
                    }
                }
                Container::Item {
                    marker,
                    width,
                    pending,
                } => {
                    if markers && *pending {
                        prefix.push_str(marker);
                    } else {
                        prefix.push_str(&" ".repeat(*width));
                    }
                }
            }
        }
        prefix
    }

    /// Prefix for the next line; consumes pending item markers.
    fn take_prefix(&mut self) -> String {
        let prefix = self.build_prefix(self.containers.len(), true, false);
        for c in &mut self.containers {
            if let Container::Item { pending, .. } = c {
                *pending = false;
            }
        }
        prefix
    }

    /// Prefix for blank lines: quote borders only, no trailing whitespace.
    fn bare_prefix(&self) -> String {
        match self
            .containers
            .iter()
            .rposition(|c| matches!(c, Container::BlockQuote))
        {
            Some(last_quote) => self.build_prefix(last_quote + 1, false, true),
            None => String::new(),
        }
    }

    /// Visible width of the line prefix at the current nesting.
    fn prefix_width(&self) -> usize {
        style::visible_len(&self.build_prefix(self.containers.len(), false, false))
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

    /// Flush `current_line` with word wrapping (or truncation in no_wrap mode).
    fn flush_wrapped(&mut self) {
        if self.current_line.is_empty() {
            return;
        }
        let text = std::mem::take(&mut self.current_line);
        let segments = split_styled_words(&text);
        if segments.is_empty() {
            return;
        }
        let available = self.width.saturating_sub(self.prefix_width());

        if available == 0 {
            self.emit(&text);
            return;
        }

        if self.no_wrap {
            // Truncate mode: single line, add ellipsis if it exceeds width
            if style::visible_len(&text) <= available {
                self.emit(&text);
            } else {
                let truncated = truncate_styled(&text, available.saturating_sub(1), self.use_color);
                self.emit(&truncated);
            }
            return;
        }

        // Word-wrap mode. Continuation lines get the same prefix width as the
        // first line, so wrapped list text hangs under the item text.
        let mut line_buf = String::new();
        let mut line_visible = 0usize;

        for seg in &segments {
            let seg_visible = style::visible_len(seg);

            if line_visible == 0 {
                line_buf.push_str(seg);
                line_visible = seg_visible;
            } else if line_visible + 1 + seg_visible <= available {
                line_buf.push(' ');
                line_buf.push_str(seg);
                line_visible += 1 + seg_visible;
            } else {
                let done = std::mem::replace(&mut line_buf, seg.clone());
                self.emit(&done);
                line_visible = seg_visible;
            }
        }

        if !line_buf.is_empty() {
            self.emit(&line_buf);
        }
    }

    fn current_style_prefix(&self) -> String {
        if !self.use_color {
            return String::new();
        }
        let mut codes = Vec::new();
        if let Some(level) = self.in_heading {
            match level {
                1 => {
                    codes.push(style::BOLD);
                    codes.push(style::UNDERLINE);
                    codes.push(style::FG_BRIGHT_WHITE);
                }
                2 => {
                    codes.push(style::BOLD);
                    codes.push(style::FG_BRIGHT_CYAN);
                }
                3 => {
                    codes.push(style::BOLD);
                    codes.push(style::FG_BRIGHT_YELLOW);
                }
                _ => {
                    codes.push(style::BOLD);
                }
            }
        }
        if self.bold {
            codes.push(style::BOLD);
        }
        if self.italic {
            codes.push(style::ITALIC);
        }
        if self.strikethrough {
            codes.push(style::STRIKETHROUGH);
        }
        if self.image_start.is_some() {
            codes.push(style::DIM);
        }
        if self.link.is_some() {
            codes.push(style::UNDERLINE);
        }
        style::combine(&codes)
    }

    /// Where inline content goes: the current table cell or the paragraph.
    fn inline_buf(&mut self) -> &mut String {
        if self.in_table_cell {
            &mut self.table_cell_buf
        } else {
            &mut self.current_line
        }
    }

    /// Append text with the active inline styles.
    fn push_inline(&mut self, text: &str) {
        let pre = self.current_style_prefix();
        let buf = self.inline_buf();
        if pre.is_empty() {
            buf.push_str(text);
        } else {
            buf.push_str(&pre);
            buf.push_str(text);
            buf.push_str(style::RESET);
        }
    }
}

/// The link currently being rendered.
struct LinkState {
    url: String,
    /// False for autolinks/email links, whose text already is the URL.
    show_url: bool,
    /// Byte offset in the inline buffer where the link text starts.
    text_start: usize,
}

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

/// Remove ANSI escape sequences, leaving only visible text.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_escape = false;
    for ch in s.chars() {
        if in_escape {
            in_escape = !ch.is_ascii_alphabetic();
        } else if ch == '\x1b' {
            in_escape = true;
        } else {
            out.push(ch);
        }
    }
    out
}

/// Split a string (potentially containing ANSI codes) into whitespace-delimited segments,
/// preserving ANSI codes attached to the words they surround.
fn split_styled_words(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut in_escape = false;
    let mut has_visible = false;

    for ch in text.chars() {
        if in_escape {
            current.push(ch);
            if ch.is_ascii_alphabetic() {
                in_escape = false;
            }
        } else if ch == '\x1b' {
            in_escape = true;
            current.push(ch);
        } else if ch == ' ' || ch == '\t' {
            if has_visible {
                words.push(std::mem::take(&mut current));
                has_visible = false;
            }
            // Discard whitespace between words (we'll re-add spaces during wrapping)
        } else {
            current.push(ch);
            has_visible = true;
        }
    }

    if !current.is_empty() {
        words.push(current);
    }

    words
}

/// Word-wrap a cell's text to fit within `max_width` visible characters.
/// Returns a Vec of lines. In no_wrap mode, truncates with ellipsis instead.
fn wrap_cell_text(text: &str, max_width: usize, no_wrap: bool, use_color: bool) -> Vec<String> {
    if max_width == 0 {
        return vec![text.to_string()];
    }

    let visible = style::visible_len(text);
    if visible <= max_width {
        return vec![text.to_string()];
    }

    if no_wrap {
        return vec![truncate_styled(
            text,
            max_width.saturating_sub(1),
            use_color,
        )];
    }

    let words = split_styled_words(text);
    if words.is_empty() {
        return vec![String::new()];
    }

    let mut lines = Vec::new();
    let mut current_line = String::new();
    let mut current_visible = 0usize;

    for word in &words {
        let word_visible = style::visible_len(word);

        if current_visible == 0 {
            current_line.push_str(word);
            current_visible = word_visible;
        } else if current_visible + 1 + word_visible <= max_width {
            current_line.push(' ');
            current_line.push_str(word);
            current_visible += 1 + word_visible;
        } else {
            lines.push(std::mem::take(&mut current_line));
            current_line = word.to_string();
            current_visible = word_visible;
        }
    }

    if !current_line.is_empty() || lines.is_empty() {
        lines.push(current_line);
    }

    lines
}

/// Truncate a string containing ANSI codes to `max_visible` visible characters,
/// appending an ellipsis character and a RESET if needed.
fn truncate_styled(text: &str, max_visible: usize, use_color: bool) -> String {
    let mut result = String::new();
    let mut visible = 0;
    let mut in_escape = false;

    for ch in text.chars() {
        if in_escape {
            result.push(ch);
            if ch.is_ascii_alphabetic() {
                in_escape = false;
            }
        } else if ch == '\x1b' {
            in_escape = true;
            result.push(ch);
        } else {
            if visible >= max_visible {
                break;
            }
            result.push(ch);
            visible += 1;
        }
    }

    if use_color {
        result.push_str(style::RESET);
    }
    result.push('…');
    result
}

/// Render a stream of markdown events into styled terminal lines.
pub fn render(events: Vec<Event<'_>>, width: u16, use_color: bool, no_wrap: bool) -> Vec<String> {
    let mut state = RenderState::new(width, use_color, no_wrap);

    for event in events {
        if state.in_metadata {
            // YAML front matter is not part of the rendered document.
            if let Event::End(TagEnd::MetadataBlock(_)) = event {
                state.in_metadata = false;
            }
            continue;
        }
        match event {
            Event::Start(Tag::MetadataBlock(_)) => state.in_metadata = true,
            Event::Start(tag) => handle_start_tag(&mut state, &tag),
            Event::End(tag) => handle_end_tag(&mut state, &tag),
            Event::Text(text) => handle_text(&mut state, &style::sanitize(&text)),
            Event::Code(code) => handle_code(&mut state, &style::sanitize(&code)),
            Event::SoftBreak => handle_soft_break(&mut state),
            Event::HardBreak => handle_hard_break(&mut state),
            Event::Rule => handle_rule(&mut state),
            Event::TaskListMarker(checked) => handle_task_marker(&mut state, checked),
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
                        state.inline_buf().push(' ');
                    } else {
                        handle_hard_break(&mut state);
                    }
                }
            }
            Event::FootnoteReference(label) => {
                let label = format!("[{}]", style::sanitize(&label));
                let styled = style::styled(&label, &[style::DIM], state.use_color);
                state.inline_buf().push_str(&styled);
            }
            _ => {}
        }
    }

    state.flush_wrapped();
    state.lines
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
            state.emit(&style::styled(&label, &[style::DIM], state.use_color));
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
            state.containers.push(Container::List { next: *first });
        }
        Tag::Item => {
            state.flush_wrapped();
            let depth = state.list_depth().saturating_sub(1);
            let marker = match state.containers.last_mut() {
                Some(Container::List { next: Some(n) }) => {
                    let s = format!("{}. ", n);
                    *n += 1;
                    s
                }
                _ => match depth {
                    0 => "• ".to_string(),
                    1 => "◦ ".to_string(),
                    _ => "▪ ".to_string(),
                },
            };
            let width = style::visible_len(&marker);
            let marker = style::styled(&marker, &[style::FG_CYAN], state.use_color);
            state.containers.push(Container::Item {
                marker,
                width,
                pending: true,
            });
        }
        Tag::FootnoteDefinition(label) => {
            // Rendered where it appears, as `[label]: body` with the body
            // hanging under itself like a list item.
            state.push_blank();
            let marker = format!("[{}]: ", style::sanitize(label));
            let width = style::visible_len(&marker);
            let marker = style::styled(&marker, &[style::DIM], state.use_color);
            state.containers.push(Container::Item {
                marker,
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
            let text_start = state.inline_buf().len();
            state.link = Some(LinkState {
                url: style::sanitize(dest_url).into_owned(),
                show_url: !matches!(link_type, LinkType::Autolink | LinkType::Email),
                text_start,
            });
        }
        Tag::HtmlBlock => {
            state.flush_wrapped();
            state.html_block = Some(String::new());
        }
        Tag::Image { .. } => {
            let start = state.inline_buf().len();
            let label = style::styled("[image: ", &[style::DIM], state.use_color);
            state.inline_buf().push_str(&label);
            state.image_start = Some(start);
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
            state.table_cell_buf.clear();
        }
        _ => {}
    }
}

fn handle_end_tag(state: &mut RenderState, tag: &TagEnd) {
    match tag {
        TagEnd::Heading(_level) => {
            state.flush_wrapped();
            state.in_heading = None;
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
                state.emit("");
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
                let text = state
                    .inline_buf()
                    .get(link.text_start..)
                    .map(strip_ansi)
                    .unwrap_or_default();
                let text = text.trim();
                let redundant = text == link.url
                    || link.url.strip_prefix("mailto:") == Some(text)
                    || link.url.is_empty();
                if link.show_url && !redundant {
                    let url_display =
                        style::styled(&format!(" ({})", link.url), &[style::DIM], state.use_color);
                    state.inline_buf().push_str(&url_display);
                }
            }
        }
        TagEnd::HtmlBlock => {
            let html = state.html_block.take().unwrap_or_default();
            let text = strip_html(&html);
            let words: Vec<String> = text
                .split_whitespace()
                .map(|w| style::styled(w, &[style::DIM], state.use_color))
                .collect();
            if !words.is_empty() {
                state.push_blank();
                state.current_line = words.join(" ");
                state.flush_wrapped();
            }
        }
        TagEnd::Image => {
            if let Some(start) = state.image_start.take() {
                let use_color = state.use_color;
                let buf = state.inline_buf();
                let alt = buf.get(start..).map(strip_ansi).unwrap_or_default();
                if alt.trim_end() == "[image:" {
                    // No alt text.
                    buf.truncate(start);
                    buf.push_str(&style::styled("[image]", &[style::DIM], use_color));
                } else {
                    buf.push_str(&style::styled("]", &[style::DIM], use_color));
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
            state
                .table_row
                .push(std::mem::take(&mut state.table_cell_buf));
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

fn handle_code(state: &mut RenderState, code: &str) {
    let styled = if state.use_color {
        format!("{} {} {}", style::BG_GREY, code, style::RESET)
    } else {
        format!("`{}`", code)
    };
    state.inline_buf().push_str(&styled);
}

fn handle_soft_break(state: &mut RenderState) {
    state.inline_buf().push(' ');
}

/// Expand tabs to 4-column tab stops.
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
            col += 1;
        }
    }
    out
}

/// Emit a buffered code block: one row per source line, tabs expanded,
/// over-wide lines truncated with `…` (code is never word-wrapped).
fn render_code_block(state: &mut RenderState, code: &str) {
    let border = if state.use_color {
        format!("{}  │ {}", style::DIM, style::RESET)
    } else {
        "  | ".to_string()
    };
    let available = state
        .width
        .saturating_sub(state.prefix_width())
        .saturating_sub(4);
    for line in code.lines() {
        let line = expand_tabs(line);
        let line = if available > 0 && line.chars().count() > available {
            let mut cut: String = line.chars().take(available - 1).collect();
            cut.push('…');
            cut
        } else {
            line
        };
        state.emit(&format!("{}{}", border, line));
    }
    let label = style::styled("  ╰───", &[style::DIM], state.use_color);
    state.emit(&label);
}

fn handle_hard_break(state: &mut RenderState) {
    state.flush_wrapped();
}

fn handle_rule(state: &mut RenderState) {
    state.push_blank();
    let rule: String = "─".repeat(state.width.saturating_sub(state.prefix_width()));
    let styled_rule = style::styled(&rule, &[style::DIM], state.use_color);
    state.emit(&styled_rule);
    state.push_blank();
}

fn handle_task_marker(state: &mut RenderState, checked: bool) {
    let marker = if checked {
        style::styled("[✓]", &[style::FG_GREEN, style::BOLD], state.use_color)
    } else {
        style::styled("[ ]", &[style::DIM], state.use_color)
    };
    state.inline_buf().push_str(&format!("{} ", marker));
}

fn render_table(state: &mut RenderState) {
    if state.table_rows.is_empty() {
        return;
    }

    let num_cols = state.table_rows.iter().map(|r| r.len()).max().unwrap_or(0);
    if num_cols == 0 {
        return;
    }

    // Calculate natural column widths using visible length (ignoring ANSI codes)
    let mut col_widths = vec![0usize; num_cols];
    for row in &state.table_rows {
        for (i, cell) in row.iter().enumerate() {
            if i < num_cols {
                col_widths[i] = col_widths[i].max(style::visible_len(cell));
            }
        }
    }

    for w in &mut col_widths {
        *w = (*w).max(3);
    }

    // Shrink columns to fit within terminal width
    // Table overhead: 2 (margin) + 1 (left border) + 1 (right border) + 2*num_cols (spaces) + (num_cols-1) (mid borders)
    // = 3 + 3*num_cols
    let overhead = 3 + 3 * num_cols;
    let available = state.width.saturating_sub(state.prefix_width());
    if available < overhead + 3 * num_cols {
        // Not even 3 columns per cell fit: lay the rows out vertically.
        render_table_vertical(state, available);
        return;
    }
    let max_content = available - overhead;
    let total: usize = col_widths.iter().sum();

    if total > max_content {
        let min_col_width = 3usize;
        let guaranteed = min_col_width * num_cols;

        if max_content <= guaranteed {
            col_widths.fill(min_col_width);
        } else {
            // Fair-share: settle columns that fit naturally, redistribute rest
            let mut remaining = max_content;
            let mut settled = vec![false; num_cols];
            let mut settled_count = 0;

            loop {
                let unsettled = num_cols - settled_count;
                if unsettled == 0 {
                    break;
                }
                let fair_share = remaining / unsettled;
                let mut any_settled = false;

                for i in 0..num_cols {
                    if settled[i] {
                        continue;
                    }
                    if col_widths[i] <= fair_share {
                        settled[i] = true;
                        settled_count += 1;
                        remaining -= col_widths[i];
                        any_settled = true;
                    }
                }

                if !any_settled {
                    // All remaining columns exceed fair_share — distribute evenly
                    let share = remaining / unsettled;
                    let mut extra = remaining % unsettled;
                    for i in 0..num_cols {
                        if !settled[i] {
                            col_widths[i] = share
                                + if extra > 0 {
                                    extra -= 1;
                                    1
                                } else {
                                    0
                                };
                        }
                    }
                    break;
                }
            }

            for w in &mut col_widths {
                *w = (*w).max(min_col_width);
            }
        }
    }

    let no_wrap = state.no_wrap;
    let use_color = state.use_color;

    let draw_separator =
        |state: &mut RenderState, left: &str, mid: &str, right: &str, fill: &str| {
            let mut line = format!("  {}", left);
            for (i, w) in col_widths.iter().enumerate() {
                line.push_str(&fill.repeat(*w + 2));
                if i < num_cols - 1 {
                    line.push_str(mid);
                }
            }
            line.push_str(right);
            state.emit(&style::styled(&line, &[style::DIM], state.use_color));
        };

    draw_separator(state, "┌", "┬", "┐", "─");

    let rows = std::mem::take(&mut state.table_rows);
    let alignments = std::mem::take(&mut state.table_alignments);
    for (row_idx, row) in rows.iter().enumerate() {
        // Wrap each cell to its allocated column width
        let mut wrapped_cells: Vec<Vec<String>> = row
            .iter()
            .enumerate()
            .map(|(i, cell)| {
                let w = col_widths.get(i).copied().unwrap_or(3);
                wrap_cell_text(cell, w, no_wrap, use_color)
            })
            .collect();

        // Pad to num_cols if row has fewer cells
        while wrapped_cells.len() < num_cols {
            wrapped_cells.push(vec![String::new()]);
        }

        // Determine tallest cell in this row
        let max_lines = wrapped_cells.iter().map(|c| c.len()).max().unwrap_or(1);

        // Emit each sub-line of the row
        for line_idx in 0..max_lines {
            let mut line = String::new();
            if use_color {
                line.push_str(&format!("  {}│{} ", style::DIM, style::RESET));
            } else {
                line.push_str("  | ");
            }

            for (i, cell_lines) in wrapped_cells.iter().enumerate() {
                let w = col_widths.get(i).copied().unwrap_or(3);
                let cell_text = cell_lines.get(line_idx).map(|s| s.as_str()).unwrap_or("");

                let align = alignments.get(i).copied().unwrap_or(Alignment::None);
                let padded = pad_cell(cell_text, w, align);
                let styled_cell = if row_idx == 0 {
                    bold_cell(&padded, use_color)
                } else {
                    padded
                };

                line.push_str(&styled_cell);

                if i < num_cols - 1 {
                    if use_color {
                        line.push_str(&format!(" {}│{} ", style::DIM, style::RESET));
                    } else {
                        line.push_str(" | ");
                    }
                }
            }

            if use_color {
                line.push_str(&format!(" {}│{}", style::DIM, style::RESET));
            } else {
                line.push_str(" |");
            }
            state.emit(&line);
        }

        if row_idx == 0 {
            draw_separator(state, "├", "┼", "┤", "─");
        }
    }

    draw_separator(state, "└", "┴", "┘", "─");
}

/// Pad a (possibly styled) cell to `width` columns according to `align`.
fn pad_cell(text: &str, width: usize, align: Alignment) -> String {
    let pad = width.saturating_sub(style::visible_len(text));
    let (left, right) = match align {
        Alignment::Right => (pad, 0),
        Alignment::Center => (pad / 2, pad - pad / 2),
        Alignment::Left | Alignment::None => (0, pad),
    };
    format!("{}{}{}", " ".repeat(left), text, " ".repeat(right))
}

/// Bold a header cell, re-opening bold after any inline style reset inside it.
fn bold_cell(text: &str, use_color: bool) -> String {
    if !use_color {
        return text.to_string();
    }
    let rebold = format!("{}{}", style::RESET, style::BOLD);
    style::styled(&text.replace(style::RESET, &rebold), &[style::BOLD], true)
}

/// Fallback for tables too wide for the terminal: each body row becomes a
/// block of `header: value` lines, with rows separated by a dim rule.
fn render_table_vertical(state: &mut RenderState, available: usize) {
    let rows = std::mem::take(&mut state.table_rows);
    state.table_alignments.clear();
    let (no_wrap, use_color) = (state.no_wrap, state.use_color);
    let Some((headers, body)) = rows.split_first() else {
        return;
    };
    let rule = style::styled(&"─".repeat(available), &[style::DIM], use_color);

    let label_width = headers
        .iter()
        .map(|h| style::visible_len(h))
        .max()
        .unwrap_or(0)
        .min(available / 2);

    state.emit(&rule);
    if body.is_empty() {
        for header in headers {
            state.emit(&bold_cell(header, use_color));
        }
        state.emit(&rule);
        return;
    }
    for row in body {
        for (i, header) in headers.iter().enumerate() {
            let value = row.get(i).map(String::as_str).unwrap_or("");
            let label = pad_cell(&format!("{}:", header), label_width + 1, Alignment::Left);
            let label = bold_cell(&label, use_color);
            let label_visible = style::visible_len(&label) + 1;
            let value_width = available.saturating_sub(label_visible);
            if value_width >= 8 {
                let lines = wrap_cell_text(value, value_width, no_wrap, use_color);
                for (n, line) in lines.iter().enumerate() {
                    if n == 0 {
                        state.emit(&format!("{} {}", label, line));
                    } else {
                        state.emit(&format!("{}{}", " ".repeat(label_visible), line));
                    }
                }
            } else {
                // Too narrow for side-by-side: value below its label.
                state.emit(&label);
                let width = available.saturating_sub(2);
                for line in wrap_cell_text(value, width, no_wrap, use_color) {
                    state.emit(&format!("  {}", line));
                }
            }
        }
        state.emit(&rule);
    }
}

#[cfg(test)]
mod snapshot_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser;

    #[test]
    fn test_heading_renders() {
        let events = parser::parse("# Hello World");
        let lines = render(events, 80, false, false);
        assert!(lines.iter().any(|l| l.contains("Hello World")));
    }

    #[test]
    fn test_bold_renders() {
        let events = parser::parse("**bold text**");
        let lines = render(events, 80, true, false);
        let joined = lines.join("");
        assert!(joined.contains("bold text"));
    }

    #[test]
    fn test_inline_formatting_stays_on_one_line() {
        let events = parser::parse("This is **bold** and *italic* text.");
        let lines = render(events, 80, false, false);
        let content_lines: Vec<&String> = lines.iter().filter(|l| !l.is_empty()).collect();
        assert_eq!(
            content_lines.len(),
            1,
            "Expected 1 content line, got: {:?}",
            content_lines
        );
        assert!(content_lines[0].contains("bold"));
        assert!(content_lines[0].contains("italic"));
    }

    #[test]
    fn test_list_renders() {
        let events = parser::parse("- item one\n- item two");
        let lines = render(events, 80, false, false);
        assert!(lines.iter().any(|l| l.contains("item one")));
        assert!(lines.iter().any(|l| l.contains("item two")));
    }

    #[test]
    fn test_code_block_renders() {
        let events = parser::parse("```\nlet x = 1;\n```");
        let lines = render(events, 80, false, false);
        assert!(lines.iter().any(|l| l.contains("let x = 1;")));
    }

    #[test]
    fn test_horizontal_rule() {
        let events = parser::parse("---");
        let lines = render(events, 40, false, false);
        assert!(lines.iter().any(|l| l.contains("─")));
    }

    #[test]
    fn test_word_wrap() {
        let events = parser::parse("This is a very long line that should be wrapped when the terminal width is narrow enough to require it.");
        let lines = render(events, 40, false, false);
        let content_lines: Vec<&String> = lines.iter().filter(|l| !l.is_empty()).collect();
        assert!(content_lines.len() > 1, "Long text should wrap");
        for line in &content_lines {
            assert!(
                style::visible_len(line) <= 40,
                "No line should exceed width"
            );
        }
    }

    #[test]
    fn test_no_wrap_truncates_with_ellipsis() {
        let events = parser::parse(
            "This is a very long line that should be truncated when no-wrap mode is enabled.",
        );
        let lines = render(events, 30, false, true);
        let content_lines: Vec<&String> = lines.iter().filter(|l| !l.is_empty()).collect();
        assert_eq!(content_lines.len(), 1, "No-wrap should produce one line");
        assert!(
            content_lines[0].ends_with('…'),
            "Truncated line should end with ellipsis"
        );
        assert!(
            style::visible_len(content_lines[0]) <= 30,
            "Line should not exceed width"
        );
    }

    #[test]
    fn test_table_respects_terminal_width() {
        let md = "| Short | This column has a very long text that should cause wrapping when the terminal is narrow enough |\n|---|---|\n| a | More long text that definitely exceeds the terminal width for testing purposes |";
        let events = parser::parse(md);
        let lines = render(events, 50, false, false);
        for line in &lines {
            assert!(
                style::visible_len(line) <= 50,
                "Table line exceeds width ({}): '{}'",
                style::visible_len(line),
                line
            );
        }
    }

    #[test]
    fn test_table_multiline_cells() {
        let md = "| A | B |\n|---|---|\n| short | This is a long cell that should definitely wrap to multiple lines within the table row |\n";
        let events = parser::parse(md);
        let lines = render(events, 40, false, false);
        // Count data row lines (lines with | between header separator and bottom border)
        let table_lines: Vec<&String> = lines.iter().filter(|l| l.contains('|')).collect();
        // Should have: header + at least 2 lines for the data row (since cell wraps)
        assert!(
            table_lines.len() > 3,
            "Expected multi-line row, got {} table lines: {:?}",
            table_lines.len(),
            table_lines
        );
    }

    #[test]
    fn test_wrap_cell_text_basic() {
        let lines = wrap_cell_text("short text", 20, false, false);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0], "short text");
    }

    #[test]
    fn test_wrap_cell_text_wraps() {
        let lines = wrap_cell_text(
            "this is a longer text that needs wrapping",
            15,
            false,
            false,
        );
        assert!(lines.len() > 1, "Should wrap: {:?}", lines);
        for line in &lines {
            assert!(
                style::visible_len(line) <= 15,
                "Wrapped line too wide: '{}'",
                line
            );
        }
    }

    fn plain(md: &str, width: u16) -> Vec<String> {
        render(parser::parse(md), width, false, false)
    }

    #[test]
    fn test_no_leading_blank_line() {
        assert_eq!(plain("hello", 80), vec!["hello"]);
    }

    #[test]
    fn test_loose_list_marker_stays_with_text() {
        assert_eq!(plain("- one\n\n- two", 80), vec!["  • one", "", "  • two"]);
    }

    #[test]
    fn test_nested_list_parent_at_parent_indent() {
        assert_eq!(
            plain("- parent\n  - child\n- next", 80),
            vec!["  • parent", "    ◦ child", "  • next"]
        );
    }

    #[test]
    fn test_list_in_quote_single_border() {
        assert_eq!(plain("> - item", 80), vec!["  |   • item"]);
    }

    #[test]
    fn test_no_wrap_list_indent_not_doubled() {
        let lines = render(parser::parse("- item\n  - nested"), 80, false, true);
        assert_eq!(lines, vec!["  • item", "    ◦ nested"]);
    }

    #[test]
    fn test_nested_blockquotes() {
        assert_eq!(
            plain("> outer\n>\n> > inner\n>\n> back", 80),
            vec!["  | outer", "  |", "  | | inner", "  |", "  | back"]
        );
    }

    #[test]
    fn test_wrapped_list_item_hanging_indent() {
        assert_eq!(
            plain("- aaaa bbbb cccc dddd", 15),
            vec!["  • aaaa bbbb", "    cccc dddd"]
        );
    }

    #[test]
    fn test_code_block_no_trailing_or_doubled_blank_rows() {
        assert_eq!(
            plain("```\na\nb\n```", 80),
            vec!["  ╭───", "  | a", "  | b", "  ╰───"]
        );
        // Inside a list item (one Text event per line).
        assert_eq!(
            plain("- x\n\n      a\n      b", 80),
            vec![
                "  • x",
                "",
                "      ╭───",
                "      | a",
                "      | b",
                "      ╰───"
            ]
        );
    }

    #[test]
    fn test_code_block_in_quote_keeps_border() {
        assert_eq!(
            plain("> ```\n> a\n> ```", 80),
            vec!["  |   ╭───", "  |   | a", "  |   ╰───"]
        );
    }

    #[test]
    fn test_code_block_expands_tabs_and_truncates() {
        assert_eq!(plain("```\n\tx\ty\n```", 80)[1], "  |     x   y");
        let lines = plain("```\n0123456789abcdef\n```", 16);
        assert_eq!(lines[1], "  | 012345678…");
        assert!(lines.iter().all(|l| style::visible_len(l) <= 14));
    }

    /// Every ESC in `s` must start a renderer-generated SGR sequence.
    fn only_sgr_escapes(s: &str) -> bool {
        let mut rest = s;
        while let Some(i) = rest.find('\x1b') {
            rest = &rest[i + 1..];
            let Some(body) = rest.strip_prefix('[') else {
                return false;
            };
            let end = body.find(|c: char| !(c.is_ascii_digit() || c == ';'));
            match end {
                Some(e) if body[e..].starts_with('m') => rest = &body[e + 1..],
                _ => return false,
            }
        }
        true
    }

    #[test]
    fn test_control_characters_are_neutralised() {
        let md = "Evil \x1b]0;title\x07 and \x1b[31mred\x1b[0m\r\x7f `\x1b[2J` \
                  [x](http://a\x1b/) \u{9b}31m\n\n```\x1b]52\n\x1b[1mcode\x07\n```";
        let plain_out = render(parser::parse(md), 80, false, false).join("\n");
        assert!(
            !plain_out
                .chars()
                .any(|c| c.is_control() && c != '\n' && c != '\t'),
            "control chars leaked: {:?}",
            plain_out
        );
        let color_out = render(parser::parse(md), 80, true, false).join("\n");
        assert!(only_sgr_escapes(&color_out), "{:?}", color_out);
        assert!(!color_out.contains('\x07') && !color_out.contains('\u{9b}'));
    }

    #[test]
    fn test_link_in_table_cell_stays_in_cell() {
        let md = "| A | B |\n|---|---|\n| [l](https://x.io) | `c` **b** |\n\nafter";
        let lines = plain(md, 80);
        assert!(lines.iter().any(|l| l.contains("| l (https://x.io) |")));
        assert!(lines.iter().any(|l| l.contains("`c` b")));
        assert_eq!(lines.last().unwrap(), "after");
        assert!(!lines.iter().any(|l| l.trim() == "(https://x.io)"));
        let color = render(parser::parse(md), 80, true, false).join("\n");
        assert!(color.contains(&format!("{}b{}", style::BOLD, style::RESET)));
    }

    #[test]
    fn test_table_alignment() {
        let md = "| Left | Center | Right |\n|:--|:-:|--:|\n| a | b | c |";
        let lines = plain(md, 80);
        assert_eq!(lines[1], "  | Left | Center | Right |");
        assert_eq!(lines[3], "  | a    |   b    |     c |");
    }

    #[test]
    fn test_too_wide_table_falls_back_to_vertical() {
        let md = "| a | bb | c |\n|---|---|---|\n| 1 | 2 | 3 |\n| 4 | 5 | 6 |";
        let lines = plain(md, 18);
        assert_eq!(
            lines,
            vec![
                "────────────────",
                "a:  1",
                "bb: 2",
                "c:  3",
                "────────────────",
                "a:  4",
                "bb: 5",
                "c:  6",
                "────────────────",
            ]
        );
        assert!(lines.iter().all(|l| style::visible_len(l) <= 18));
    }

    #[test]
    fn test_yaml_front_matter_skipped() {
        assert_eq!(
            plain("---\ntitle: x\n---\n\n# Head\n\nbody", 80),
            vec!["Head", "", "body"]
        );
    }

    #[test]
    fn test_footnotes() {
        assert_eq!(
            plain("Text[^1].\n\n[^1]: The body.", 80),
            vec!["Text[1].", "", "[1]: The body."]
        );
        let color = render(parser::parse("a[^n]\n\n[^n]: b"), 80, true, false);
        assert!(color[0].contains(&format!("{}[n]{}", style::DIM, style::RESET)));
        assert!(color[2].starts_with(&format!("{}[n]: {}", style::DIM, style::RESET)));
    }

    #[test]
    fn test_inline_html() {
        assert_eq!(
            plain("a<br>b <BR/> c <kbd>Ctrl</kbd>+<kbd>C</kbd>", 80),
            vec!["a", "b", "c Ctrl+C"]
        );
    }

    #[test]
    fn test_html_block_text_kept() {
        assert_eq!(
            plain(
                "<details>\n<summary>Click &amp; see</summary>\n\nbody\n</details>\n\n<!-- hidden -->",
                80
            ),
            vec!["Click & see", "", "body"]
        );
        assert_eq!(strip_html("a < b <i>c</i>"), "a < b c");
        let color = render(parser::parse("<p>x y</p>"), 80, true, false);
        assert_eq!(
            color,
            vec![format!("{d}x{r} {d}y{r}", d = style::DIM, r = style::RESET)]
        );
    }

    #[test]
    fn test_images() {
        assert_eq!(
            plain("![Alt text](a.png) ![](b.png)", 80),
            vec!["[image: Alt text] [image]"]
        );
        let color = render(parser::parse("![a](a.png)"), 80, true, false);
        let (d, r) = (style::DIM, style::RESET);
        assert_eq!(color, vec![format!("{d}[image: {r}{d}a{r}{d}]{r}")]);
    }

    #[test]
    fn test_autolink_url_not_repeated() {
        assert_eq!(
            plain("<https://a.io> <me@x.org> [https://b.io](https://b.io)", 80),
            vec!["https://a.io me@x.org https://b.io"]
        );
        assert_eq!(plain("[t](https://a.io)", 80), vec!["t (https://a.io)"]);
    }

    #[test]
    fn test_link_text_underlined() {
        let lines = render(parser::parse("[t](https://a.io)"), 80, true, false);
        assert!(lines[0].starts_with(&format!("{}t{}", style::UNDERLINE, style::RESET)));
    }

    #[test]
    fn test_empty_list_item_keeps_marker() {
        assert_eq!(plain("-\n- x", 80), vec!["  • ", "  • x"]);
    }
}
