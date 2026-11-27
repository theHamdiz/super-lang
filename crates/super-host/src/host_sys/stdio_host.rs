//! Process stdio host intrinsics (`stdin` / `stdout` / `stderr`).
//!
//! Test hooks (`sys_stdin_feed`, `sys_stdio_capture_*`) keep `.sr` tests off the
//! real terminal without affecting normal `print` (which still uses `println!`).

use std::io::{self, Read, Write};
use std::sync::{Mutex, OnceLock};

use super_runtime::{HostEnv, Value};

use super::{cb, err_val, payload_bytes};

struct Capture {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

struct StdioState {
    /// Simulated stdin: reads drain this first (tests only).
    stdin_buf: Vec<u8>,
    /// When set, stdout/stderr writes go only here (not the process streams).
    capture: Option<Capture>,
}

fn state() -> &'static Mutex<StdioState> {
    static STATE: OnceLock<Mutex<StdioState>> = OnceLock::new();
    STATE.get_or_init(|| {
        Mutex::new(StdioState {
            stdin_buf: Vec::new(),
            capture: None,
        })
    })
}

fn with_state<R>(f: impl FnOnce(&mut StdioState) -> R) -> R {
    let mut guard = state()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    f(&mut guard)
}

fn write_stream(stream: &mut dyn Write, data: &[u8]) -> Result<usize, io::Error> {
    stream.write_all(data)?;
    Ok(data.len())
}

fn stdout_write(st: &mut StdioState, data: &[u8]) -> Result<usize, io::Error> {
    if let Some(cap) = &mut st.capture {
        cap.stdout.extend_from_slice(data);
        return Ok(data.len());
    }
    let mut out = io::stdout().lock();
    write_stream(&mut out, data)
}

fn stderr_write(st: &mut StdioState, data: &[u8]) -> Result<usize, io::Error> {
    if let Some(cap) = &mut st.capture {
        cap.stderr.extend_from_slice(data);
        return Ok(data.len());
    }
    let mut err = io::stderr().lock();
    write_stream(&mut err, data)
}

fn flush_stdio(st: &StdioState) -> Result<(), io::Error> {
    if st.capture.is_some() {
        return Ok(());
    }
    io::stdout().flush()?;
    io::stderr().flush()?;
    Ok(())
}

fn stdin_read(st: &mut StdioState, max: usize) -> Result<Vec<u8>, io::Error> {
    if max == 0 {
        return Ok(Vec::new());
    }
    if !st.stdin_buf.is_empty() {
        let take = max.min(st.stdin_buf.len());
        let chunk: Vec<u8> = st.stdin_buf.drain(..take).collect();
        return Ok(chunk);
    }
    let mut buf = vec![0u8; max];
    let got = io::stdin().read(&mut buf)?;
    buf.truncate(got);
    Ok(buf)
}

pub(super) fn register(env: &mut HostEnv) {
    env.register_fn(
        "sys_stdout_write",
        cb(|args| {
            let data = payload_bytes(args.first());
            with_state(|st| match stdout_write(st, &data) {
                Ok(n) => Value::Int(n as i64),
                Err(e) => err_val(e.to_string()),
            })
        }),
    );
    env.register_fn(
        "sys_stderr_write",
        cb(|args| {
            let data = payload_bytes(args.first());
            with_state(|st| match stderr_write(st, &data) {
                Ok(n) => Value::Int(n as i64),
                Err(e) => err_val(e.to_string()),
            })
        }),
    );
    env.register_fn(
        "sys_stdio_flush",
        cb(|_| {
            with_state(|st| match flush_stdio(st) {
                Ok(()) => Value::Bool(true),
                Err(e) => err_val(e.to_string()),
            })
        }),
    );
    env.register_fn(
        "sys_stdin_read",
        cb(|args| {
            let max = super::arg_int(args, 0).clamp(0, 1 << 20) as usize;
            with_state(|st| match stdin_read(st, max) {
                Ok(b) => Value::Bytes(b),
                Err(e) => err_val(e.to_string()),
            })
        }),
    );
    // --- deterministic test hooks (safe to call from `test` blocks) -------
    env.register_fn(
        "sys_stdin_feed",
        cb(|args| {
            let data = payload_bytes(args.first());
            with_state(|st| {
                st.stdin_buf.extend_from_slice(&data);
                Value::Bool(true)
            })
        }),
    );
    env.register_fn(
        "sys_stdio_capture_begin",
        cb(|_| {
            with_state(|st| {
                if st.capture.is_some() {
                    return err_val("stdio capture already active");
                }
                st.capture = Some(Capture {
                    stdout: Vec::new(),
                    stderr: Vec::new(),
                });
                Value::Bool(true)
            })
        }),
    );
    env.register_fn(
        "sys_stdio_capture_drain",
        cb(|args| {
            let which = super::arg_int(args, 0);
            with_state(|st| {
                let Some(cap) = &mut st.capture else {
                    return err_val("stdio capture not active");
                };
                let bytes = match which {
                    0 => std::mem::take(&mut cap.stdout),
                    1 => std::mem::take(&mut cap.stderr),
                    _ => return err_val("stdio capture stream must be 0 (stdout) or 1 (stderr)"),
                };
                Value::Str(String::from_utf8_lossy(&bytes).into_owned())
            })
        }),
    );
    env.register_fn(
        "sys_stdio_capture_end",
        cb(|_| {
            with_state(|st| {
                st.capture = None;
                Value::Bool(true)
            })
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host_sys::default_host_env;

    fn call(env: &HostEnv, name: &str, args: &[Value]) -> Value {
        env.call(name, args)
            .unwrap_or_else(|| panic!("unknown host fn {name}"))
    }

    fn ok(v: Value) -> Value {
        assert!(!matches!(v, Value::Error(_)), "unexpected error: {v:?}");
        v
    }

    fn s(t: &str) -> Value {
        Value::Str(t.into())
    }

    fn i(n: i64) -> Value {
        Value::Int(n)
    }

    #[test]
    fn stdin_feed_and_read_line_chunks() {
        let env = default_host_env();
        ok(call(&env, "sys_stdin_feed", &[s("alpha\nbeta")]));
        let a = ok(call(&env, "sys_stdin_read", &[i(5)]));
        assert_eq!(a.as_str(), "alpha");
        let rest = ok(call(&env, "sys_stdin_read", &[i(64)]));
        assert_eq!(rest.as_str(), "\nbeta");
        let eof = ok(call(&env, "sys_stdin_read", &[i(8)]));
        assert_eq!(eof.as_str(), "");
    }

    #[test]
    fn capture_stdout_stderr_round_trip() {
        let env = default_host_env();
        ok(call(&env, "sys_stdio_capture_begin", &[]));
        assert_eq!(ok(call(&env, "sys_stdout_write", &[s("out1")])).as_int(), 4);
        assert_eq!(ok(call(&env, "sys_stderr_write", &[s("err!")])).as_int(), 4);
        assert_eq!(
            call(&env, "sys_stdio_capture_drain", &[i(0)]).as_str(),
            "out1"
        );
        assert_eq!(
            call(&env, "sys_stdio_capture_drain", &[i(1)]).as_str(),
            "err!"
        );
        assert!(call(&env, "sys_stdio_capture_end", &[]).as_bool());
        // Live streams again — just ensure no error.
        assert_eq!(ok(call(&env, "sys_stdout_write", &[s("x")])).as_int(), 1);
    }

    #[test]
    fn flush_succeeds() {
        let env = default_host_env();
        assert!(ok(call(&env, "sys_stdio_flush", &[])).as_bool());
    }
}
