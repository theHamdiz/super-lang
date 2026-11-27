//! OS signal notify: self-pipe + handlers for graceful shutdown.
//!
//! `sys_signal_arm` installs handlers that write the signo to a pipe. The read
//! end is pollable (`sys_signal_fd`) and readable (`sys_signal_recv`), so both
//! the fiber VM and sequential C AOT can wait without a full scheduler.

use std::io::{self, IsTerminal};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Mutex, OnceLock};

use super_runtime::{HostEnv, Value};

use super::{cb, err_val};

struct NotifyState {
    read: OwnedFd,
    write: OwnedFd,
    /// Last delivered signo (also readable via pending).
    last: AtomicI32,
    armed: AtomicBool,
}

fn state() -> &'static Mutex<Option<NotifyState>> {
    static STATE: OnceLock<Mutex<Option<NotifyState>>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(None))
}

fn pipe_pair() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0i32; 2];
    #[cfg(target_os = "linux")]
    // SAFETY: pipe2 writes two valid descriptors into `fds` on success.
    let rc = unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) };
    #[cfg(not(target_os = "linux"))]
    // SAFETY: pipe writes two valid descriptors into `fds` on success.
    let rc = unsafe { libc::pipe(fds.as_mut_ptr()) };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: ownership of the pipe ends transfers to OwnedFd.
    let pair = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
    #[cfg(not(target_os = "linux"))]
    for fd in [&pair.0, &pair.1] {
        set_nonblock(fd.as_raw_fd(), true)?;
        // SAFETY: F_SETFD changes descriptor flags without touching memory.
        if unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(pair)
}

fn set_nonblock(fd: RawFd, enabled: bool) -> io::Result<()> {
    // SAFETY: fcntl F_GETFL / F_SETFL only touch the descriptor flags.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    let next = if enabled {
        flags | libc::O_NONBLOCK
    } else {
        flags & !libc::O_NONBLOCK
    };
    let rc = unsafe { libc::fcntl(fd, libc::F_SETFL, next) };
    if rc != 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Write end of the notify pipe for the signal handler (async-signal-safe path).
static WRITE_FD: AtomicI32 = AtomicI32::new(-1);
/// Last signo observed (updated from the handler without taking locks).
static LAST_SIG: AtomicI32 = AtomicI32::new(0);

extern "C" fn on_signal(signo: libc::c_int) {
    let fd = WRITE_FD.load(Ordering::SeqCst);
    if fd < 0 {
        return;
    }
    LAST_SIG.store(signo, Ordering::SeqCst);
    let byte = [signo as u8];
    // SAFETY: write is async-signal-safe; fd is the live notify write end.
    unsafe {
        let _ = libc::write(fd, byte.as_ptr().cast(), 1);
    }
}

fn install_handler(signo: i32) -> io::Result<()> {
    // SAFETY: SIG_DFL-compatible handler with no signal-unsafe work beyond the
    // self-pipe write above; sa_mask is empty and SA_RESTART is unset so poll wakes.
    let mut sa: libc::sigaction = unsafe { std::mem::zeroed() };
    sa.sa_sigaction = on_signal as *const () as usize;
    unsafe {
        libc::sigemptyset(&mut sa.sa_mask);
    }
    sa.sa_flags = 0;
    let rc = unsafe { libc::sigaction(signo, &sa, std::ptr::null_mut()) };
    if rc != 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn arm(sigs: &[i64]) -> Result<i64, String> {
    let (read, write) = pipe_pair().map_err(|e| e.to_string())?;
    let read_fd = read.as_raw_fd() as i64;
    let write_fd = write.as_raw_fd();
    LAST_SIG.store(0, Ordering::SeqCst);
    WRITE_FD.store(write_fd, Ordering::SeqCst);
    {
        let mut guard = state().lock().map_err(|_| "signal state poisoned")?;
        *guard = Some(NotifyState {
            read,
            write,
            last: AtomicI32::new(0),
            armed: AtomicBool::new(true),
        });
    }
    for &s in sigs {
        if s <= 0 {
            continue;
        }
        install_handler(s as i32).map_err(|e| e.to_string())?;
    }
    Ok(read_fd)
}

fn close_notify() -> bool {
    WRITE_FD.store(-1, Ordering::SeqCst);
    let Ok(mut guard) = state().lock() else {
        return false;
    };
    if let Some(st) = guard.take() {
        st.armed.store(false, Ordering::SeqCst);
        drop(st);
        true
    } else {
        false
    }
}

fn signal_fd() -> i64 {
    let Ok(guard) = state().lock() else {
        return -1;
    };
    guard
        .as_ref()
        .map(|s| s.read.as_raw_fd() as i64)
        .unwrap_or(-1)
}

fn signaled() -> bool {
    LAST_SIG.load(Ordering::SeqCst) != 0
}

fn recv_blocking() -> Result<i64, String> {
    let fd = {
        let guard = state().lock().map_err(|_| "signal state poisoned")?;
        let st = guard.as_ref().ok_or("signal not armed")?;
        st.read.as_raw_fd()
    };
    let mut buf = [0u8; 1];
    // Try non-blocking first (inject / already-queued signals).
    // SAFETY: read from the notify pipe while armed.
    let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), 1) };
    if n < 0 {
        let err = io::Error::last_os_error();
        if err.kind() == io::ErrorKind::WouldBlock {
            // Fall back to blocking wait for a real OS signal.
            set_nonblock(fd, false).map_err(|e| e.to_string())?;
            let n2 = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), 1) };
            let _ = set_nonblock(fd, true);
            if n2 < 0 {
                return Err(io::Error::last_os_error().to_string());
            }
            if n2 == 0 {
                return Err("signal pipe closed".into());
            }
        } else {
            return Err(err.to_string());
        }
    } else if n == 0 {
        return Err("signal pipe closed".into());
    }
    let from_pipe = buf[0] as i32;
    let from_atomic = LAST_SIG.load(Ordering::SeqCst);
    let signo = if from_pipe != 0 {
        from_pipe
    } else if from_atomic != 0 {
        from_atomic
    } else {
        0
    };
    if signo == 0 {
        return Err("signal: empty notify".into());
    }
    Ok(i64::from(signo))
}

fn inject(signo: i64) -> Result<bool, String> {
    let guard = state().lock().map_err(|_| "signal state poisoned")?;
    let st = guard.as_ref().ok_or("signal not armed")?;
    LAST_SIG.store(signo as i32, Ordering::SeqCst);
    st.last.store(signo as i32, Ordering::SeqCst);
    let fd = st.write.as_raw_fd();
    let byte = [signo as u8];
    // SAFETY: write to the notify pipe under the state lock.
    let n = unsafe { libc::write(fd, byte.as_ptr().cast(), 1) };
    if n < 0 {
        return Err(io::Error::last_os_error().to_string());
    }
    Ok(true)
}

fn wants_color() -> bool {
    match std::env::var("FORCE_COLOR") {
        Ok(v) if !v.is_empty() && v != "0" => return true,
        _ => {}
    }
    if std::env::var_os("NO_COLOR").is_some() {
        return false;
    }
    io::stderr().is_terminal()
}

pub(super) fn register(env: &mut HostEnv) {
    env.register_fn(
        "sys_signal_arm",
        cb(|args| {
            let sigs: Vec<i64> = match args.first() {
                Some(Value::Array(a)) => a.iter().map(Value::as_int).collect(),
                Some(Value::ArrayI64(a)) => a.to_vec(),
                Some(v) => vec![v.as_int()],
                None => vec![libc::SIGINT as i64, libc::SIGTERM as i64],
            };
            match arm(&sigs) {
                Ok(fd) => Value::Int(fd),
                Err(e) => err_val(e),
            }
        }),
    );
    env.register_fn("sys_signal_fd", cb(|_| Value::Int(signal_fd())));
    env.register_fn(
        "sys_signal_recv",
        cb(|_| match recv_blocking() {
            Ok(s) => Value::Int(s),
            Err(e) => err_val(e),
        }),
    );
    env.register_fn("sys_signal_signaled", cb(|_| Value::Bool(signaled())));
    env.register_fn(
        "sys_signal_last",
        cb(|_| Value::Int(i64::from(LAST_SIG.load(Ordering::SeqCst)))),
    );
    env.register_fn("sys_signal_close", cb(|_| Value::Bool(close_notify())));
    env.register_fn(
        "sys_signal_inject",
        cb(
            |args| match inject(args.first().map(Value::as_int).unwrap_or(15)) {
                Ok(v) => Value::Bool(v),
                Err(e) => err_val(e),
            },
        ),
    );
    env.register_fn("sys_wants_color", cb(|_| Value::Bool(wants_color())));
    env.register_fn(
        "sys_isatty_fd",
        cb(|args| {
            let fd = args.first().map(Value::as_int).unwrap_or(2) as i32;
            // SAFETY: isatty only inspects the descriptor.
            Value::Bool(unsafe { libc::isatty(fd) == 1 })
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use super_runtime::HostEnv;

    #[test]
    fn inject_makes_recv_and_signaled() {
        let mut env = HostEnv::new();
        register(&mut env);
        let fd = env
            .call("sys_signal_arm", &[Value::ArrayI64(vec![15].into())])
            .unwrap();
        assert!(fd.as_int() >= 0);
        assert!(
            env.call("sys_signal_inject", &[Value::Int(15)])
                .unwrap()
                .as_bool()
        );
        assert!(env.call("sys_signal_signaled", &[]).unwrap().as_bool());
        assert_eq!(env.call("sys_signal_recv", &[]).unwrap().as_int(), 15);
        assert!(env.call("sys_signal_close", &[]).unwrap().as_bool());
    }

    #[test]
    fn wants_color_respects_force_and_no_color() {
        let mut env = HostEnv::new();
        register(&mut env);
        // FORCE_COLOR wins when set non-empty / non-zero.
        unsafe { std::env::set_var("FORCE_COLOR", "1") };
        unsafe { std::env::remove_var("NO_COLOR") };
        assert!(env.call("sys_wants_color", &[]).unwrap().as_bool());
        unsafe { std::env::set_var("NO_COLOR", "1") };
        unsafe { std::env::remove_var("FORCE_COLOR") };
        assert!(!env.call("sys_wants_color", &[]).unwrap().as_bool());
        unsafe { std::env::remove_var("NO_COLOR") };
    }
}
