//! Windows signal compatibility surface.
//!
//! Windows has no pollable POSIX signal descriptor. The portable host keeps
//! explicit inject/last/signaled semantics and reports `-1` for the descriptor;
//! servers use the socket poll fallback and cooperative shutdown flag.

use std::io::{self, IsTerminal};
use std::sync::atomic::{AtomicI32, Ordering};

use super_runtime::{HostEnv, Value};

use super::{cb, err_val};

static LAST_SIGNAL: AtomicI32 = AtomicI32::new(0);

pub(super) fn register(env: &mut HostEnv) {
    env.register_fn(
        "sys_signal_arm",
        cb(|_| {
            LAST_SIGNAL.store(0, Ordering::SeqCst);
            Value::Int(-1)
        }),
    );
    env.register_fn("sys_signal_fd", cb(|_| Value::Int(-1)));
    env.register_fn(
        "sys_signal_recv",
        cb(|_| {
            let signal = LAST_SIGNAL.swap(0, Ordering::SeqCst);
            if signal == 0 {
                err_val("no pending console signal")
            } else {
                Value::Int(i64::from(signal))
            }
        }),
    );
    env.register_fn(
        "sys_signal_signaled",
        cb(|_| Value::Bool(LAST_SIGNAL.load(Ordering::SeqCst) != 0)),
    );
    env.register_fn(
        "sys_signal_last",
        cb(|_| Value::Int(i64::from(LAST_SIGNAL.load(Ordering::SeqCst)))),
    );
    env.register_fn(
        "sys_signal_close",
        cb(|_| {
            LAST_SIGNAL.store(0, Ordering::SeqCst);
            Value::Bool(true)
        }),
    );
    env.register_fn(
        "sys_signal_inject",
        cb(|args| {
            LAST_SIGNAL.store(
                args.first().map(Value::as_int).unwrap_or(15) as i32,
                Ordering::SeqCst,
            );
            Value::Bool(true)
        }),
    );
    env.register_fn(
        "sys_wants_color",
        cb(|_| {
            let forced = std::env::var("FORCE_COLOR").is_ok_and(|v| !v.is_empty() && v != "0");
            let disabled = std::env::var_os("NO_COLOR").is_some();
            Value::Bool(forced || (!disabled && io::stderr().is_terminal()))
        }),
    );
    env.register_fn(
        "sys_isatty_fd",
        cb(|args| {
            let fd = args.first().map(Value::as_int).unwrap_or(2);
            Value::Bool(match fd {
                0 => io::stdin().is_terminal(),
                1 => io::stdout().is_terminal(),
                _ => io::stderr().is_terminal(),
            })
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn injected_signal_is_observable() {
        let mut env = HostEnv::new();
        register(&mut env);
        assert!(env.call("sys_signal_arm", &[]).is_some());
        assert!(
            env.call("sys_signal_inject", &[Value::Int(15)])
                .unwrap()
                .as_bool()
        );
        assert!(env.call("sys_signal_signaled", &[]).unwrap().as_bool());
        assert_eq!(env.call("sys_signal_recv", &[]).unwrap().as_int(), 15);
    }
}
