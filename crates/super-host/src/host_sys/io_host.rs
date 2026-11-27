//! Host intrinsics behind `std.io.*`: raw file descriptors and the io_uring rings.
//!
//! These are the *only* `sys_fd_*` / `sys_iouring_*` entry points; `.sr` code reaches
//! them exclusively through the lowest-level wrappers in `stdlib/io/`.
//!
//! Lock order is always ring → net. A ring lock is held while a handle is resolved to a
//! descriptor or an accepted connection is registered, never the other way round.

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::net::TcpStream;
use std::os::fd::RawFd;
use std::os::unix::fs::FileExt;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::Duration;

use super_runtime::{HostEnv, Value};

use super::rbuf::RingBuf;
use super::uring::{Backend, CUR_POS, Completion, Options, Preference, Ring, Target};
use super::{arg_int, arg_str, cb, err_val, net, payload_bytes, socket_raw_fd};

/// `sys_iouring_create` flag bits.
const FLAG_SQPOLL: i64 = 1;
const FLAG_FORCE_EPOLL: i64 = 2;
const FLAG_FORCE_NATIVE: i64 = 4;

const MAX_IO_LEN: i64 = 1 << 26;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

struct RingEntry {
    ring: Ring,
    /// Handles registered with `register_files`; the index is the fixed-file slot.
    files: Vec<i64>,
}

type RingTable = Mutex<HashMap<i64, Arc<Mutex<RingEntry>>>>;

fn rings() -> &'static RingTable {
    static RINGS: OnceLock<RingTable> = OnceLock::new();
    RINGS.get_or_init(Default::default)
}

fn ring_entry(handle: i64) -> Result<Arc<Mutex<RingEntry>>, String> {
    lock(rings())
        .get(&handle)
        .cloned()
        .ok_or_else(|| "bad io_uring fd".to_string())
}

/// Run `f` on the ring behind `handle`, mapping failures to a Super error value.
fn with_ring(handle: i64, f: impl FnOnce(&mut RingEntry) -> Result<Value, String>) -> Value {
    let entry = match ring_entry(handle) {
        Ok(e) => e,
        Err(msg) => return err_val(msg),
    };
    let mut guard = lock(&entry);
    f(&mut guard).unwrap_or_else(err_val)
}

fn io_msg(e: std::io::Error) -> String {
    e.to_string()
}

/// Resolve a Super handle to a ring target: a registered slot when available,
/// otherwise the live descriptor.
fn resolve(entry: &RingEntry, handle: i64) -> Result<Target, String> {
    if let Some(slot) = entry.files.iter().position(|&h| h == handle) {
        return Ok(Target::Fixed(slot as u32));
    }
    let state = lock(net());
    socket_raw_fd(&state, handle)
        .map(Target::Fd)
        .ok_or_else(|| "bad fd".to_string())
}

fn offset_arg(args: &[Value], i: usize) -> u64 {
    let n = arg_int(args, i);
    if n < 0 { CUR_POS } else { n as u64 }
}

fn len_arg(args: &[Value], i: usize) -> usize {
    arg_int(args, i).clamp(1, MAX_IO_LEN) as usize
}

fn timeout_arg(ms: i64) -> Option<Duration> {
    u64::try_from(ms).ok().map(Duration::from_millis)
}

fn int_list(v: Option<&Value>) -> Vec<i64> {
    match v {
        Some(Value::ArrayI64(a)) => a.to_vec(),
        Some(Value::Array(a)) => a.iter().map(Value::as_int).collect(),
        _ => Vec::new(),
    }
}

fn buf_list(v: Option<&Value>) -> Vec<Vec<u8>> {
    match v {
        Some(Value::Array(a)) => a.iter().map(|x| payload_bytes(Some(x))).collect(),
        _ => Vec::new(),
    }
}

/// Turn reaped completions into Super structs, registering accepted connections.
fn completion_values(done: Vec<Completion>) -> Value {
    let mut out = Vec::with_capacity(done.len());
    for c in done {
        let mut result = c.result;
        if let Some(fd) = c.accepted {
            let mut state = lock(net());
            let handle = state.alloc();
            state.tcp.insert(handle, TcpStream::from(fd));
            result = handle;
        }
        let events = if c.is_poll && result > 0 { result } else { 0 };
        let mut fields = HashMap::new();
        fields.insert("token".into(), Value::Int(c.token as i64));
        fields.insert("result".into(), Value::Int(result));
        fields.insert("events".into(), Value::Int(events));
        fields.insert("flags".into(), Value::Int(i64::from(c.flags)));
        fields.insert("data".into(), Value::Bytes(c.data));
        out.push(Value::Struct {
            name: None,
            label: None,
            fields,
        });
    }
    Value::Array(out.into())
}

fn open_mode(mode: &str) -> Option<OpenOptions> {
    let mut o = OpenOptions::new();
    match mode {
        "r" => o.read(true),
        "w" => o.write(true).create(true).truncate(true),
        "a" => o.append(true).create(true),
        "rw" => o.read(true).write(true).create(true),
        "w+" => o.read(true).write(true).create(true).truncate(true),
        "x" => o.read(true).write(true).create_new(true),
        _ => return None,
    };
    Some(o)
}

fn raw_fd_of(handle: i64) -> Result<RawFd, String> {
    socket_raw_fd(&lock(net()), handle).ok_or_else(|| "bad fd".to_string())
}

fn register_fd_intrinsics(env: &mut HostEnv) {
    env.register_fn(
        "sys_fd_open",
        cb(|args| {
            let mode = arg_str(args, 1);
            let mode = if mode.is_empty() {
                "r".to_string()
            } else {
                mode
            };
            let Some(opts) = open_mode(&mode) else {
                return err_val(format!("bad open mode '{mode}'"));
            };
            match opts.open(arg_str(args, 0)) {
                Ok(file) => {
                    let mut state = lock(net());
                    let handle = state.alloc();
                    state.files.insert(handle, file);
                    Value::Int(handle)
                }
                Err(e) => err_val(e.to_string()),
            }
        }),
    );
    env.register_fn(
        "sys_fd_close",
        cb(|args| {
            let handle = arg_int(args, 0);
            let mut state = lock(net());
            let closed = state.files.remove(&handle).is_some()
                || state.tcp.remove(&handle).is_some()
                || state.tcp_listen.remove(&handle).is_some()
                || state.udp.remove(&handle).is_some();
            Value::Bool(closed)
        }),
    );
    env.register_fn(
        "sys_fd_size",
        cb(|args| {
            let state = lock(net());
            let Some(file) = state.files.get(&arg_int(args, 0)) else {
                return err_val("bad file fd");
            };
            match file.metadata() {
                Ok(m) => Value::Int(i64::try_from(m.len()).unwrap_or(i64::MAX)),
                Err(e) => err_val(e.to_string()),
            }
        }),
    );
    env.register_fn(
        "sys_fd_read",
        cb(|args| {
            let n = len_arg(args, 1);
            let offset = arg_int(args, 2);
            let state = lock(net());
            let Some(file) = state.files.get(&arg_int(args, 0)) else {
                return err_val("bad file fd");
            };
            let mut buf = vec![0u8; n];
            let got = if offset < 0 {
                std::io::Read::read(&mut &*file, &mut buf)
            } else {
                file.read_at(&mut buf, offset as u64)
            };
            match got {
                Ok(got) => {
                    buf.truncate(got);
                    Value::Bytes(buf)
                }
                Err(e) => err_val(e.to_string()),
            }
        }),
    );
    env.register_fn(
        "sys_fd_write",
        cb(|args| {
            let data = payload_bytes(args.get(1));
            let offset = arg_int(args, 2);
            let state = lock(net());
            let Some(file) = state.files.get(&arg_int(args, 0)) else {
                return err_val("bad file fd");
            };
            let wrote = if offset < 0 {
                std::io::Write::write_all(&mut &*file, &data)
            } else {
                file.write_all_at(&data, offset as u64)
            };
            match wrote {
                Ok(()) => Value::Int(data.len() as i64),
                Err(e) => err_val(e.to_string()),
            }
        }),
    );
    env.register_fn(
        "sys_fd_sync",
        cb(|args| {
            let state = lock(net());
            let Some(file) = state.files.get(&arg_int(args, 0)) else {
                return err_val("bad file fd");
            };
            match file.sync_all() {
                Ok(()) => Value::Bool(true),
                Err(e) => err_val(e.to_string()),
            }
        }),
    );
    env.register_fn(
        "sys_fd_truncate",
        cb(|args| {
            let state = lock(net());
            let Some(file) = state.files.get(&arg_int(args, 0)) else {
                return err_val("bad file fd");
            };
            match file.set_len(arg_int(args, 1).max(0) as u64) {
                Ok(()) => Value::Bool(true),
                Err(e) => err_val(e.to_string()),
            }
        }),
    );
    env.register_fn(
        "sys_fd_local_addr",
        cb(|args| {
            let handle = arg_int(args, 0);
            let state = lock(net());
            let addr = state
                .tcp_listen
                .get(&handle)
                .map(|s| s.local_addr())
                .or_else(|| state.tcp.get(&handle).map(|s| s.local_addr()))
                .or_else(|| state.udp.get(&handle).map(|s| s.local_addr()));
            match addr {
                Some(Ok(a)) => Value::Str(a.to_string()),
                Some(Err(e)) => err_val(e.to_string()),
                None => err_val("bad socket fd"),
            }
        }),
    );
    env.register_fn(
        "sys_fd_raw",
        cb(|args| match raw_fd_of(arg_int(args, 0)) {
            Ok(fd) => Value::Int(i64::from(fd)),
            Err(e) => err_val(e),
        }),
    );
}

/// `sys_iouring_submit_*` entries: resolve the handle, then stage one operation.
fn register_submit(
    env: &mut HostEnv,
    name: &'static str,
    stage: impl Fn(&mut RingEntry, &[Value]) -> Result<(), String> + Send + Sync + 'static,
) {
    env.register_fn(
        name,
        cb(move |args| {
            with_ring(arg_int(args, 0), |entry| {
                stage(entry, args).map(|()| Value::Bool(true))
            })
        }),
    );
}

fn register_ring_intrinsics(env: &mut HostEnv) {
    env.register_fn(
        "sys_iouring_available",
        cb(|_| Value::Bool(Ring::native_available())),
    );
    env.register_fn(
        "sys_iouring_create",
        cb(|args| {
            let entries = match arg_int(args, 0) {
                n if n > 0 => n.min(4096) as u32,
                _ => 256,
            };
            let flags = arg_int(args, 1);
            let backend = if flags & FLAG_FORCE_EPOLL != 0 {
                Preference::Emulated
            } else if flags & FLAG_FORCE_NATIVE != 0 {
                Preference::Native
            } else {
                Preference::Auto
            };
            let opts = Options {
                entries,
                sqpoll: flags & FLAG_SQPOLL != 0,
                backend,
            };
            match Ring::open(opts) {
                Ok(ring) => {
                    let handle = lock(net()).alloc();
                    lock(rings()).insert(
                        handle,
                        Arc::new(Mutex::new(RingEntry {
                            ring,
                            files: Vec::new(),
                        })),
                    );
                    Value::Int(handle)
                }
                Err(e) => err_val(e.to_string()),
            }
        }),
    );
    env.register_fn(
        "sys_iouring_backend",
        cb(|args| {
            with_ring(arg_int(args, 0), |e| {
                Ok(Value::Str(e.ring.backend().name().into()))
            })
        }),
    );
    env.register_fn(
        "sys_iouring_is_native",
        cb(|args| {
            with_ring(arg_int(args, 0), |e| {
                Ok(Value::Bool(e.ring.backend() == Backend::IoUring))
            })
        }),
    );
    env.register_fn(
        "sys_iouring_sqpoll",
        cb(|args| {
            with_ring(arg_int(args, 0), |e| {
                Ok(Value::Bool(e.ring.sqpoll_active()))
            })
        }),
    );
    env.register_fn(
        "sys_iouring_inflight",
        cb(|args| {
            with_ring(arg_int(args, 0), |e| {
                Ok(Value::Int(e.ring.inflight() as i64))
            })
        }),
    );
    env.register_fn(
        "sys_iouring_queued",
        cb(|args| with_ring(arg_int(args, 0), |e| Ok(Value::Int(e.ring.queued() as i64)))),
    );

    register_submit(env, "sys_iouring_submit_nop", |e, a| {
        e.ring.prep_nop(arg_int(a, 1) as u64).map_err(io_msg)
    });
    register_submit(env, "sys_iouring_submit_poll_add", |e, a| {
        let target = resolve(e, arg_int(a, 1))?;
        let events = arg_int(a, 2) as u32;
        e.ring
            .prep_poll_add(arg_int(a, 3) as u64, target, events)
            .map_err(io_msg)
    });
    // (ring, fd, len, offset, token)
    register_submit(env, "sys_iouring_submit_read", |e, a| {
        let target = resolve(e, arg_int(a, 1))?;
        e.ring
            .prep_read(
                arg_int(a, 4) as u64,
                target,
                len_arg(a, 2),
                offset_arg(a, 3),
            )
            .map_err(io_msg)
    });
    // (ring, fd, data, offset, token)
    register_submit(env, "sys_iouring_submit_write", |e, a| {
        let target = resolve(e, arg_int(a, 1))?;
        e.ring
            .prep_write(
                arg_int(a, 4) as u64,
                target,
                payload_bytes(a.get(2)),
                offset_arg(a, 3),
            )
            .map_err(io_msg)
    });
    // (ring, fd, [len, ...], offset, token)
    register_submit(env, "sys_iouring_submit_readv", |e, a| {
        let target = resolve(e, arg_int(a, 1))?;
        let lens: Vec<usize> = int_list(a.get(2))
            .into_iter()
            .map(|n| n.clamp(1, MAX_IO_LEN) as usize)
            .collect();
        e.ring
            .prep_readv(arg_int(a, 4) as u64, target, &lens, offset_arg(a, 3))
            .map_err(io_msg)
    });
    // (ring, fd, [data, ...], offset, token)
    register_submit(env, "sys_iouring_submit_writev", |e, a| {
        let target = resolve(e, arg_int(a, 1))?;
        e.ring
            .prep_writev(
                arg_int(a, 4) as u64,
                target,
                buf_list(a.get(2)),
                offset_arg(a, 3),
            )
            .map_err(io_msg)
    });
    // (ring, fd, buf_index, len, offset, token)
    register_submit(env, "sys_iouring_submit_read_fixed", |e, a| {
        let target = resolve(e, arg_int(a, 1))?;
        let buf = u16::try_from(arg_int(a, 2)).map_err(|_| "bad buffer index".to_string())?;
        let len = u32::try_from(arg_int(a, 3).clamp(1, MAX_IO_LEN)).unwrap_or(1);
        e.ring
            .prep_read_fixed(arg_int(a, 5) as u64, target, buf, len, offset_arg(a, 4))
            .map_err(io_msg)
    });
    // (ring, fd, buf_index, data, offset, token)
    register_submit(env, "sys_iouring_submit_write_fixed", |e, a| {
        let target = resolve(e, arg_int(a, 1))?;
        let buf = u16::try_from(arg_int(a, 2)).map_err(|_| "bad buffer index".to_string())?;
        e.ring
            .prep_write_fixed(
                arg_int(a, 5) as u64,
                target,
                buf,
                payload_bytes(a.get(3)),
                offset_arg(a, 4),
            )
            .map_err(io_msg)
    });
    register_submit(env, "sys_iouring_submit_accept", |e, a| {
        let target = resolve(e, arg_int(a, 1))?;
        e.ring
            .prep_accept(arg_int(a, 2) as u64, target)
            .map_err(io_msg)
    });
    // (ring, socket, len, token)
    register_submit(env, "sys_iouring_submit_recv", |e, a| {
        let target = resolve(e, arg_int(a, 1))?;
        e.ring
            .prep_recv(arg_int(a, 3) as u64, target, len_arg(a, 2))
            .map_err(io_msg)
    });
    // (ring, socket, data, token)
    register_submit(env, "sys_iouring_submit_send", |e, a| {
        let target = resolve(e, arg_int(a, 1))?;
        e.ring
            .prep_send(arg_int(a, 3) as u64, target, payload_bytes(a.get(2)))
            .map_err(io_msg)
    });
    // (ring, milliseconds, token)
    register_submit(env, "sys_iouring_submit_timeout", |e, a| {
        let ms = u64::try_from(arg_int(a, 1)).map_err(|_| "negative timeout".to_string())?;
        e.ring
            .prep_timeout(arg_int(a, 2) as u64, Duration::from_millis(ms))
            .map_err(io_msg)
    });
    register_submit(env, "sys_iouring_submit_fsync", |e, a| {
        let target = resolve(e, arg_int(a, 1))?;
        e.ring
            .prep_fsync(arg_int(a, 2) as u64, target)
            .map_err(io_msg)
    });

    env.register_fn(
        "sys_iouring_submit",
        cb(|args| {
            with_ring(arg_int(args, 0), |e| {
                e.ring
                    .submit()
                    .map(|n| Value::Int(n as i64))
                    .map_err(io_msg)
            })
        }),
    );
    // (ring, min_complete, max, timeout_ms)  timeout_ms: -1 forever, 0 poll
    env.register_fn(
        "sys_iouring_submit_and_wait",
        cb(|args| {
            let min = arg_int(args, 1).max(0) as usize;
            let max = arg_int(args, 2).clamp(1, 4096) as usize;
            let timeout = timeout_arg(arg_int(args, 3));
            with_ring(arg_int(args, 0), |e| {
                e.ring
                    .submit_and_wait(min, timeout, max)
                    .map(completion_values)
                    .map_err(io_msg)
            })
        }),
    );
    // (ring, max, timeout_ms): submit staged work, wait for at least one completion
    // unless `timeout_ms == 0`.
    env.register_fn(
        "sys_iouring_wait",
        cb(|args| {
            let max = arg_int(args, 1).clamp(1, 4096) as usize;
            let timeout_ms = arg_int(args, 2);
            let min = usize::from(timeout_ms != 0);
            let timeout = timeout_arg(timeout_ms);
            with_ring(arg_int(args, 0), |e| {
                e.ring
                    .submit_and_wait(min, timeout, max)
                    .map(completion_values)
                    .map_err(io_msg)
            })
        }),
    );
    env.register_fn(
        "sys_iouring_register_buffers",
        cb(|args| {
            let count = arg_int(args, 1).max(0) as usize;
            let size = arg_int(args, 2).max(0) as usize;
            with_ring(arg_int(args, 0), |e| {
                e.ring
                    .register_buffers(count, size)
                    .map(|()| Value::Bool(true))
                    .map_err(io_msg)
            })
        }),
    );
    env.register_fn(
        "sys_iouring_unregister_buffers",
        cb(|args| {
            with_ring(arg_int(args, 0), |e| {
                e.ring
                    .unregister_buffers()
                    .map(|()| Value::Bool(true))
                    .map_err(io_msg)
            })
        }),
    );
    env.register_fn(
        "sys_iouring_register_files",
        cb(|args| {
            let handles = int_list(args.get(1));
            with_ring(arg_int(args, 0), |e| {
                let fds: Result<Vec<RawFd>, String> =
                    handles.iter().map(|&h| raw_fd_of(h)).collect();
                let fds = fds?;
                e.ring.register_files(&fds).map_err(io_msg)?;
                e.files = handles;
                Ok(Value::Bool(true))
            })
        }),
    );
    env.register_fn(
        "sys_iouring_unregister_files",
        cb(|args| {
            with_ring(arg_int(args, 0), |e| {
                e.ring.unregister_files().map_err(io_msg)?;
                e.files.clear();
                Ok(Value::Bool(true))
            })
        }),
    );
    env.register_fn(
        "sys_iouring_close",
        cb(|args| {
            // Dropping the ring cancels nothing in user space: buffers are owned by the
            // ring and freed only after the kernel descriptor is closed.
            Value::Bool(lock(rings()).remove(&arg_int(args, 0)).is_some())
        }),
    );
}

type RbufTable = Mutex<HashMap<i64, RingBuf>>;

fn rbufs() -> &'static RbufTable {
    static RBUFS: OnceLock<RbufTable> = OnceLock::new();
    RBUFS.get_or_init(Default::default)
}

fn with_rbuf(handle: i64, f: impl FnOnce(&mut RingBuf) -> Value) -> Value {
    match lock(rbufs()).get_mut(&handle) {
        Some(rb) => f(rb),
        None => err_val("bad ring buffer"),
    }
}

/// `sys_rbuf_*`: host-side circular buffers (no per-byte array copies in `.sr`).
fn register_rbuf_intrinsics(env: &mut HostEnv) {
    env.register_fn(
        "sys_rbuf_new",
        cb(|args| {
            let cap = arg_int(args, 0).clamp(1, MAX_IO_LEN) as usize;
            let handle = lock(net()).alloc();
            lock(rbufs()).insert(handle, RingBuf::new(cap));
            Value::Int(handle)
        }),
    );
    env.register_fn(
        "sys_rbuf_close",
        cb(|args| Value::Bool(lock(rbufs()).remove(&arg_int(args, 0)).is_some())),
    );
    env.register_fn(
        "sys_rbuf_put",
        cb(|args| {
            let data = payload_bytes(args.get(1));
            with_rbuf(arg_int(args, 0), |rb| Value::Int(rb.put(&data) as i64))
        }),
    );
    env.register_fn(
        "sys_rbuf_take",
        cb(|args| {
            let n = arg_int(args, 1).max(0) as usize;
            with_rbuf(arg_int(args, 0), |rb| Value::Bytes(rb.take(n)))
        }),
    );
    env.register_fn(
        "sys_rbuf_peek",
        cb(|args| {
            let n = arg_int(args, 1).max(0) as usize;
            with_rbuf(arg_int(args, 0), |rb| Value::Bytes(rb.peek(n)))
        }),
    );
    env.register_fn(
        "sys_rbuf_consume",
        cb(|args| {
            let n = arg_int(args, 1).max(0) as usize;
            with_rbuf(arg_int(args, 0), |rb| Value::Int(rb.consume(n) as i64))
        }),
    );
    env.register_fn(
        "sys_rbuf_find",
        cb(|args| {
            let byte = arg_int(args, 1) as u8;
            with_rbuf(arg_int(args, 0), |rb| {
                Value::Int(rb.find(byte).map_or(-1, |i| i as i64))
            })
        }),
    );
    env.register_fn(
        "sys_rbuf_len",
        cb(|args| with_rbuf(arg_int(args, 0), |rb| Value::Int(rb.len() as i64))),
    );
    env.register_fn(
        "sys_rbuf_cap",
        cb(|args| with_rbuf(arg_int(args, 0), |rb| Value::Int(rb.capacity() as i64))),
    );
    env.register_fn(
        "sys_rbuf_free",
        cb(|args| with_rbuf(arg_int(args, 0), |rb| Value::Int(rb.free() as i64))),
    );
    env.register_fn(
        "sys_rbuf_clear",
        cb(|args| {
            with_rbuf(arg_int(args, 0), |rb| {
                rb.clear();
                Value::Bool(true)
            })
        }),
    );
    // Readable bytes as up to two contiguous chunks, ready for `writev`.
    env.register_fn(
        "sys_rbuf_segments",
        cb(|args| {
            with_rbuf(arg_int(args, 0), |rb| {
                let (a, b) = rb.readable();
                let mut parts = vec![Value::Bytes(a.to_vec())];
                if !b.is_empty() {
                    parts.push(Value::Bytes(b.to_vec()));
                }
                Value::Array(parts.into())
            })
        }),
    );
    // Free space as contiguous segment lengths, ready for `readv`.
    env.register_fn(
        "sys_rbuf_free_segments",
        cb(|args| {
            with_rbuf(arg_int(args, 0), |rb| {
                let (a, b) = rb.writable_lens();
                let lens: Vec<i64> = [a, b]
                    .into_iter()
                    .filter(|&n| n > 0)
                    .map(|n| n as i64)
                    .collect();
                Value::ArrayI64(lens.into())
            })
        }),
    );
    // Binary-safe bridges between host byte payloads and `.sr` byte arrays.
    env.register_fn(
        "sys_io_bytes_to_array",
        cb(|args| {
            let bytes: Vec<i64> = payload_bytes(args.first())
                .into_iter()
                .map(i64::from)
                .collect();
            Value::ArrayI64(bytes.into())
        }),
    );
    env.register_fn(
        "sys_io_array_to_bytes",
        cb(|args| Value::Bytes(payload_bytes(args.first()))),
    );
}

pub(super) fn register(env: &mut HostEnv) {
    register_fd_intrinsics(env);
    register_ring_intrinsics(env);
    register_rbuf_intrinsics(env);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host_sys::default_host_env;
    use std::fs;
    use std::io::{Read, Write};
    use std::os::fd::AsRawFd;

    /// Descriptor behind a file handle.
    fn file_raw(handle: i64) -> Option<RawFd> {
        lock(net()).files.get(&handle).map(fs::File::as_raw_fd)
    }

    fn call(env: &HostEnv, name: &str, args: &[Value]) -> Value {
        env.call(name, args)
            .unwrap_or_else(|| panic!("unknown host fn {name}"))
    }

    fn ok(v: Value) -> Value {
        assert!(!matches!(v, Value::Error(_)), "unexpected error: {v:?}");
        v
    }

    fn i(n: i64) -> Value {
        Value::Int(n)
    }

    fn s(t: &str) -> Value {
        Value::Str(t.into())
    }

    /// All backend flag sets worth exercising on this host.
    fn flag_sets() -> Vec<i64> {
        let mut v = vec![FLAG_FORCE_EPOLL];
        if Ring::native_available() {
            v.push(FLAG_FORCE_NATIVE);
        }
        v
    }

    fn completions(v: Value) -> Vec<HashMap<String, Value>> {
        match v {
            Value::Array(items) => items
                .iter()
                .map(|x| match x {
                    Value::Struct { fields, .. } => fields.clone(),
                    other => panic!("completion must be a struct: {other:?}"),
                })
                .collect(),
            other => panic!("expected completion array, got {other:?}"),
        }
    }

    fn temp(tag: &str) -> String {
        format!(
            "{}/super-io-{tag}-{}",
            std::env::temp_dir().display(),
            std::process::id()
        )
    }

    #[test]
    fn nop_wait_close_default_backend() {
        let env = default_host_env();
        let ring = ok(call(&env, "sys_iouring_create", &[])).as_int();
        assert!(ring > 0);
        assert!(ok(call(&env, "sys_iouring_submit_nop", &[i(ring), i(91)])).as_bool());
        let done = completions(call(&env, "sys_iouring_wait", &[i(ring), i(8), i(0)]));
        assert_eq!(done.len(), 1);
        assert_eq!(done[0]["token"].as_int(), 91);
        assert_eq!(done[0]["result"].as_int(), 0);
        assert!(call(&env, "sys_iouring_close", &[i(ring)]).as_bool());
        assert!(matches!(
            call(&env, "sys_iouring_submit_nop", &[i(ring), i(1)]),
            Value::Error(_)
        ));
        assert!(!call(&env, "sys_iouring_close", &[i(ring)]).as_bool());
    }

    #[test]
    fn backend_reporting_and_forced_fallback() {
        let env = default_host_env();
        let forced = ok(call(
            &env,
            "sys_iouring_create",
            &[i(32), i(FLAG_FORCE_EPOLL)],
        ))
        .as_int();
        assert_eq!(
            call(&env, "sys_iouring_backend", &[i(forced)]).as_str(),
            "epoll"
        );
        assert!(!call(&env, "sys_iouring_is_native", &[i(forced)]).as_bool());
        if Ring::native_available() {
            let native = ok(call(
                &env,
                "sys_iouring_create",
                &[i(32), i(FLAG_FORCE_NATIVE)],
            ))
            .as_int();
            assert_eq!(
                call(&env, "sys_iouring_backend", &[i(native)]).as_str(),
                "io_uring"
            );
            assert!(call(&env, "sys_iouring_is_native", &[i(native)]).as_bool());
            let auto = ok(call(&env, "sys_iouring_create", &[])).as_int();
            assert_eq!(
                call(&env, "sys_iouring_backend", &[i(auto)]).as_str(),
                "io_uring"
            );
            for h in [native, auto] {
                call(&env, "sys_iouring_close", &[i(h)]);
            }
        }
        assert!(call(&env, "sys_iouring_available", &[]).as_bool() == Ring::native_available());
        call(&env, "sys_iouring_close", &[i(forced)]);
    }

    #[test]
    fn batched_file_io_through_the_ring() {
        let env = default_host_env();
        for flags in flag_sets() {
            let path = temp(&format!("batch{flags}"));
            let ring = ok(call(&env, "sys_iouring_create", &[i(64), i(flags)])).as_int();
            let fd = ok(call(&env, "sys_fd_open", &[s(&path), s("w+")])).as_int();
            // Two writes and an fsync go out in one batch.
            ok(call(
                &env,
                "sys_iouring_submit_write",
                &[i(ring), i(fd), s("hello "), i(0), i(1)],
            ));
            ok(call(
                &env,
                "sys_iouring_submit_write",
                &[i(ring), i(fd), s("world"), i(6), i(2)],
            ));
            ok(call(
                &env,
                "sys_iouring_submit_fsync",
                &[i(ring), i(fd), i(3)],
            ));
            assert_eq!(call(&env, "sys_iouring_queued", &[i(ring)]).as_int(), 3);
            let done = completions(ok(call(
                &env,
                "sys_iouring_submit_and_wait",
                &[i(ring), i(3), i(16), i(5000)],
            )));
            assert_eq!(done.len(), 3);
            assert_eq!(call(&env, "sys_fd_size", &[i(fd)]).as_int(), 11);
            assert_eq!(
                call(&env, "sys_fd_read", &[i(fd), i(64), i(0)]).as_str(),
                "hello world"
            );

            ok(call(
                &env,
                "sys_iouring_submit_read",
                &[i(ring), i(fd), i(5), i(6), i(4)],
            ));
            let done = completions(ok(call(
                &env,
                "sys_iouring_wait",
                &[i(ring), i(4), i(5000)],
            )));
            assert_eq!(done[0]["data"].as_str(), "world");
            assert_eq!(done[0]["result"].as_int(), 5);

            assert!(call(&env, "sys_fd_close", &[i(fd)]).as_bool());
            call(&env, "sys_iouring_close", &[i(ring)]);
            fs::remove_file(path).unwrap();
        }
    }

    #[test]
    fn vectored_ring_io() {
        let env = default_host_env();
        for flags in flag_sets() {
            let path = temp(&format!("vec{flags}"));
            let ring = ok(call(&env, "sys_iouring_create", &[i(16), i(flags)])).as_int();
            let fd = ok(call(&env, "sys_fd_open", &[s(&path), s("w+")])).as_int();
            let parts = Value::Array(vec![s("ab"), s("cd"), Value::Bytes(b"ef".to_vec())].into());
            ok(call(
                &env,
                "sys_iouring_submit_writev",
                &[i(ring), i(fd), parts, i(0), i(1)],
            ));
            let done = completions(ok(call(
                &env,
                "sys_iouring_wait",
                &[i(ring), i(4), i(5000)],
            )));
            assert_eq!(done[0]["result"].as_int(), 6);
            let lens = Value::ArrayI64(vec![1i64, 2, 3].into());
            ok(call(
                &env,
                "sys_iouring_submit_readv",
                &[i(ring), i(fd), lens, i(0), i(2)],
            ));
            let done = completions(ok(call(
                &env,
                "sys_iouring_wait",
                &[i(ring), i(4), i(5000)],
            )));
            assert_eq!(done[0]["data"].as_str(), "abcdef");
            call(&env, "sys_fd_close", &[i(fd)]);
            call(&env, "sys_iouring_close", &[i(ring)]);
            fs::remove_file(path).unwrap();
        }
    }

    #[test]
    fn fixed_buffers_and_registered_files() {
        let env = default_host_env();
        for flags in flag_sets() {
            let path = temp(&format!("fixed{flags}"));
            let ring = ok(call(&env, "sys_iouring_create", &[i(16), i(flags)])).as_int();
            let fd = ok(call(&env, "sys_fd_open", &[s(&path), s("w+")])).as_int();
            ok(call(
                &env,
                "sys_iouring_register_buffers",
                &[i(ring), i(2), i(4096)],
            ));
            let files = Value::ArrayI64(vec![fd].into());
            ok(call(&env, "sys_iouring_register_files", &[i(ring), files]));
            // fd is now a fixed-file slot; the same Super handle keeps working.
            ok(call(
                &env,
                "sys_iouring_submit_write_fixed",
                &[i(ring), i(fd), i(0), s("fixed io"), i(0), i(1)],
            ));
            let done = completions(ok(call(
                &env,
                "sys_iouring_wait",
                &[i(ring), i(4), i(5000)],
            )));
            assert_eq!(done[0]["result"].as_int(), 8);
            ok(call(
                &env,
                "sys_iouring_submit_read_fixed",
                &[i(ring), i(fd), i(1), i(8), i(0), i(2)],
            ));
            let done = completions(ok(call(
                &env,
                "sys_iouring_wait",
                &[i(ring), i(4), i(5000)],
            )));
            assert_eq!(done[0]["data"].as_str(), "fixed io");
            // Invalid slot and double registration are errors, not crashes.
            assert!(matches!(
                call(
                    &env,
                    "sys_iouring_submit_read_fixed",
                    &[i(ring), i(fd), i(9), i(1), i(0), i(3)]
                ),
                Value::Error(_)
            ));
            assert!(matches!(
                call(&env, "sys_iouring_register_buffers", &[i(ring), i(1), i(8)]),
                Value::Error(_)
            ));
            ok(call(&env, "sys_iouring_unregister_buffers", &[i(ring)]));
            ok(call(&env, "sys_iouring_unregister_files", &[i(ring)]));
            call(&env, "sys_fd_close", &[i(fd)]);
            call(&env, "sys_iouring_close", &[i(ring)]);
            fs::remove_file(path).unwrap();
        }
    }

    #[test]
    fn accept_recv_send_timeout_and_poll() {
        let env = default_host_env();
        for flags in flag_sets() {
            let ring = ok(call(&env, "sys_iouring_create", &[i(32), i(flags)])).as_int();
            let listener = ok(call(&env, "sys_tcp_listen", &[s("127.0.0.1:0")])).as_int();
            let addr = lock(net()).tcp_listen[&listener].local_addr().unwrap();

            ok(call(
                &env,
                "sys_iouring_submit_poll_add",
                &[i(ring), i(listener), i(1), i(5)],
            ));
            ok(call(
                &env,
                "sys_iouring_submit_accept",
                &[i(ring), i(listener), i(6)],
            ));
            ok(call(
                &env,
                "sys_iouring_submit_timeout",
                &[i(ring), i(10), i(7)],
            ));
            let mut client = TcpStream::connect(addr).unwrap();
            let mut seen = HashMap::new();
            let mut accepted = 0;
            while seen.len() < 3 {
                let done = completions(ok(call(
                    &env,
                    "sys_iouring_wait",
                    &[i(ring), i(8), i(5000)],
                )));
                assert!(!done.is_empty(), "timed out waiting for completions");
                for c in done {
                    let token = c["token"].as_int();
                    if token == 6 {
                        accepted = c["result"].as_int();
                    }
                    seen.insert(token, c);
                }
            }
            assert!(accepted > 0, "accept result is a registered tcp handle");
            assert_eq!(seen[&7]["result"].as_int(), -i64::from(libc::ETIME));
            assert_ne!(seen[&5]["events"].as_int() & 1, 0);

            client.write_all(b"ping").unwrap();
            ok(call(
                &env,
                "sys_iouring_submit_recv",
                &[i(ring), i(accepted), i(64), i(8)],
            ));
            let done = completions(ok(call(
                &env,
                "sys_iouring_wait",
                &[i(ring), i(4), i(5000)],
            )));
            assert_eq!(done[0]["data"].as_str(), "ping");

            ok(call(
                &env,
                "sys_iouring_submit_send",
                &[i(ring), i(accepted), s("pong"), i(9)],
            ));
            let done = completions(ok(call(
                &env,
                "sys_iouring_wait",
                &[i(ring), i(4), i(5000)],
            )));
            assert_eq!(done[0]["result"].as_int(), 4);
            let mut got = [0u8; 4];
            client.read_exact(&mut got).unwrap();
            assert_eq!(&got, b"pong");

            // The accepted handle is a normal tcp handle for the blocking API too.
            client.write_all(b"more").unwrap();
            assert_eq!(
                call(&env, "sys_tcp_read", &[i(accepted), i(4)]).as_str(),
                "more"
            );

            call(&env, "sys_fd_close", &[i(accepted)]);
            call(&env, "sys_fd_close", &[i(listener)]);
            call(&env, "sys_iouring_close", &[i(ring)]);
        }
    }

    #[test]
    fn errors_are_values() {
        let env = default_host_env();
        let ring = ok(call(&env, "sys_iouring_create", &[i(8)])).as_int();
        assert!(matches!(
            call(
                &env,
                "sys_iouring_submit_read",
                &[i(ring), i(424_242), i(8), i(0), i(1)]
            ),
            Value::Error(_)
        ));
        assert!(matches!(
            call(&env, "sys_iouring_submit_timeout", &[i(ring), i(-5), i(1)]),
            Value::Error(_)
        ));
        assert!(matches!(
            call(&env, "sys_iouring_wait", &[i(999_999), i(1), i(0)]),
            Value::Error(_)
        ));
        assert!(matches!(
            call(&env, "sys_fd_open", &[s("/nope/not/here"), s("r")]),
            Value::Error(_)
        ));
        assert!(matches!(
            call(&env, "sys_fd_open", &[s("x"), s("bogus")]),
            Value::Error(_)
        ));
        assert!(matches!(
            call(&env, "sys_fd_read", &[i(123_456), i(4), i(0)]),
            Value::Error(_)
        ));
        // A closed handle is rejected before it can reach the kernel.
        let path = temp("badfd");
        let fd = ok(call(&env, "sys_fd_open", &[s(&path), s("w")])).as_int();
        let raw = file_raw(fd).unwrap();
        assert!(raw >= 0);
        call(&env, "sys_fd_close", &[i(fd)]);
        assert!(matches!(
            call(
                &env,
                "sys_iouring_submit_write",
                &[i(ring), i(fd), s("x"), i(0), i(2)]
            ),
            Value::Error(_)
        ));
        fs::remove_file(path).unwrap();
        call(&env, "sys_iouring_close", &[i(ring)]);
    }

    #[test]
    fn fd_blocking_helpers() {
        let env = default_host_env();
        let path = temp("fdio");
        let fd = ok(call(&env, "sys_fd_open", &[s(&path), s("w+")])).as_int();
        assert_eq!(
            ok(call(&env, "sys_fd_write", &[i(fd), s("abcdef"), i(-1)])).as_int(),
            6
        );
        assert_eq!(
            ok(call(&env, "sys_fd_write", &[i(fd), s("XY"), i(2)])).as_int(),
            2
        );
        assert_eq!(
            call(&env, "sys_fd_read", &[i(fd), i(16), i(0)]).as_str(),
            "abXYef"
        );
        assert!(ok(call(&env, "sys_fd_sync", &[i(fd)])).as_bool());
        assert!(ok(call(&env, "sys_fd_truncate", &[i(fd), i(3)])).as_bool());
        assert_eq!(call(&env, "sys_fd_size", &[i(fd)]).as_int(), 3);
        assert!(call(&env, "sys_fd_raw", &[i(fd)]).as_int() >= 0);
        assert!(matches!(
            call(&env, "sys_fd_local_addr", &[i(fd)]),
            Value::Error(_)
        ));
        let l = ok(call(&env, "sys_tcp_listen", &[s("127.0.0.1:0")])).as_int();
        assert!(
            call(&env, "sys_fd_local_addr", &[i(l)])
                .as_str()
                .starts_with("127.0.0.1:")
        );
        call(&env, "sys_fd_close", &[i(l)]);
        assert!(call(&env, "sys_fd_close", &[i(fd)]).as_bool());
        assert!(!call(&env, "sys_fd_close", &[i(fd)]).as_bool());
        let ro = ok(call(&env, "sys_fd_open", &[s(&path), s("r")])).as_int();
        assert_eq!(
            call(&env, "sys_fd_read", &[i(ro), i(8), i(-1)]).as_str(),
            "abX"
        );
        call(&env, "sys_fd_close", &[i(ro)]);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn rbuf_intrinsics_round_trip_and_feed_readv() {
        let env = default_host_env();
        let rb = ok(call(&env, "sys_rbuf_new", &[i(8)])).as_int();
        assert_eq!(
            call(&env, "sys_rbuf_put", &[i(rb), s("abcdef")]).as_int(),
            6
        );
        assert_eq!(call(&env, "sys_rbuf_take", &[i(rb), i(4)]).as_str(), "abcd");
        assert_eq!(
            call(&env, "sys_rbuf_put", &[i(rb), s("ghijkl")]).as_int(),
            6
        );
        assert_eq!(call(&env, "sys_rbuf_len", &[i(rb)]).as_int(), 8);
        assert_eq!(call(&env, "sys_rbuf_free", &[i(rb)]).as_int(), 0);
        match call(&env, "sys_rbuf_segments", &[i(rb)]) {
            Value::Array(parts) => assert_eq!(parts.len(), 2, "wrapped data is two segments"),
            other => panic!("{other:?}"),
        }
        assert_eq!(call(&env, "sys_rbuf_peek", &[i(rb), i(3)]).as_str(), "efg");
        assert_eq!(
            call(&env, "sys_rbuf_find", &[i(rb), i(i64::from(b'h'))]).as_int(),
            3
        );
        assert_eq!(
            call(&env, "sys_rbuf_find", &[i(rb), i(i64::from(b'z'))]).as_int(),
            -1
        );
        assert_eq!(call(&env, "sys_rbuf_consume", &[i(rb), i(2)]).as_int(), 2);
        assert_eq!(
            call(&env, "sys_rbuf_free_segments", &[i(rb)]).to_string(),
            "[2]"
        );
        let arr = call(
            &env,
            "sys_io_bytes_to_array",
            &[Value::Bytes(vec![0, 255, 7])],
        );
        assert_eq!(arr.to_string(), "[0, 255, 7]");
        assert!(
            matches!(call(&env, "sys_io_array_to_bytes", &[arr]), Value::Bytes(b) if b == [0, 255, 7])
        );
        assert!(call(&env, "sys_rbuf_close", &[i(rb)]).as_bool());
        assert!(matches!(
            call(&env, "sys_rbuf_len", &[i(rb)]),
            Value::Error(_)
        ));
    }
}
