//! Terminal capability detection and dimension queries.

use crossterm::terminal;

/// Returns (width, height) of the terminal, with sensible defaults.
pub fn size() -> (u16, u16) {
    terminal::size().unwrap_or((80, 24))
}

/// Whether the terminal background is dark or light.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Background {
    Dark,
    Light,
}

/// Best-effort detection of the terminal background colour.
///
/// Tries, in order: the `COLORFGBG` environment variable, then (Unix only,
/// when stdout is a terminal) an OSC 11 background-colour query on
/// `/dev/tty`. Returns `None` when neither gives an answer.
pub fn detect_background() -> Option<Background> {
    if let Some(bg) = std::env::var("COLORFGBG")
        .ok()
        .and_then(|v| parse_colorfgbg(&v))
    {
        return Some(bg);
    }
    #[cfg(unix)]
    {
        use std::io::IsTerminal;
        if std::io::stdout().is_terminal() {
            return tty::query_osc11(std::time::Duration::from_millis(100))
                .and_then(|reply| parse_osc11_reply(&reply));
        }
    }
    None
}

/// Parse `COLORFGBG` (`fg;bg` or `fg;default;bg`). The background is the last
/// field: ANSI colours 0–6 and 8 are dark, 7 and 9–15 are light.
fn parse_colorfgbg(value: &str) -> Option<Background> {
    let bg: u8 = value.rsplit(';').next()?.trim().parse().ok()?;
    match bg {
        0..=6 | 8 => Some(Background::Dark),
        7 | 9..=15 => Some(Background::Light),
        _ => None,
    }
}

/// Find an OSC 11 reply (`ESC ] 11 ; rgb:R/G/B` terminated by BEL or ST)
/// anywhere in `buf` and classify the colour it reports.
#[cfg_attr(not(unix), allow(dead_code))]
fn parse_osc11_reply(buf: &[u8]) -> Option<Background> {
    const PREFIX: &[u8] = b"\x1b]11;";
    let start = buf.windows(PREFIX.len()).position(|w| w == PREFIX)? + PREFIX.len();
    let rest = &buf[start..];
    let end = rest
        .iter()
        .enumerate()
        .find(|&(i, &b)| b == 0x07 || (b == 0x1b && rest.get(i + 1) == Some(&b'\\')))
        .map(|(i, _)| i)?;
    let body = std::str::from_utf8(&rest[..end]).ok()?;
    let (r, g, b) = parse_rgb_spec(body)?;
    Some(classify(r, g, b))
}

/// Parse an X11 colour spec `rgb:R/G/B` with 1–4 hex digits per channel into
/// channel values scaled to 0.0–1.0.
#[cfg_attr(not(unix), allow(dead_code))]
fn parse_rgb_spec(spec: &str) -> Option<(f64, f64, f64)> {
    let channels = spec.strip_prefix("rgb:")?;
    let mut it = channels.split('/').map(|c| {
        if c.is_empty() || c.len() > 4 || !c.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let max = (1u32 << (4 * c.len())) - 1;
        let v = u32::from_str_radix(c, 16).ok()?;
        Some(f64::from(v) / f64::from(max))
    });
    let (r, g, b) = (it.next()??, it.next()??, it.next()??);
    if it.next().is_some() {
        return None;
    }
    Some((r, g, b))
}

/// Dark if the (gamma-encoded, Rec. 709 weighted) luminance is below 0.5.
#[cfg_attr(not(unix), allow(dead_code))]
fn classify(r: f64, g: f64, b: f64) -> Background {
    let luminance = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    if luminance < 0.5 {
        Background::Dark
    } else {
        Background::Light
    }
}

/// True once `buf` contains a complete DA1 reply (`ESC [ ? … c`).
#[cfg_attr(not(unix), allow(dead_code))]
fn has_da1_reply(buf: &[u8]) -> bool {
    const PREFIX: &[u8] = b"\x1b[?";
    let mut from = 0;
    while let Some(pos) = buf[from..].windows(PREFIX.len()).position(|w| w == PREFIX) {
        let params = &buf[from + pos + PREFIX.len()..];
        match params
            .iter()
            .position(|b| !(b.is_ascii_digit() || *b == b';'))
        {
            Some(i) if params[i] == b'c' => return true,
            _ => from += pos + 1,
        }
    }
    false
}

#[cfg(unix)]
mod tty {
    use std::time::{Duration, Instant};

    /// OSC 11 "report background colour", followed by DA1 so that terminals
    /// without OSC 11 support still answer promptly.
    const QUERY: &[u8] = b"\x1b]11;?\x1b\\\x1b[c";

    struct Fd(libc::c_int);

    impl Drop for Fd {
        fn drop(&mut self) {
            // SAFETY: we own this descriptor and close it exactly once.
            unsafe { libc::close(self.0) };
        }
    }

    /// Restores the saved terminal attributes on every exit path.
    struct RestoreTermios {
        fd: libc::c_int,
        saved: libc::termios,
    }

    impl Drop for RestoreTermios {
        fn drop(&mut self) {
            // SAFETY: `fd` is open (it outlives this guard) and `saved` was
            // filled in by tcgetattr.
            unsafe { libc::tcsetattr(self.fd, libc::TCSANOW, &self.saved) };
        }
    }

    /// Send the query to `/dev/tty` and collect replies until the DA1
    /// response arrives or `timeout` elapses. Returns whatever was read.
    pub fn query_osc11(timeout: Duration) -> Option<Vec<u8>> {
        // SAFETY: plain libc calls on a descriptor we own; all pointers
        // refer to live, correctly sized local values.
        unsafe {
            let fd = libc::open(
                c"/dev/tty".as_ptr(),
                libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC,
            );
            if fd < 0 {
                return None;
            }
            let fd = Fd(fd);

            // Touching the tty from a background process group would stop
            // us with SIGTTOU/SIGTTIN, so only query when in the foreground.
            if libc::tcgetpgrp(fd.0) != libc::getpgrp() {
                return None;
            }

            let mut saved: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(fd.0, &mut saved) != 0 {
                return None;
            }
            let mut raw = saved;
            raw.c_lflag &= !(libc::ICANON | libc::ECHO);
            raw.c_cc[libc::VMIN] = 0;
            raw.c_cc[libc::VTIME] = 0;
            if libc::tcsetattr(fd.0, libc::TCSANOW, &raw) != 0 {
                return None;
            }
            let _restore = RestoreTermios { fd: fd.0, saved };

            if !write_all(fd.0, QUERY) {
                return None;
            }

            let deadline = Instant::now() + timeout;
            let mut buf = Vec::with_capacity(64);
            let mut chunk = [0u8; 64];
            while !super::has_da1_reply(&buf) {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    break;
                }
                let mut pfd = libc::pollfd {
                    fd: fd.0,
                    events: libc::POLLIN,
                    revents: 0,
                };
                let ms = left.as_millis().clamp(1, 1000) as libc::c_int;
                let n = libc::poll(&mut pfd, 1, ms);
                if n < 0 {
                    if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                        continue;
                    }
                    break;
                }
                if n == 0 || pfd.revents & libc::POLLIN == 0 {
                    continue;
                }
                let r = libc::read(fd.0, chunk.as_mut_ptr().cast(), chunk.len());
                if r <= 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..r as usize]);
            }
            Some(buf)
        }
    }

    fn write_all(fd: libc::c_int, mut data: &[u8]) -> bool {
        while !data.is_empty() {
            // SAFETY: `data` is a live slice of the given length.
            let n = unsafe { libc::write(fd, data.as_ptr().cast(), data.len()) };
            if n < 0 {
                if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                return false;
            }
            data = &data[n as usize..];
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colorfgbg_two_fields() {
        assert_eq!(parse_colorfgbg("15;0"), Some(Background::Dark));
        assert_eq!(parse_colorfgbg("0;15"), Some(Background::Light));
        assert_eq!(parse_colorfgbg("0;7"), Some(Background::Light));
        assert_eq!(parse_colorfgbg("7;8"), Some(Background::Dark));
        assert_eq!(parse_colorfgbg("0;9"), Some(Background::Light));
        assert_eq!(parse_colorfgbg("15;6"), Some(Background::Dark));
    }

    #[test]
    fn colorfgbg_three_fields() {
        assert_eq!(parse_colorfgbg("15;default;0"), Some(Background::Dark));
        assert_eq!(parse_colorfgbg("0;default;15"), Some(Background::Light));
    }

    #[test]
    fn colorfgbg_garbage() {
        assert_eq!(parse_colorfgbg(""), None);
        assert_eq!(parse_colorfgbg("15;default"), None);
        assert_eq!(parse_colorfgbg("15;16"), None);
        assert_eq!(parse_colorfgbg("15;x"), None);
        assert_eq!(parse_colorfgbg("15;-1"), None);
    }

    #[test]
    fn osc11_st_terminator_four_digits() {
        let r = b"\x1b]11;rgb:0000/0000/0000\x1b\\";
        assert_eq!(parse_osc11_reply(r), Some(Background::Dark));
        let r = b"\x1b]11;rgb:ffff/ffff/ffff\x1b\\";
        assert_eq!(parse_osc11_reply(r), Some(Background::Light));
    }

    #[test]
    fn osc11_bel_terminator_two_digits() {
        let r = b"\x1b]11;rgb:fd/f6/e3\x07";
        assert_eq!(parse_osc11_reply(r), Some(Background::Light));
        let r = b"\x1b]11;rgb:1e/1e/2e\x07";
        assert_eq!(parse_osc11_reply(r), Some(Background::Dark));
    }

    #[test]
    fn osc11_one_and_three_digit_channels() {
        assert_eq!(
            parse_osc11_reply(b"\x1b]11;rgb:f/f/f\x07"),
            Some(Background::Light)
        );
        assert_eq!(
            parse_osc11_reply(b"\x1b]11;rgb:111/111/111\x07"),
            Some(Background::Dark)
        );
    }

    #[test]
    fn osc11_followed_by_da1_and_preceded_by_noise() {
        let r = b"x\x1b]11;rgb:2828/2c2c/3434\x1b\\\x1b[?62;22c";
        assert_eq!(parse_osc11_reply(r), Some(Background::Dark));
    }

    #[test]
    fn osc11_garbage() {
        assert_eq!(parse_osc11_reply(b""), None);
        // Only DA1: terminal without OSC 11 support.
        assert_eq!(parse_osc11_reply(b"\x1b[?1;2c"), None);
        // Unterminated.
        assert_eq!(parse_osc11_reply(b"\x1b]11;rgb:ffff/ffff/ffff"), None);
        // Wrong format, too many digits, non-hex, wrong channel count.
        assert_eq!(parse_osc11_reply(b"\x1b]11;#ffffff\x07"), None);
        assert_eq!(parse_osc11_reply(b"\x1b]11;rgb:fffff/0/0\x07"), None);
        assert_eq!(parse_osc11_reply(b"\x1b]11;rgb:zz/00/00\x07"), None);
        assert_eq!(parse_osc11_reply(b"\x1b]11;rgb:ff/00\x07"), None);
        assert_eq!(parse_osc11_reply(b"\x1b]11;rgb:ff/00/00/00\x07"), None);
        assert_eq!(parse_osc11_reply(b"\x1b]11;rgb://\x07"), None);
    }

    #[test]
    fn luminance_threshold() {
        assert_eq!(classify(0.49, 0.49, 0.49), Background::Dark);
        assert_eq!(classify(0.5, 0.5, 0.5), Background::Light);
        // Pure green is bright, pure blue is dark.
        assert_eq!(classify(0.0, 1.0, 0.0), Background::Light);
        assert_eq!(classify(0.0, 0.0, 1.0), Background::Dark);
        // Mid grey 0x80 sits just above the threshold, 0x7f just below.
        assert_eq!(
            parse_osc11_reply(b"\x1b]11;rgb:80/80/80\x07"),
            Some(Background::Light)
        );
        assert_eq!(
            parse_osc11_reply(b"\x1b]11;rgb:7f/7f/7f\x07"),
            Some(Background::Dark)
        );
    }

    #[test]
    fn da1_detection() {
        assert!(has_da1_reply(b"\x1b[?62;22c"));
        assert!(has_da1_reply(b"\x1b]11;rgb:0/0/0\x07\x1b[?1;2c"));
        assert!(has_da1_reply(b"\x1b[?c"));
        assert!(!has_da1_reply(b""));
        assert!(!has_da1_reply(b"\x1b[?62;22"));
        assert!(!has_da1_reply(b"\x1b[?1h"));
        assert!(has_da1_reply(b"\x1b[?1h\x1b[?6c"));
    }
}
