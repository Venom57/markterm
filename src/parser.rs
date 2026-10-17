//! Markdown parsing wrapper around pulldown-cmark.

use pulldown_cmark::{Event, Options, Parser};

/// Parse markdown content and return an owned vector of events.
pub fn parse(content: &str) -> Vec<Event<'_>> {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_TASKLISTS);

    Parser::new_ext(content, options).collect()
}
