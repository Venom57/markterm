mod pager;
mod parser;
mod renderer;
mod style;
mod terminal;
mod theme;

use clap::{CommandFactory, Parser, ValueEnum};
use std::ffi::OsString;
use std::fs;
use std::io::{self, BufWriter, IsTerminal, Read, Write};
use std::process;

/// Colour theme selected with `--theme`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum Theme {
    /// Detect the terminal background (falls back to dark)
    Auto,
    /// Palette for dark terminal backgrounds
    Dark,
    /// Palette for light terminal backgrounds
    Light,
    /// No colours or text attributes at all
    None,
}

/// When to emit colour / text attribute escape codes (`--color`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum ColorChoice {
    /// Colour only when stdout is a terminal
    Auto,
    /// Always emit colour, even when piped
    Always,
    /// Never emit colour
    Never,
}

#[derive(Parser)]
#[command(
    name = "markterm",
    version,
    about = "Render markdown in the terminal with built-in paging"
)]
struct Cli {
    /// Markdown file to display (use - for stdin; defaults to stdin when it is piped)
    file: Option<String>,

    /// Override terminal width
    #[arg(short, long, value_name = "COLS")]
    width: Option<u16>,

    /// Color theme
    #[arg(short, long, value_enum, default_value_t = Theme::Auto)]
    theme: Theme,

    /// When to use colour
    #[arg(long, value_enum, value_name = "WHEN", default_value_t = ColorChoice::Auto)]
    color: ColorChoice,

    /// Dump rendered output without paging
    #[arg(long)]
    no_pager: bool,

    /// Truncate long lines with ellipsis instead of wrapping
    #[arg(long)]
    no_wrap: bool,
}

/// Everything the renderer needs besides the width, so the pager can
/// re-render at a new width after a terminal resize.
struct RenderOptions {
    /// Selected theme, with `auto` normally resolved to dark or light.
    theme: Theme,
    use_color: bool,
    no_wrap: bool,
}

impl RenderOptions {
    fn render(&self, content: &str, width: u16) -> Vec<String> {
        let events = parser::parse(content);
        renderer::render(events, width, &self.palette(), self.no_wrap)
    }

    /// The renderer's styles. `auto` left unresolved (detection skipped)
    /// uses the dark palette.
    fn palette(&self) -> theme::Theme {
        if !self.use_color {
            return theme::Theme::none();
        }
        let italic = theme::supports_italic(std::env::var("TERM").ok().as_deref());
        match self.theme {
            Theme::Light => theme::Theme::light(italic),
            Theme::Auto | Theme::Dark | Theme::None => theme::Theme::dark(italic),
        }
    }
}

/// Where the markdown comes from.
#[derive(Debug, PartialEq, Eq)]
enum Input {
    Stdin,
    File(String),
}

/// Decide the input source. `None` means "no file given while stdin is a
/// terminal", which is a usage error.
fn resolve_input(file: Option<&str>, stdin_is_tty: bool) -> Option<Input> {
    match file {
        Some("-") => Some(Input::Stdin),
        Some(path) => Some(Input::File(path.to_string())),
        None if !stdin_is_tty => Some(Input::Stdin),
        None => None,
    }
}

/// Colour is on iff the theme is not `none`, `NO_COLOR` is unset or empty,
/// and either `--color always` or (`--color auto` and stdout is a TTY).
fn use_color(
    theme: Theme,
    color: ColorChoice,
    no_color: Option<&OsString>,
    stdout_is_tty: bool,
) -> bool {
    if theme == Theme::None {
        return false;
    }
    if no_color.is_some_and(|v| !v.is_empty()) {
        return false;
    }
    match color {
        ColorChoice::Always => true,
        ColorChoice::Never => false,
        ColorChoice::Auto => stdout_is_tty,
    }
}

/// Resolve `--theme auto` to dark or light. Detection (`detect`) only runs
/// when colour is on and stdout is a terminal, so piped output pays nothing.
fn resolve_theme(
    theme: Theme,
    use_color: bool,
    stdout_is_tty: bool,
    detect: impl FnOnce() -> Option<terminal::Background>,
) -> Theme {
    if theme != Theme::Auto || !use_color || !stdout_is_tty {
        return theme;
    }
    match detect() {
        Some(terminal::Background::Light) => Theme::Light,
        Some(terminal::Background::Dark) | None => Theme::Dark,
    }
}

/// Format an I/O error without the trailing " (os error N)".
fn describe_error(e: &io::Error) -> String {
    let msg = e.to_string();
    match msg.rfind(" (os error ") {
        Some(idx) if msg.ends_with(')') => msg[..idx].to_string(),
        _ => msg,
    }
}

/// Write rendered lines to stdout. A closed pipe (e.g. `| head`) is not an error.
fn dump(lines: &[String]) -> io::Result<()> {
    let stdout = io::stdout();
    let mut out = BufWriter::new(stdout.lock());
    let result = lines
        .iter()
        .try_for_each(|line| writeln!(out, "{}", line))
        .and_then(|()| out.flush());
    match result {
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        other => other,
    }
}

fn read_input(input: &Input) -> Vec<u8> {
    let mut buf = Vec::new();
    match input {
        Input::Stdin => {
            if let Err(e) = io::stdin().lock().read_to_end(&mut buf) {
                eprintln!("markterm: error reading stdin: {}", describe_error(&e));
                process::exit(1);
            }
        }
        Input::File(path) => match fs::read(path) {
            Ok(bytes) => buf = bytes,
            Err(e) => {
                eprintln!("markterm: {}: {}", path, describe_error(&e));
                process::exit(1);
            }
        },
    }
    buf
}

fn main() {
    let cli = Cli::parse();

    let input = match resolve_input(cli.file.as_deref(), io::stdin().is_terminal()) {
        Some(input) => input,
        None => Cli::command()
            .error(
                clap::error::ErrorKind::MissingRequiredArgument,
                "no input file given and stdin is a terminal (use - to read stdin)",
            )
            .exit(),
    };

    let bytes = read_input(&input);
    let content = String::from_utf8_lossy(&bytes);

    let stdout_is_tty = io::stdout().is_terminal();
    let use_color = use_color(
        cli.theme,
        cli.color,
        std::env::var_os("NO_COLOR").as_ref(),
        stdout_is_tty,
    );
    let options = RenderOptions {
        theme: resolve_theme(
            cli.theme,
            use_color,
            stdout_is_tty,
            terminal::detect_background,
        ),
        use_color,
        no_wrap: cli.no_wrap,
    };

    let (term_width, _) = terminal::size();
    let width = cli.width.unwrap_or(term_width);
    let lines = options.render(&content, width);

    if cli.no_pager || !stdout_is_tty {
        if let Err(e) = dump(&lines) {
            eprintln!("markterm: error writing output: {}", describe_error(&e));
            process::exit(1);
        }
        return;
    }

    let filename = match &input {
        Input::Stdin => "(stdin)".to_string(),
        Input::File(path) => path.clone(),
    };
    let rerender = |w: u16| options.render(&content, w);
    // With an explicit -w the layout width is fixed; otherwise follow resizes.
    let rerender: Option<&dyn Fn(u16) -> Vec<String>> = if cli.width.is_none() {
        Some(&rerender)
    } else {
        None
    };
    match pager::run(lines, &filename, rerender) {
        Ok(code) => {
            if code != 0 {
                process::exit(code);
            }
        }
        Err(e) => {
            eprintln!("markterm: pager error: {}", describe_error(&e));
            process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_dash_is_stdin() {
        assert_eq!(resolve_input(Some("-"), true), Some(Input::Stdin));
    }

    #[test]
    fn input_file_is_file() {
        assert_eq!(
            resolve_input(Some("a.md"), false),
            Some(Input::File("a.md".into()))
        );
    }

    #[test]
    fn input_missing_uses_piped_stdin() {
        assert_eq!(resolve_input(None, false), Some(Input::Stdin));
    }

    #[test]
    fn input_missing_with_tty_stdin_is_error() {
        assert_eq!(resolve_input(None, true), None);
    }

    #[test]
    fn color_auto_follows_tty() {
        assert!(use_color(Theme::Auto, ColorChoice::Auto, None, true));
        assert!(!use_color(Theme::Auto, ColorChoice::Auto, None, false));
    }

    #[test]
    fn color_always_overrides_tty() {
        assert!(use_color(Theme::Dark, ColorChoice::Always, None, false));
    }

    #[test]
    fn color_never_and_theme_none() {
        assert!(!use_color(Theme::Auto, ColorChoice::Never, None, true));
        assert!(!use_color(Theme::None, ColorChoice::Always, None, true));
    }

    #[test]
    fn no_color_only_when_non_empty() {
        let set = OsString::from("1");
        let empty = OsString::new();
        assert!(!use_color(
            Theme::Auto,
            ColorChoice::Always,
            Some(&set),
            true
        ));
        assert!(use_color(
            Theme::Auto,
            ColorChoice::Always,
            Some(&empty),
            true
        ));
    }

    #[test]
    fn auto_theme_uses_detected_background() {
        use terminal::Background;
        let light = || Some(Background::Light);
        assert_eq!(resolve_theme(Theme::Auto, true, true, light), Theme::Light);
        let dark = || Some(Background::Dark);
        assert_eq!(resolve_theme(Theme::Auto, true, true, dark), Theme::Dark);
        assert_eq!(resolve_theme(Theme::Auto, true, true, || None), Theme::Dark);
    }

    #[test]
    fn theme_detection_skipped_unless_needed() {
        let never = || -> Option<terminal::Background> { panic!("detection ran") };
        assert_eq!(resolve_theme(Theme::Auto, false, true, never), Theme::Auto);
        assert_eq!(resolve_theme(Theme::Auto, true, false, never), Theme::Auto);
        assert_eq!(resolve_theme(Theme::Light, true, true, never), Theme::Light);
        assert_eq!(resolve_theme(Theme::None, false, true, never), Theme::None);
    }

    #[test]
    fn error_description_strips_os_error() {
        let e = io::Error::from_raw_os_error(2);
        let d = describe_error(&e);
        assert!(!d.contains("os error"), "{d}");
        assert!(!d.is_empty());
    }
}
