//! Converts pulldown-cmark events into ANSI-styled, word-wrapped terminal lines.

use pulldown_cmark::{CodeBlockKind, Event, Tag, TagEnd};

use crate::style;

struct RenderState {
    use_color: bool,
    no_wrap: bool,
    width: usize,
    lines: Vec<String>,
    current_line: String,
    indent: usize,
    bold: bool,
    italic: bool,
    strikethrough: bool,
    in_code_block: bool,
    in_blockquote: bool,
    in_heading: Option<u8>,
    list_stack: Vec<ListContext>,
    link_url: Option<String>,
    table_row: Vec<String>,
    table_cell_buf: String,
    table_alignments: Vec<pulldown_cmark::Alignment>,
    table_rows: Vec<Vec<String>>,
    in_table_head: bool,
    in_table_cell: bool,
}

#[derive(Clone)]
enum ListContext {
    Unordered(usize), // depth
    Ordered(u64),     // current number
}

impl RenderState {
    fn new(width: u16, use_color: bool, no_wrap: bool) -> Self {
        Self {
            use_color,
            no_wrap,
            width: width.saturating_sub(2) as usize, // margin
            lines: Vec::new(),
            current_line: String::new(),
            indent: 0,
            bold: false,
            italic: false,
            strikethrough: false,
            in_code_block: false,
            in_blockquote: false,
            in_heading: None,
            list_stack: Vec::new(),
            link_url: None,
            table_row: Vec::new(),
            table_cell_buf: String::new(),
            table_alignments: Vec::new(),
            table_rows: Vec::new(),
            in_table_head: false,
            in_table_cell: false,
        }
    }

    fn push_line(&mut self, line: &str) {
        self.lines.push(line.to_string());
    }

    fn push_blank(&mut self) {
        // Flush any pending content first
        self.flush_wrapped();
        if self.lines.last().is_some_and(|l| !l.is_empty()) {
            self.lines.push(String::new());
        }
    }

    /// Flush `current_line` with word wrapping (or truncation in no_wrap mode).
    fn flush_wrapped(&mut self) {
        if self.current_line.is_empty() {
            return;
        }
        let text = std::mem::take(&mut self.current_line);
        let prefix = self.indent_prefix();
        let prefix_visible_len = style::visible_len(&prefix);
        let available = self.width.saturating_sub(prefix_visible_len);

        if available == 0 {
            self.lines.push(format!("{}{}", prefix, text));
            return;
        }

        if self.no_wrap {
            // Truncate mode: single line, add ellipsis if it exceeds width
            let full = format!("{}{}", prefix, text);
            let visible = style::visible_len(&full);
            if visible <= self.width {
                self.lines.push(full);
            } else {
                let truncated =
                    truncate_styled(&full, self.width.saturating_sub(1), self.use_color);
                self.lines.push(truncated);
            }
            return;
        }

        // Word-wrap mode
        let segments = split_styled_words(&text);

        let mut line_buf = prefix.clone();
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
                self.lines.push(line_buf);
                line_buf = format!("{}{}", prefix, seg);
                line_visible = seg_visible;
            }
        }

        if line_visible > 0 || !line_buf.is_empty() {
            self.lines.push(line_buf);
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
        style::combine(&codes)
    }

    fn current_style_suffix(&self) -> String {
        if !self.use_color {
            String::new()
        } else if self.bold || self.italic || self.strikethrough || self.in_heading.is_some() {
            style::RESET.to_string()
        } else {
            String::new()
        }
    }

    fn indent_prefix(&self) -> String {
        let mut prefix = String::new();
        if self.in_blockquote {
            if self.use_color {
                prefix.push_str(&format!("{}  │ {}", style::DIM, style::RESET));
            } else {
                prefix.push_str("  | ");
            }
        }
        if self.indent > 0 {
            prefix.push_str(&" ".repeat(self.indent));
        }
        prefix
    }
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
        match event {
            Event::Start(tag) => handle_start_tag(&mut state, &tag),
            Event::End(tag) => handle_end_tag(&mut state, &tag),
            Event::Text(text) => handle_text(&mut state, &text),
            Event::Code(code) => handle_code(&mut state, &code),
            Event::SoftBreak => handle_soft_break(&mut state),
            Event::HardBreak => handle_hard_break(&mut state),
            Event::Rule => handle_rule(&mut state),
            Event::TaskListMarker(checked) => handle_task_marker(&mut state, checked),
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
            if !state.in_code_block {
                state.push_blank();
            }
        }
        Tag::BlockQuote(_) => {
            state.in_blockquote = true;
            state.push_blank();
        }
        Tag::CodeBlock(kind) => {
            state.in_code_block = true;
            state.push_blank();
            if let CodeBlockKind::Fenced(lang) = kind {
                if !lang.is_empty() {
                    let label =
                        style::styled(&format!("  ╭─ {} ", lang), &[style::DIM], state.use_color);
                    state.push_line(&label);
                } else {
                    let label = style::styled("  ╭───", &[style::DIM], state.use_color);
                    state.push_line(&label);
                }
            } else {
                let label = style::styled("  ╭───", &[style::DIM], state.use_color);
                state.push_line(&label);
            }
        }
        Tag::List(first) => {
            if state.list_stack.is_empty() {
                state.push_blank();
            }
            let depth = state.list_stack.len();
            match first {
                Some(start) => state.list_stack.push(ListContext::Ordered(*start)),
                None => state.list_stack.push(ListContext::Unordered(depth)),
            }
            state.indent = (depth + 1) * 2;
        }
        Tag::Item => {
            state.flush_wrapped();
            let prefix = state.indent_prefix();
            let marker = match state.list_stack.last() {
                Some(ListContext::Unordered(depth)) => match depth {
                    0 => "• ".to_string(),
                    1 => "◦ ".to_string(),
                    _ => "▪ ".to_string(),
                },
                Some(ListContext::Ordered(num)) => {
                    let s = format!("{}. ", num);
                    if let Some(ListContext::Ordered(n)) = state.list_stack.last_mut() {
                        *n += 1;
                    }
                    s
                }
                None => "• ".to_string(),
            };
            let styled_marker = if state.use_color {
                style::styled(&marker, &[style::FG_CYAN], state.use_color)
            } else {
                marker
            };
            state.current_line = format!("{}{}", prefix, styled_marker);
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
        Tag::Link { dest_url, .. } => {
            state.link_url = Some(dest_url.to_string());
        }
        Tag::Table(alignments) => {
            state.push_blank();
            state.table_alignments = alignments.clone();
            state.table_rows.clear();
        }
        Tag::TableHead => {
            state.in_table_head = true;
            state.table_row.clear();
        }
        Tag::TableRow => {
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
            state.in_blockquote = false;
        }
        TagEnd::CodeBlock => {
            let label = style::styled("  ╰───", &[style::DIM], state.use_color);
            state.push_line(&label);
            state.in_code_block = false;
        }
        TagEnd::List(_) => {
            state.list_stack.pop();
            state.indent = state.list_stack.len() * 2;
            if state.list_stack.is_empty() {
                state.push_blank();
            }
        }
        TagEnd::Item => {
            state.flush_wrapped();
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
            if let Some(url) = state.link_url.take() {
                let url_display =
                    style::styled(&format!(" ({})", url), &[style::DIM], state.use_color);
                state.current_line.push_str(&url_display);
            }
        }
        TagEnd::Table => {
            render_table(state);
        }
        TagEnd::TableHead => {
            state.in_table_head = false;
            state.table_rows.push(state.table_row.clone());
        }
        TagEnd::TableRow => {
            state.table_rows.push(state.table_row.clone());
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
    if state.in_code_block {
        for line in text.split('\n') {
            let formatted = if state.use_color {
                format!("{}  │ {}{}", style::DIM, style::RESET, line)
            } else {
                format!("  | {}", line)
            };
            state.push_line(&formatted);
        }
        return;
    }

    if state.in_table_cell {
        state.table_cell_buf.push_str(text);
        return;
    }

    // Accumulate styled text into current_line. Word wrapping happens at flush.
    let style_pre = state.current_style_prefix();
    let style_suf = state.current_style_suffix();
    state
        .current_line
        .push_str(&format!("{}{}{}", style_pre, text, style_suf));
}

fn handle_code(state: &mut RenderState, code: &str) {
    if state.in_table_cell {
        state.table_cell_buf.push_str(code);
        return;
    }
    let styled = if state.use_color {
        format!("{} {} {}", style::BG_GREY, code, style::RESET)
    } else {
        format!("`{}`", code)
    };
    state.current_line.push_str(&styled);
}

fn handle_soft_break(state: &mut RenderState) {
    if !state.in_code_block {
        state.current_line.push(' ');
    }
}

fn handle_hard_break(state: &mut RenderState) {
    state.flush_wrapped();
}

fn handle_rule(state: &mut RenderState) {
    state.push_blank();
    let rule: String = "─".repeat(state.width);
    let styled_rule = style::styled(&rule, &[style::DIM], state.use_color);
    state.push_line(&styled_rule);
    state.push_blank();
}

fn handle_task_marker(state: &mut RenderState, checked: bool) {
    let marker = if checked {
        style::styled("[✓]", &[style::FG_GREEN, style::BOLD], state.use_color)
    } else {
        style::styled("[ ]", &[style::DIM], state.use_color)
    };
    state.current_line.push_str(&format!("{} ", marker));
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
    let max_content = state.width.saturating_sub(overhead);
    let total: usize = col_widths.iter().sum();

    if total > max_content && max_content > 0 {
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
            state.push_line(&style::styled(&line, &[style::DIM], state.use_color));
        };

    draw_separator(state, "┌", "┬", "┐", "─");

    for (row_idx, row) in state.table_rows.clone().iter().enumerate() {
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

                // Pad to column width based on visible length
                let visible = style::visible_len(cell_text);
                let pad = w.saturating_sub(visible);
                let padded = format!("{}{}", cell_text, " ".repeat(pad));

                let styled_cell = if row_idx == 0 {
                    style::styled(&padded, &[style::BOLD], use_color)
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
            state.push_line(&line);
        }

        if row_idx == 0 {
            draw_separator(state, "├", "┼", "┤", "─");
        }
    }

    draw_separator(state, "└", "┴", "┘", "─");

    state.table_rows.clear();
    state.table_alignments.clear();
}

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
}
