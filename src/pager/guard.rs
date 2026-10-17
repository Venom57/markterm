//! Terminal setup and guaranteed teardown for the interactive pager.
//!
//! [`TerminalGuard`] enables raw mode, switches to the alternate screen,
//! hides the cursor and turns on alternate-scroll mode (so the mouse wheel
//! sends Up/Down without capturing the mouse). The terminal is restored when
//! the guard is dropped, when the process panics (via a panic hook), and on
//! SIGTERM/SIGHUP/SIGQUIT/SIGINT (exit status 128 + signal).

use crossterm::{
    cursor, queue,
    style::Print,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Once;

use crate::style;

/// Alternate-scroll mode: wheel events become cursor keys on the alt screen.
const ALT_SCROLL_ON: &str = "\x1b[?1007h";
const ALT_SCROLL_OFF: &str = "\x1b[?1007l";

/// Whether the terminal is currently in pager mode and needs restoring.
static ACTIVE: AtomicBool = AtomicBool::new(false);

fn setup() -> io::Result<()> {
    terminal::enable_raw_mode()?;
    ACTIVE.store(true, Ordering::SeqCst);
    let mut out = io::stdout();
    queue!(
        out,
        EnterAlternateScreen,
        cursor::Hide,
        Print(ALT_SCROLL_ON)
    )?;
    out.flush()
}

/// Undo [`setup`]. Idempotent and callable from any thread; errors are
/// ignored because this runs on the way out (panic, signal, drop).
fn restore() {
    if !ACTIVE.swap(false, Ordering::SeqCst) {
        return;
    }
    let mut out = io::stdout();
    let _ = queue!(
        out,
        Print(style::RESET),
        Print(ALT_SCROLL_OFF),
        cursor::Show,
        LeaveAlternateScreen
    );
    let _ = out.flush();
    let _ = terminal::disable_raw_mode();
}

fn install_panic_hook() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            restore();
            previous(info);
        }));
    });
}

#[cfg(unix)]
mod signals {
    use signal_hook::consts::{SIGHUP, SIGINT, SIGQUIT, SIGTERM};
    use signal_hook::iterator::{Handle, Signals};
    use std::io;

    /// Restore the terminal and exit with 128 + signal on a fatal signal.
    pub fn spawn() -> io::Result<Handle> {
        let mut signals = Signals::new([SIGTERM, SIGHUP, SIGQUIT, SIGINT])?;
        let handle = signals.handle();
        std::thread::Builder::new()
            .name("markterm-signals".into())
            .spawn(move || {
                if let Some(sig) = signals.forever().next() {
                    super::restore();
                    std::process::exit(128 + sig);
                }
            })?;
        Ok(handle)
    }

    /// Stop the process as if Ctrl-Z had been delivered by the tty driver.
    pub fn stop_self() -> io::Result<()> {
        signal_hook::low_level::raise(signal_hook::consts::SIGTSTP)
    }
}

/// Keeps the terminal in pager mode for as long as it lives.
pub struct TerminalGuard {
    #[cfg(unix)]
    signals: signal_hook::iterator::Handle,
}

impl TerminalGuard {
    pub fn enter() -> io::Result<Self> {
        install_panic_hook();
        let guard = TerminalGuard {
            #[cfg(unix)]
            signals: signals::spawn()?,
        };
        // On failure `guard` is dropped, which restores whatever was set up.
        setup()?;
        Ok(guard)
    }

    /// Ctrl-Z: restore the terminal, stop the process, and re-enter pager
    /// mode once the shell resumes us. The caller must redraw afterwards.
    pub fn suspend(&self) -> io::Result<()> {
        #[cfg(unix)]
        {
            restore();
            signals::stop_self()?;
            setup()?;
        }
        Ok(())
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore();
        #[cfg(unix)]
        self.signals.close();
    }
}
