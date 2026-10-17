//! Markdown parsing wrapper around pulldown-cmark.

use pulldown_cmark::{Event, Options, Parser};

/// Parse markdown content into a stream of events. The renderer consumes
/// the stream directly; only ordered lists are buffered (to count their
/// items for number alignment).
pub fn parse(content: &str) -> impl Iterator<Item = Event<'_>> {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_TASKLISTS);
    options.insert(Options::ENABLE_FOOTNOTES);
    options.insert(Options::ENABLE_YAML_STYLE_METADATA_BLOCKS);

    Parser::new_ext(content, options)
}
