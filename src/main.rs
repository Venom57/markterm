mod pager;
mod parser;
mod renderer;
mod style;
mod terminal;

use clap::Parser;
use std::fs;
use std::io::{self, IsTerminal, Read};
use std::process;

#[derive(Parser)]
#[command(
    name = "markterm",
    version,
    about = "Render markdown in the terminal with built-in paging"
)]
struct Cli {
    /// Markdown file to display (use - for stdin)
    file: String,

    /// Override terminal width
    #[arg(short, long)]
    width: Option<u16>,

    /// Color theme: auto, dark, light, none
    #[arg(short, long, default_value = "auto")]
    theme: String,

    /// Dump rendered output without paging
    #[arg(long)]
    no_pager: bool,

    /// Truncate long lines with ellipsis instead of wrapping
    #[arg(long)]
    no_wrap: bool,
}

fn main() {
    let cli = Cli::parse();

    // Read input
    let content = if cli.file == "-" {
        let mut buf = String::new();
        io::stdin().read_to_string(&mut buf).unwrap_or_else(|e| {
            eprintln!("markterm: error reading stdin: {}", e);
            process::exit(1);
        });
        buf
    } else {
        fs::read_to_string(&cli.file).unwrap_or_else(|e| {
            eprintln!("markterm: {}: {}", cli.file, e);
            process::exit(1);
        })
    };

    // Determine terminal dimensions
    let (term_width, term_height) = terminal::size();
    let width = cli.width.unwrap_or(term_width);

    // Determine if we should use color
    let use_color = match cli.theme.as_str() {
        "none" => false,
        _ => {
            // Respect NO_COLOR env var
            std::env::var("NO_COLOR").is_err()
        }
    };

    // Parse and render
    let events = parser::parse(&content);
    let lines = renderer::render(events, width, use_color, cli.no_wrap);

    // Output
    let is_tty = io::stdout().is_terminal();
    if cli.no_pager || !is_tty {
        // Dump to stdout
        for line in &lines {
            println!("{}", line);
        }
    } else {
        // Interactive pager
        let filename = if cli.file == "-" {
            "(stdin)".to_string()
        } else {
            cli.file.clone()
        };
        if let Err(e) = pager::run(&lines, term_height, &filename) {
            eprintln!("markterm: pager error: {}", e);
            process::exit(1);
        }
    }
}
