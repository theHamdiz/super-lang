//! Terminal backend for `std.tui`.
//!
//! Widget layout stays in Super. This module only does what a cell loop of
//! string concatenation cannot do cheaply: raw mode, a non-blocking read, and
//! one-pass ANSI (full frame or damaged runs).

#[cfg(unix)]
use std::io::Read;
use std::io::{self, Write};
use std::sync::{Arc, Mutex, OnceLock};

use super_runtime::{HostEnv, Value};

use super::{arg_int, cb};

struct TermState {
    /// Bytes queued by `sys_term_feed` (tests, and a pushed-back tail).
    input: Vec<u8>,
    /// Captured writes. `None` means a write goes to the real stdout.
    captured: Option<Vec<u8>>,
    #[cfg(unix)]
    raw: bool,
    #[cfg(unix)]
    saved: Option<libc::termios>,
}

fn state() -> &'static Mutex<TermState> {
    static STATE: OnceLock<Mutex<TermState>> = OnceLock::new();
    STATE.get_or_init(|| {
        Mutex::new(TermState {
            input: Vec::new(),
            captured: None,
            #[cfg(unix)]
            raw: false,
            #[cfg(unix)]
            saved: None,
        })
    })
}

fn with_state<R>(f: impl FnOnce(&mut TermState) -> R) -> R {
    let mut guard = state()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    f(&mut guard)
}

fn i64s(v: Option<&Value>) -> Arc<Vec<i64>> {
    match v {
        Some(Value::ArrayI64(a)) => Arc::clone(a),
        Some(Value::Array(a)) => Arc::new(a.iter().map(Value::as_int).collect()),
        _ => Arc::new(Vec::new()),
    }
}

fn at(plane: &[i64], i: usize, fallback: i64) -> i64 {
    plane.get(i).copied().unwrap_or(fallback)
}

fn push_cp(out: &mut String, cp: i64) {
    let Some(c) = u32::try_from(cp).ok().and_then(char::from_u32) else {
        out.push(' ');
        return;
    };
    if c == '\0' || c == '\n' || c == '\r' {
        out.push(' ');
    } else {
        out.push(c);
    }
}

fn sgr(fg: i64, bg: i64, attr: i64) -> String {
    let fg_code = if (0..8).contains(&fg) {
        30 + fg
    } else if (8..16).contains(&fg) {
        90 + (fg - 8)
    } else {
        39
    };
    let bg_code = if (0..8).contains(&bg) {
        40 + bg
    } else if (8..16).contains(&bg) {
        100 + (bg - 8)
    } else {
        49
    };
    let bold = if attr & 1 != 0 { ";1" } else { "" };
    let reverse = if attr & 2 != 0 { ";7" } else { "" };
    format!("\x1b[0{bold}{reverse};{fg_code};{bg_code}m")
}

/// Plain text, rows joined by `\n`, no trailing newline.
pub(super) fn render_plain(w: i64, h: i64, ch: &[i64]) -> String {
    if w <= 0 || h <= 0 {
        return String::new();
    }
    let width = w as usize;
    let height = h as usize;
    let mut out = String::with_capacity(width * height + height);
    for y in 0..height {
        if y > 0 {
            out.push('\n');
        }
        for x in 0..width {
            push_cp(&mut out, at(ch, y * width + x, 32));
        }
    }
    out
}

/// ANSI frame. An empty `prev` plane (or a length mismatch) is a full redraw
/// starting at the home cursor. A same-sized previous frame emits only the
/// damaged horizontal runs, each positioned with `CSI row;col H`.
pub(super) fn render_diff(
    w: i64,
    h: i64,
    ch: &[i64],
    fg: &[i64],
    bg: &[i64],
    attr: &[i64],
    pch: &[i64],
    pfg: &[i64],
    pbg: &[i64],
    pat: &[i64],
) -> String {
    if w <= 0 || h <= 0 {
        return String::new();
    }
    let width = w as usize;
    let height = h as usize;
    let n = width * height;
    let full = pch.len() != n || pfg.len() != n || pbg.len() != n || pat.len() != n;
    let mut out = String::with_capacity(n * 4);
    if full {
        out.push_str("\x1b[H");
    }
    let mut style: Option<(i64, i64, i64)> = None;
    let mut any = full;
    for y in 0..height {
        let mut run = false;
        for x in 0..width {
            let i = y * width + x;
            let cell = (at(ch, i, 32), at(fg, i, 7), at(bg, i, 0), at(attr, i, 0));
            let unchanged = !full
                && cell.0 == at(pch, i, 32)
                && cell.1 == at(pfg, i, 7)
                && cell.2 == at(pbg, i, 0)
                && cell.3 == at(pat, i, 0);
            if unchanged {
                run = false;
                continue;
            }
            any = true;
            if !run {
                if !full {
                    out.push_str(&format!("\x1b[{};{}H", y + 1, x + 1));
                    style = None;
                }
                run = true;
            }
            let st = (cell.1, cell.2, cell.3);
            if style != Some(st) {
                out.push_str(&sgr(st.0, st.1, st.2));
                style = Some(st);
            }
            push_cp(&mut out, cell.0);
        }
    }
    if !any {
        return String::new();
    }
    out.push_str("\x1b[0m");
    out
}

#[cfg(unix)]
fn is_tty() -> bool {
    // SAFETY: file descriptor 0 is the process stdin; isatty only reads its status.
    unsafe { libc::isatty(0) == 1 }
}

#[cfg(not(unix))]
fn is_tty() -> bool {
    false
}

#[cfg(unix)]
fn term_size() -> (i64, i64) {
    let mut sz: libc::winsize = unsafe { std::mem::zeroed() };
    // SAFETY: TIOCGWINSZ writes a winsize into `sz` for stdin. A failure leaves the zeros.
    let ok = unsafe { libc::ioctl(0, libc::TIOCGWINSZ, &mut sz) } == 0;
    if ok && sz.ws_col > 0 && sz.ws_row > 0 {
        (sz.ws_col as i64, sz.ws_row as i64)
    } else {
        (80, 24)
    }
}

#[cfg(not(unix))]
fn term_size() -> (i64, i64) {
    (80, 24)
}

#[cfg(unix)]
fn set_raw(on: bool) -> bool {
    if !is_tty() {
        return false;
    }
    with_state(|st| {
        if on {
            if st.raw {
                return true;
            }
            let mut saved: libc::termios = unsafe { std::mem::zeroed() };
            // SAFETY: tcgetattr reads the terminal state of stdin into `saved`.
            if unsafe { libc::tcgetattr(0, &mut saved) } != 0 {
                return false;
            }
            let mut raw = saved;
            raw.c_iflag &= !(libc::IGNBRK
                | libc::BRKINT
                | libc::PARMRK
                | libc::ISTRIP
                | libc::INLCR
                | libc::IGNCR
                | libc::ICRNL
                | libc::IXON);
            raw.c_oflag &= !libc::OPOST;
            raw.c_lflag &= !(libc::ECHO | libc::ECHONL | libc::ICANON | libc::ISIG | libc::IEXTEN);
            raw.c_cflag &= !(libc::CSIZE | libc::PARENB);
            raw.c_cflag |= libc::CS8;
            raw.c_cc[libc::VMIN] = 0;
            raw.c_cc[libc::VTIME] = 0;
            // SAFETY: tcsetattr applies `raw` to stdin. `saved` is restored by a later call.
            if unsafe { libc::tcsetattr(0, libc::TCSANOW, &raw) } != 0 {
                return false;
            }
            st.saved = Some(saved);
            st.raw = true;
            true
        } else {
            if let Some(saved) = st.saved {
                // SAFETY: `saved` came from tcgetattr on this same descriptor.
                unsafe { libc::tcsetattr(0, libc::TCSANOW, &saved) };
            }
            st.saved = None;
            st.raw = false;
            true
        }
    })
}

#[cfg(not(unix))]
fn set_raw(on: bool) -> bool {
    let _ = on;
    false
}

fn read_available(max: usize) -> Vec<u8> {
    let queued = with_state(|st| {
        if st.input.is_empty() {
            None
        } else {
            let n = max.min(st.input.len());
            Some(st.input.drain(..n).collect::<Vec<u8>>())
        }
    });
    if let Some(bytes) = queued {
        return bytes;
    }
    #[cfg(unix)]
    {
        let mut pfd = libc::pollfd {
            fd: 0,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: poll on stdin with a zero timeout does not block and does not take ownership.
        let ready = unsafe { libc::poll(&mut pfd, 1, 0) };
        if ready <= 0 {
            return Vec::new();
        }
        let mut buf = vec![0u8; max];
        match io::stdin().read(&mut buf) {
            Ok(n) => {
                buf.truncate(n);
                buf
            }
            Err(_) => Vec::new(),
        }
    }
    #[cfg(not(unix))]
    Vec::new()
}

fn reg(env: &mut HostEnv, name: &str, f: impl Fn(&[Value]) -> Value + Send + Sync + 'static) {
    let cb = cb(f);
    env.register_fn(name, Arc::clone(&cb));
    env.register_fn(format!("host.{name}"), cb);
}

pub(super) fn register(env: &mut HostEnv) {
    reg(env, "sys_term_cols", |_| Value::Int(term_size().0));
    reg(env, "sys_term_rows", |_| Value::Int(term_size().1));
    reg(env, "sys_term_isatty", |_| Value::Bool(is_tty()));
    reg(env, "sys_term_raw", |args| {
        Value::Bool(set_raw(arg_int(args, 0) != 0))
    });
    reg(env, "sys_term_read", |args| {
        let n = arg_int(args, 0).clamp(1, 4096);
        let bytes = read_available(n as usize);
        Value::Str(String::from_utf8_lossy(&bytes).into_owned())
    });
    reg(env, "sys_term_write", |args| {
        let text = args.first().map(Value::as_str).unwrap_or_default();
        let bytes = text.as_bytes();
        let captured = with_state(|st| {
            if let Some(buf) = &mut st.captured {
                buf.extend_from_slice(bytes);
                true
            } else {
                false
            }
        });
        if captured {
            return Value::Int(bytes.len() as i64);
        }
        match io::stdout()
            .write_all(bytes)
            .and_then(|()| io::stdout().flush())
        {
            Ok(()) => Value::Int(bytes.len() as i64),
            Err(e) => Value::Error(e.to_string()),
        }
    });
    reg(env, "sys_term_feed", |args| {
        let text = args.first().map(Value::as_str).unwrap_or_default();
        with_state(|st| {
            st.input.extend(text.as_bytes());
            if st.captured.is_none() {
                st.captured = Some(Vec::new());
            }
        });
        Value::Bool(true)
    });
    reg(env, "sys_term_written", |_| {
        let text = with_state(|st| {
            st.captured
                .as_mut()
                .map(|buf| {
                    let s = String::from_utf8_lossy(buf).into_owned();
                    buf.clear();
                    s
                })
                .unwrap_or_default()
        });
        Value::Str(text)
    });
    reg(env, "sys_term_reset", |_| {
        set_raw(false);
        with_state(|st| {
            st.input.clear();
            st.captured = None;
        });
        Value::Bool(true)
    });
    reg(env, "sys_tui_plain", |args| {
        let w = arg_int(args, 0);
        let h = arg_int(args, 1);
        let ch = i64s(args.get(2));
        Value::Str(render_plain(w, h, &ch))
    });
    reg(env, "sys_tui_diff", |args| {
        let w = arg_int(args, 0);
        let h = arg_int(args, 1);
        let ch = i64s(args.get(2));
        let fg = i64s(args.get(3));
        let bg = i64s(args.get(4));
        let attr = i64s(args.get(5));
        let pch = i64s(args.get(6));
        let pfg = i64s(args.get(7));
        let pbg = i64s(args.get(8));
        let pat = i64s(args.get(9));
        Value::Str(render_diff(
            w, h, &ch, &fg, &bg, &attr, &pch, &pfg, &pbg, &pat,
        ))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_joins_rows_without_a_trailing_newline() {
        let ch = vec![b'A' as i64, b'B' as i64, b'C' as i64, b'D' as i64];
        assert_eq!(render_plain(2, 2, &ch), "AB\nCD");
        assert_eq!(render_plain(0, 2, &ch), "");
    }

    #[test]
    fn diff_skips_a_frame_that_matches_and_patches_one_cell() {
        let ch = vec![b'A' as i64, b'B' as i64];
        let fg = vec![7, 7];
        let bg = vec![0, 0];
        let attr = vec![0, 0];
        let full = render_diff(2, 1, &ch, &fg, &bg, &attr, &[], &[], &[], &[]);
        assert!(full.starts_with("\x1b[H"), "{full:?}");
        assert!(full.contains('A'));
        assert!(full.contains('B'));
        let same = render_diff(2, 1, &ch, &fg, &bg, &attr, &ch, &fg, &bg, &attr);
        assert_eq!(same, "");
        let mut next = ch.clone();
        next[1] = b'Z' as i64;
        let patch = render_diff(2, 1, &next, &fg, &bg, &attr, &ch, &fg, &bg, &attr);
        assert!(patch.contains("\x1b[1;2H"), "{patch:?}");
        assert!(patch.contains('Z'));
        assert!(!patch.contains('A'));
    }

    #[test]
    fn style_runs_emit_one_sgr_per_change() {
        let ch = vec![b'x' as i64, b'y' as i64];
        let fg = vec![1, 1];
        let bg = vec![0, 4];
        let attr = vec![1, 1];
        let frame = render_diff(2, 1, &ch, &fg, &bg, &attr, &[], &[], &[], &[]);
        assert_eq!(frame.matches("\x1b[").count(), 4, "{frame:?}");
    }
}
