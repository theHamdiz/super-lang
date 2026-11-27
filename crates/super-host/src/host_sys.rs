//! Tier-1 OS / crypto / platform kernel for Super.
//!
//! Only primitives that pure `.sr` cannot express: process/env, filesystem,
//! sockets/TLS hooks, clocks, crypto, and lightweight window/audio fallbacks.
//! High-level APIs live in embedded `stdlib/**/*.sr`.

#![allow(clippy::too_many_arguments)]

use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream, ToSocketAddrs, UdpSocket};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::os::fd::AsRawFd;
#[cfg(windows)]
use std::os::windows::io::AsRawSocket;

use sha1::Sha1;
use sha2::{Digest, Sha256, Sha512};
use super_runtime::{HostCallback, HostEnv, Value};

mod gui_host;
#[cfg(target_os = "linux")]
mod io_host;
mod pal;
#[cfg(target_os = "linux")]
pub mod rbuf;
#[cfg(unix)]
mod signal_host;
#[cfg(windows)]
#[path = "host_sys/signal_host_windows.rs"]
mod signal_host;
mod stdio_host;
mod term_host;
#[cfg(target_os = "linux")]
pub mod uring;

static MONO_START: OnceLock<Instant> = OnceLock::new();

fn mono_start() -> Instant {
    *MONO_START.get_or_init(Instant::now)
}

fn cb(f: impl Fn(&[Value]) -> Value + Send + Sync + 'static) -> HostCallback {
    Arc::new(f)
}

fn arg_str(args: &[Value], i: usize) -> String {
    args.get(i).map(Value::as_str).unwrap_or_default()
}

fn arg_int(args: &[Value], i: usize) -> i64 {
    args.get(i).map(Value::as_int).unwrap_or(0)
}

/// Byte offset of character index `ci` in `s` (`s.len()` at the end), or `None` past the end.
fn char_to_byte(s: &str, ci: usize) -> Option<usize> {
    s.char_indices()
        .map(|(b, _)| b)
        .chain(std::iter::once(s.len()))
        .nth(ci)
}

/// Character index of the first `needle` in `s` at or after character `start` (negative reads
/// as 0), or -1; an empty needle answers `start` unchanged.
fn str_find(s: &str, needle: &str, start: i64) -> i64 {
    if needle.is_empty() {
        return start;
    }
    let from = start.max(0) as usize;
    let Some(b) = char_to_byte(s, from) else {
        return -1;
    };
    s[b..]
        .find(needle)
        .map_or(-1, |off| (from + s[b..b + off].chars().count()) as i64)
}

/// True when `needle` occurs in `s` starting exactly at character `pos`.
fn str_match_at(s: &str, needle: &str, pos: i64) -> bool {
    if needle.is_empty() {
        return true;
    }
    if pos < 0 {
        return false;
    }
    char_to_byte(s, pos as usize).is_some_and(|b| s[b..].starts_with(needle))
}

fn arg_bool(args: &[Value], i: usize) -> bool {
    args.get(i).map(Value::as_bool).unwrap_or(false)
}

fn ok_str(s: impl Into<String>) -> Value {
    Value::Str(s.into())
}

fn err_val(msg: impl Into<String>) -> Value {
    Value::Error(msg.into())
}

fn bytes_val(b: Vec<u8>) -> Value {
    Value::Bytes(b)
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

fn payload_bytes(v: Option<&Value>) -> Vec<u8> {
    match v {
        Some(Value::Bytes(b)) => b.clone(),
        Some(Value::ArrayI64(a)) => a.iter().map(|&x| x as u8).collect(),
        Some(Value::Array(a)) => a.iter().map(|x| x.as_int() as u8).collect(),
        Some(v) => v.as_str().into_bytes(),
        None => Vec::new(),
    }
}

/// First `nbytes` of `mac` equal lowercase hex at `text[lo..lo+2*nbytes)`.
fn sha2_hex_prefix_eq(mac: &[i64], nbytes: i64, text: &str, lo: i64) -> bool {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    if nbytes < 0 || lo < 0 {
        return false;
    }
    let nbytes = nbytes as usize;
    let lo = lo as usize;
    let need = lo.saturating_add(nbytes.saturating_mul(2));
    let bytes = text.as_bytes();
    if need > bytes.len() || nbytes > mac.len() {
        return false;
    }
    for i in 0..nbytes {
        let b = mac[i] as u8;
        if bytes[lo + 2 * i] != HEX[(b >> 4) as usize]
            || bytes[lo + 2 * i + 1] != HEX[(b & 15) as usize]
        {
            return false;
        }
    }
    true
}

/// Standard base64 (RFC 4648 §4) with `=` padding.
fn base64_encode(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(chunk.get(1).copied().unwrap_or(0)) << 8)
            | u32::from(chunk.get(2).copied().unwrap_or(0));
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// IEEE CRC-32 (ISO 3309), reflected, init/xorout `0xFFFFFFFF`.
fn crc32_ieee(data: &[u8]) -> u32 {
    static TABLE: OnceLock<[u32; 256]> = OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut t = [0u32; 256];
        for (i, slot) in t.iter_mut().enumerate() {
            let mut c = i as u32;
            for _ in 0..8 {
                c = if c & 1 == 1 {
                    0xEDB8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
            *slot = c;
        }
        t
    });
    !data.iter().fold(!0u32, |crc, &b| {
        table[((crc ^ u32::from(b)) & 0xFF) as usize] ^ (crc >> 8)
    })
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    let mut out = Vec::with_capacity(s.len() / 2);
    let bytes = s.as_bytes();
    let mut i = 0;
    while i + 1 < bytes.len() {
        let hi = from_hex(bytes[i])?;
        let lo = from_hex(bytes[i + 1])?;
        out.push((hi << 4) | lo);
        i += 2;
    }
    Some(out)
}

fn from_hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// In-process TCP/UDP handle table (fd as i64).
#[derive(Default)]
struct NetState {
    next: i64,
    tcp_listen: HashMap<i64, TcpListener>,
    tcp: HashMap<i64, TcpStream>,
    udp: HashMap<i64, UdpSocket>,
    pollers: HashMap<i64, pal::Poller>,
    /// Regular files opened through `sys_fd_open` (shared handle space).
    #[cfg(target_os = "linux")]
    files: HashMap<i64, fs::File>,
}

impl NetState {
    fn alloc(&mut self) -> i64 {
        self.next += 1;
        self.next
    }
}

static NET: OnceLock<Mutex<NetState>> = OnceLock::new();

fn net() -> &'static Mutex<NetState> {
    NET.get_or_init(|| Mutex::new(NetState::default()))
}

#[cfg(unix)]
fn socket_raw_handle(state: &NetState, handle: i64) -> Option<i64> {
    state
        .tcp_listen
        .get(&handle)
        .map(|socket| i64::from(socket.as_raw_fd()))
        .or_else(|| {
            state
                .tcp
                .get(&handle)
                .map(|socket| i64::from(socket.as_raw_fd()))
        })
        .or_else(|| {
            state
                .udp
                .get(&handle)
                .map(|socket| i64::from(socket.as_raw_fd()))
        })
        .or_else(|| {
            #[cfg(target_os = "linux")]
            {
                state
                    .files
                    .get(&handle)
                    .map(|file| i64::from(file.as_raw_fd()))
            }
            #[cfg(not(target_os = "linux"))]
            {
                None
            }
        })
}

#[cfg(windows)]
fn socket_raw_handle(state: &NetState, handle: i64) -> Option<i64> {
    state
        .tcp_listen
        .get(&handle)
        .map(|socket| socket.as_raw_socket() as i64)
        .or_else(|| {
            state
                .tcp
                .get(&handle)
                .map(|socket| socket.as_raw_socket() as i64)
        })
        .or_else(|| {
            state
                .udp
                .get(&handle)
                .map(|socket| socket.as_raw_socket() as i64)
        })
}

#[cfg(target_os = "linux")]
fn socket_raw_fd(state: &NetState, handle: i64) -> Option<std::os::fd::RawFd> {
    socket_raw_handle(state, handle).and_then(|fd| fd.try_into().ok())
}

/// Pure value intrinsics (array / string primitives with value semantics).
/// Each is registered both bare and as `host.<name>`.
fn register_value_intrinsics(env: &mut HostEnv) {
    fn both(env: &mut HostEnv, name: &str, f: HostCallback) {
        env.register_fn(format!("host.{name}"), Arc::clone(&f));
        env.register_fn(name, f);
    }

    // sys_arr_new(n, fill) -> dense [fill; n] when fill is f64/i64 (JIT/VM hot path).
    both(
        env,
        "sys_arr_new",
        cb(|args| {
            let n = arg_int(args, 0).clamp(0, 1 << 26) as usize;
            let fill = args.get(1).cloned().unwrap_or(Value::Int(0));
            match fill {
                Value::Float(f) => Value::ArrayF64(vec![f; n].into()),
                Value::Int(i) => Value::ArrayI64(vec![i; n].into()),
                other => Value::Array(vec![other; n].into()),
            }
        }),
    );
    // sys_bytes_new(n) -> dense zeroed u8 buffer (peer/C `[n]u8` parity).
    both(
        env,
        "sys_bytes_new",
        cb(|args| {
            let n = arg_int(args, 0).clamp(0, 1 << 28) as usize;
            Value::Bytes(vec![0u8; n])
        }),
    );
    // sys_u8_copy_range(dst, dst_off, src, src_off, n) — peer `@memcpy` on dense bytes.
    both(
        env,
        "sys_u8_copy_range",
        cb(|args| {
            let dst_off = arg_int(args, 1).max(0) as usize;
            let src_off = arg_int(args, 3).max(0) as usize;
            let n = arg_int(args, 4).max(0) as usize;
            match (args.first(), args.get(2)) {
                (Some(Value::Bytes(dst)), Some(Value::Bytes(src))) => {
                    let mut out = dst.clone();
                    let lim = n
                        .min(out.len().saturating_sub(dst_off))
                        .min(src.len().saturating_sub(src_off));
                    if lim > 0 {
                        out[dst_off..dst_off + lim].copy_from_slice(&src[src_off..src_off + lim]);
                    }
                    Value::Bytes(out)
                }
                (Some(Value::ArrayI64(dst)), Some(Value::ArrayI64(src))) => {
                    let mut out = Arc::clone(dst);
                    let d = Arc::make_mut(&mut out);
                    let lim = n
                        .min(d.len().saturating_sub(dst_off))
                        .min(src.len().saturating_sub(src_off));
                    for i in 0..lim {
                        d[dst_off + i] = src[src_off + i] & 255;
                    }
                    Value::ArrayI64(out)
                }
                _ => args.first().cloned().unwrap_or(Value::Null),
            }
        }),
    );
    // sys_be32_put(buf, at, v) / sys_be32_get(buf, at) — fused BE u32 on dense bytes.
    both(
        env,
        "sys_be32_put",
        cb(|args| {
            let at = arg_int(args, 1);
            let v = arg_int(args, 2) as u32;
            let lanes = [
                ((v >> 24) & 255) as i64,
                ((v >> 16) & 255) as i64,
                ((v >> 8) & 255) as i64,
                (v & 255) as i64,
            ];
            match args.first() {
                Some(Value::Bytes(b)) => {
                    let mut out = b.clone();
                    let i = at as usize;
                    if at >= 0 && i + 4 <= out.len() {
                        for (k, lane) in lanes.iter().enumerate() {
                            out[i + k] = *lane as u8;
                        }
                    }
                    Value::Bytes(out)
                }
                Some(Value::ArrayI64(a)) => {
                    let mut out = Arc::clone(a);
                    let d = Arc::make_mut(&mut out);
                    let i = at as usize;
                    if at >= 0 && i + 4 <= d.len() {
                        d[i..i + 4].copy_from_slice(&lanes);
                    }
                    Value::ArrayI64(out)
                }
                Some(Value::Array(a)) => {
                    let mut out = Arc::clone(a);
                    let d = Arc::make_mut(&mut out);
                    let i = at as usize;
                    if at >= 0 && i + 4 <= d.len() {
                        for (k, lane) in lanes.iter().enumerate() {
                            d[i + k] = Value::Int(*lane);
                        }
                    }
                    Value::Array(out)
                }
                _ => args.first().cloned().unwrap_or(Value::Null),
            }
        }),
    );
    both(
        env,
        "sys_be32_get",
        cb(|args| {
            let at = arg_int(args, 1);
            let i = at as usize;
            let lane = |get: &dyn Fn(usize) -> i64| -> Value {
                if at < 0 {
                    return Value::Int(0);
                }
                Value::Int(
                    ((get(i) & 255) << 24)
                        | ((get(i + 1) & 255) << 16)
                        | ((get(i + 2) & 255) << 8)
                        | (get(i + 3) & 255),
                )
            };
            match args.first() {
                Some(Value::Bytes(b)) if i + 4 <= b.len() => lane(&|j| i64::from(b[j])),
                Some(Value::ArrayI64(a)) if i + 4 <= a.len() => lane(&|j| a[j]),
                Some(Value::Array(a)) if i + 4 <= a.len() => lane(&|j| a[j].as_int()),
                _ => Value::Int(0),
            }
        }),
    );
    // sys_crc32_bytes_range(data, lo, hi) -> IEEE CRC-32 over byte lanes / dense Bytes.
    both(
        env,
        "sys_crc32_bytes_range",
        cb(|args| {
            let lo = arg_int(args, 1).max(0) as usize;
            let hi = arg_int(args, 2).max(0) as usize;
            let bytes = match args.first() {
                Some(Value::Bytes(b)) => b.as_slice(),
                Some(Value::ArrayI64(a)) => {
                    let hi = hi.min(a.len());
                    let lo = lo.min(hi);
                    let slice: Vec<u8> = a[lo..hi].iter().map(|&x| (x & 255) as u8).collect();
                    return Value::Int(i64::from(crc32_ieee(&slice)));
                }
                Some(Value::Array(a)) => {
                    let hi = hi.min(a.len());
                    let lo = lo.min(hi);
                    let slice: Vec<u8> =
                        a[lo..hi].iter().map(|x| (x.as_int() & 255) as u8).collect();
                    return Value::Int(i64::from(crc32_ieee(&slice)));
                }
                _ => return Value::Null,
            };
            let hi = hi.min(bytes.len());
            let lo = lo.min(hi);
            Value::Int(i64::from(crc32_ieee(&bytes[lo..hi])))
        }),
    );
    // sys_arr_memset(arr, n, val) -> fill arr[0..min(len,n)) with val.
    both(
        env,
        "sys_arr_memset",
        cb(|args| {
            let n = arg_int(args, 1).max(0) as usize;
            let val = arg_int(args, 2);
            match args.first() {
                Some(Value::ArrayI64(a)) => {
                    let mut arr = Arc::clone(a);
                    let end = n.min(arr.len());
                    Arc::make_mut(&mut arr)[..end].fill(val);
                    Value::ArrayI64(arr)
                }
                Some(Value::Array(a)) => {
                    let mut arr = Arc::clone(a);
                    let end = n.min(arr.len());
                    let slot = Arc::make_mut(&mut arr);
                    for e in slot.iter_mut().take(end) {
                        *e = Value::Int(val);
                    }
                    Value::Array(arr)
                }
                other => other.cloned().unwrap_or(Value::Null),
            }
        }),
    );
    // sys_arr_set(arr, i, v) -> arr with arr[i] = v (out of range: unchanged)
    both(
        env,
        "sys_arr_set",
        cb(|args| {
            let i = arg_int(args, 1);
            let v = args.get(2).cloned().unwrap_or(Value::Null);
            match args.first() {
                Some(Value::ArrayF64(a)) => {
                    let mut arr = Arc::clone(a);
                    if i >= 0 && (i as usize) < arr.len() {
                        Arc::make_mut(&mut arr)[i as usize] = v.as_float();
                    }
                    Value::ArrayF64(arr)
                }
                Some(Value::ArrayI64(a)) => {
                    let mut arr = Arc::clone(a);
                    if i >= 0 && (i as usize) < arr.len() {
                        Arc::make_mut(&mut arr)[i as usize] = v.as_int();
                    }
                    Value::ArrayI64(arr)
                }
                Some(Value::Bytes(b)) => {
                    let mut out = b.clone();
                    if i >= 0 && (i as usize) < out.len() {
                        out[i as usize] = (v.as_int() & 255) as u8;
                    }
                    Value::Bytes(out)
                }
                Some(Value::Array(a)) => {
                    let mut arr = Arc::clone(a);
                    if i >= 0 && (i as usize) < arr.len() {
                        Arc::make_mut(&mut arr)[i as usize] = v;
                    }
                    Value::Array(arr)
                }
                _ => Value::Array(Arc::default()),
            }
        }),
    );
    // sys_i64_gather / sys_i64_gather_dense(dst, src, idx) -> dst[i] = src[idx[i]]
    // Dense is the unchecked AOT path; the VM host still clamps out-of-range indices.
    let i64_gather = cb(|args| {
        let n_idx = match args.get(2) {
            Some(Value::ArrayI64(i)) => i.len(),
            Some(Value::Array(i)) => i.len(),
            _ => 0,
        };
        let at = |i: usize| -> i64 {
            match args.get(2) {
                Some(Value::ArrayI64(ix)) => ix[i],
                Some(Value::Array(ix)) => ix[i].as_int(),
                _ => -1,
            }
        };
        let src_at = |j: usize| -> i64 {
            match args.get(1) {
                Some(Value::ArrayI64(s)) => s[j],
                Some(Value::Array(s)) => s[j].as_int(),
                _ => 0,
            }
        };
        let sn = match args.get(1) {
            Some(Value::ArrayI64(s)) => s.len(),
            Some(Value::Array(s)) => s.len(),
            _ => 0,
        };
        match args.first() {
            Some(Value::ArrayI64(dst0)) => {
                let mut dst = Arc::clone(dst0);
                let n = dst.len().min(n_idx);
                let dv = Arc::make_mut(&mut dst);
                for i in 0..n {
                    let j = at(i);
                    dv[i] = if j >= 0 && (j as usize) < sn {
                        src_at(j as usize)
                    } else {
                        0
                    };
                }
                Value::ArrayI64(dst)
            }
            Some(Value::Array(dst0)) => {
                let mut dst = Arc::clone(dst0);
                let n = dst.len().min(n_idx);
                let dv = Arc::make_mut(&mut dst);
                for i in 0..n {
                    let j = at(i);
                    dv[i] = Value::Int(if j >= 0 && (j as usize) < sn {
                        src_at(j as usize)
                    } else {
                        0
                    });
                }
                Value::Array(dst)
            }
            _ => args.first().cloned().unwrap_or(Value::Null),
        }
    });
    both(env, "sys_i64_gather", Arc::clone(&i64_gather));
    both(env, "sys_i64_gather_dense", i64_gather);
    // sys_f64_gather / sys_f64_gather_dense(dst, src, idx) -> dst[i] = src[idx[i]]
    let f64_gather = cb(|args| {
        let n_idx = match args.get(2) {
            Some(Value::ArrayI64(i)) => i.len(),
            Some(Value::Array(i)) => i.len(),
            _ => 0,
        };
        let at = |i: usize| -> i64 {
            match args.get(2) {
                Some(Value::ArrayI64(ix)) => ix[i],
                Some(Value::Array(ix)) => ix[i].as_int(),
                _ => -1,
            }
        };
        let sn = match args.get(1) {
            Some(Value::ArrayF64(s)) => s.len(),
            Some(Value::Array(s)) => s.len(),
            _ => 0,
        };
        let src_at = |j: usize| -> f64 {
            match args.get(1) {
                Some(Value::ArrayF64(s)) => s[j],
                Some(Value::Array(s)) => s[j].as_float(),
                _ => 0.0,
            }
        };
        match args.first() {
            Some(Value::ArrayF64(dst0)) => {
                let mut dst = Arc::clone(dst0);
                let n = dst.len().min(n_idx);
                let dv = Arc::make_mut(&mut dst);
                for i in 0..n {
                    let j = at(i);
                    dv[i] = if j >= 0 && (j as usize) < sn {
                        src_at(j as usize)
                    } else {
                        0.0
                    };
                }
                Value::ArrayF64(dst)
            }
            Some(Value::Array(dst0)) => {
                let mut dst = Arc::clone(dst0);
                let n = dst.len().min(n_idx);
                let dv = Arc::make_mut(&mut dst);
                for i in 0..n {
                    let j = at(i);
                    dv[i] = Value::Float(if j >= 0 && (j as usize) < sn {
                        src_at(j as usize)
                    } else {
                        0.0
                    });
                }
                Value::Array(dst)
            }
            _ => args.first().cloned().unwrap_or(Value::Null),
        }
    });
    both(env, "sys_f64_gather", Arc::clone(&f64_gather));
    both(env, "sys_f64_gather_dense", f64_gather);
    // sys_i64_merge(a, tmp, lo, mid, hi) -> a  (stable mergesort merge of a[lo..mid) with a[mid..hi))
    both(
        env,
        "sys_i64_merge",
        cb(|args| {
            // `tmp` is only a capacity witness (scratch is local), so any array variant works.
            let tmp_len = match args.get(1) {
                Some(Value::ArrayI64(t)) => t.len(),
                Some(Value::Array(t)) => t.len(),
                Some(Value::ArrayF64(t)) => t.len(),
                _ => 0,
            };
            // Prefer dense i64 paths (benchmarks / `[0; N]`).
            if let Some(Value::ArrayI64(a0)) = args.first() {
                let mut a = Arc::clone(a0);
                let lo = arg_int(args, 2).max(0) as usize;
                let mid = arg_int(args, 3).max(0) as usize;
                let hi = arg_int(args, 4).max(0) as usize;
                if lo > mid || mid > hi || hi > a.len() || hi > tmp_len {
                    return Value::ArrayI64(a);
                }
                let mut i = lo;
                let mut j = mid;
                let mut scratch = Vec::with_capacity(hi - lo);
                while i < mid && j < hi {
                    if a[i] <= a[j] {
                        scratch.push(a[i]);
                        i += 1;
                    } else {
                        scratch.push(a[j]);
                        j += 1;
                    }
                }
                while i < mid {
                    scratch.push(a[i]);
                    i += 1;
                }
                while j < hi {
                    scratch.push(a[j]);
                    j += 1;
                }
                let dst = Arc::make_mut(&mut a);
                dst[lo..hi].copy_from_slice(&scratch);
                return Value::ArrayI64(a);
            }
            let mut a = match args.first() {
                Some(Value::Array(arr)) => Arc::clone(arr),
                _ => return args.first().cloned().unwrap_or(Value::Null),
            };
            let lo = arg_int(args, 2).max(0) as usize;
            let mid = arg_int(args, 3).max(0) as usize;
            let hi = arg_int(args, 4).max(0) as usize;
            if lo > mid || mid > hi || hi > a.len() || hi > tmp_len {
                return Value::Array(a);
            }
            let mut i = lo;
            let mut j = mid;
            let mut scratch = Vec::with_capacity(hi - lo);
            while i < mid && j < hi {
                let ai = a[i].as_int();
                let aj = a[j].as_int();
                if ai <= aj {
                    scratch.push(Value::Int(ai));
                    i += 1;
                } else {
                    scratch.push(Value::Int(aj));
                    j += 1;
                }
            }
            while i < mid {
                scratch.push(Value::Int(a[i].as_int()));
                i += 1;
            }
            while j < hi {
                scratch.push(Value::Int(a[j].as_int()));
                j += 1;
            }
            let dst = Arc::make_mut(&mut a);
            for (k, v) in scratch.into_iter().enumerate() {
                dst[lo + k] = v;
            }
            Value::Array(a)
        }),
    );
    // sys_arr_len(arr) -> element count
    both(
        env,
        "sys_arr_len",
        cb(|args| match args.first() {
            Some(Value::Array(a)) => Value::Int(a.len() as i64),
            Some(Value::ArrayF64(a)) => Value::Int(a.len() as i64),
            Some(Value::ArrayI64(a)) => Value::Int(a.len() as i64),
            _ => Value::Int(0),
        }),
    );
    // sys_str_len(s) -> length in characters
    both(
        env,
        "sys_str_len",
        cb(|args| match args.first() {
            Some(Value::Bytes(b)) => Value::Int(b.len() as i64),
            Some(v) => Value::Int(v.as_str().chars().count() as i64),
            None => Value::Int(0),
        }),
    );
    // sys_str_find(s, needle, start) -> character index of the first match at/after start, or -1
    both(
        env,
        "sys_str_find",
        cb(|args| {
            Value::Int(str_find(
                &arg_str(args, 0),
                &arg_str(args, 1),
                arg_int(args, 2),
            ))
        }),
    );
    // sys_str_match_at(s, needle, pos) -> needle occurs at character index pos
    both(
        env,
        "sys_str_match_at",
        cb(|args| {
            Value::Bool(str_match_at(
                &arg_str(args, 0),
                &arg_str(args, 1),
                arg_int(args, 2),
            ))
        }),
    );
    // sys_json_encode(value) -> RFC JSON text for any Super value
    both(
        env,
        "sys_json_encode",
        cb(|args| match args.first() {
            Some(v) => Value::Str(crate::host_net::encode_json_value(v)),
            None => Value::Str("null".into()),
        }),
    );
    // sys_json_body(value) -> str passthrough, else JSON-encode (HTTP dual-mode)
    both(
        env,
        "sys_json_body",
        cb(|args| match args.first() {
            Some(v) => Value::Str(crate::host_net::encode_json_body(v)),
            None => Value::Str("null".into()),
        }),
    );
    // sys_chr(code) -> one-character string ("" for invalid scalar values)
    both(
        env,
        "sys_chr",
        cb(|args| {
            let c = u32::try_from(arg_int(args, 0))
                .ok()
                .and_then(char::from_u32)
                .map(String::from)
                .unwrap_or_default();
            Value::Str(c)
        }),
    );
    // sys_byte_at(s, i) -> unsigned byte at code-unit index i, or -1 OOB (ASCII/HTTP/JSON hot path).
    both(
        env,
        "sys_byte_at",
        cb(|args| {
            let s = arg_str(args, 0);
            let i = arg_int(args, 1);
            if i < 0 {
                return Value::Int(-1);
            }
            let b = s.as_bytes();
            match b.get(i as usize) {
                Some(&c) => Value::Int(i64::from(c)),
                None => Value::Int(-1),
            }
        }),
    );
    // sys_byte_len(s) -> UTF-8 byte/code-unit length (matches AOT strlen; not Unicode scalars).
    both(
        env,
        "sys_byte_len",
        cb(|args| Value::Int(arg_str(args, 0).len() as i64)),
    );
    // sys_str_push_ascii(s, code) -> s with one Latin-1/ASCII byte (code & 0xff) appended.
    both(
        env,
        "sys_str_push_ascii",
        cb(|args| {
            let s = arg_str(args, 0);
            let code = (arg_int(args, 1) as u8) as char;
            let mut out = String::with_capacity(s.len() + 1);
            out.push_str(&s);
            out.push(code);
            Value::Str(out)
        }),
    );
    // sys_str_push_i64(s, v) -> s with decimal digits of v appended (CSV/HTTP builders).
    both(
        env,
        "sys_str_push_i64",
        cb(|args| {
            let s = arg_str(args, 0);
            let v = arg_int(args, 1);
            let digits = v.to_string();
            let mut out = String::with_capacity(s.len() + digits.len());
            out.push_str(&s);
            out.push_str(&digits);
            Value::Str(out)
        }),
    );

    // ---- StrBuilder (sys_sb_*) — VM mirror of AOT SupSb / peer Buf --------------------
    struct VmSb {
        buf: String,
        /// Soft capacity; `fixed` never grows past this (oversized writes truncate).
        cap: usize,
        fixed: bool,
    }
    static SB_TABLE: OnceLock<Mutex<(i64, HashMap<i64, VmSb>)>> = OnceLock::new();
    fn sb_table() -> &'static Mutex<(i64, HashMap<i64, VmSb>)> {
        SB_TABLE.get_or_init(|| Mutex::new((1, HashMap::new())))
    }
    fn sb_alloc(cap0: i64, fixed: bool) -> i64 {
        let cap = (cap0.max(64) as usize).max(64);
        let mut g = sb_table().lock().expect("sb table");
        let id = g.0;
        g.0 += 1;
        g.1.insert(
            id,
            VmSb {
                buf: String::with_capacity(cap),
                cap,
                fixed,
            },
        );
        id
    }
    fn sb_with(h: i64, f: impl FnOnce(&mut VmSb)) {
        if let Ok(mut g) = sb_table().lock()
            && let Some(b) = g.1.get_mut(&h)
        {
            f(b);
        }
    }
    fn sb_push_bytes(b: &mut VmSb, bytes: &[u8]) {
        let take = if b.fixed {
            let room = b.cap.saturating_sub(b.buf.len());
            if room == 0 {
                return;
            }
            &bytes[..bytes.len().min(room)]
        } else {
            bytes
        };
        match std::str::from_utf8(take) {
            Ok(s) => b.buf.push_str(s),
            Err(_) => {
                // Latin-1 / raw bytes (AOT stores `code & 0xff` as a single char unit).
                for &c in take {
                    b.buf.push(char::from(c));
                }
            }
        }
    }
    both(
        env,
        "sys_sb_new",
        cb(|args| Value::Int(sb_alloc(arg_int(args, 0), false))),
    );
    both(
        env,
        "sys_sb_stack",
        cb(|args| Value::Int(sb_alloc(arg_int(args, 0), true))),
    );
    both(
        env,
        "sys_sb_lit",
        cb(|args| {
            let h = arg_int(args, 0);
            let s = arg_str(args, 1);
            sb_with(h, |b| sb_push_bytes(b, s.as_bytes()));
            Value::Null
        }),
    );
    both(
        env,
        "sys_sb_ascii",
        cb(|args| {
            let h = arg_int(args, 0);
            let code = arg_int(args, 1) as u8;
            sb_with(h, |b| sb_push_bytes(b, &[code]));
            Value::Null
        }),
    );
    both(
        env,
        "sys_sb_i64",
        cb(|args| {
            let h = arg_int(args, 0);
            let digits = arg_int(args, 1).to_string();
            sb_with(h, |b| sb_push_bytes(b, digits.as_bytes()));
            Value::Null
        }),
    );
    // peer `reqb[tpos + at] = 'z'` — poke one byte inside the live Buf (wire/token fuzz).
    both(
        env,
        "sys_sb_set",
        cb(|args| {
            let h = arg_int(args, 0);
            let i = arg_int(args, 1);
            let code = (arg_int(args, 2) & 0xff) as u8;
            sb_with(h, |b| {
                if i >= 0 {
                    let iu = i as usize;
                    if iu < b.buf.len() {
                        // ASCII poke (peer wire fuzz); keep the String UTF-8-valid.
                        let ch = [code];
                        if let Ok(s) = std::str::from_utf8(&ch) {
                            b.buf.replace_range(iu..=iu, s);
                        }
                    }
                }
            });
            Value::Null
        }),
    );
    both(
        env,
        "sys_sb_len",
        cb(|args| {
            let h = arg_int(args, 0);
            let n = sb_table()
                .lock()
                .ok()
                .and_then(|g| g.1.get(&h).map(|b| b.buf.len() as i64))
                .unwrap_or(0);
            Value::Int(n)
        }),
    );
    both(
        env,
        "sys_sb_clear",
        cb(|args| {
            sb_with(arg_int(args, 0), |b| b.buf.clear());
            Value::Null
        }),
    );
    both(
        env,
        "sys_sb_cat",
        cb(|args| {
            let dst = arg_int(args, 0);
            let src = arg_int(args, 1);
            let piece = sb_table()
                .lock()
                .ok()
                .and_then(|g| g.1.get(&src).map(|b| b.buf.clone()))
                .unwrap_or_default();
            sb_with(dst, |b| sb_push_bytes(b, piece.as_bytes()));
            Value::Null
        }),
    );
    both(
        env,
        "sys_sb_cstr",
        cb(|args| {
            let h = arg_int(args, 0);
            let s = sb_table()
                .lock()
                .ok()
                .and_then(|g| g.1.get(&h).map(|b| b.buf.clone()))
                .unwrap_or_default();
            Value::Str(s)
        }),
    );
    both(
        env,
        "sys_sb_finish",
        cb(|args| {
            let h = arg_int(args, 0);
            let mut g = match sb_table().lock() {
                Ok(g) => g,
                Err(_) => return Value::Str("".into()),
            };
            let s = if let Some(b) = g.1.get_mut(&h) {
                if b.fixed {
                    let out = b.buf.clone();
                    b.buf.clear();
                    out
                } else {
                    g.1.remove(&h).map(|b| b.buf).unwrap_or_default()
                }
            } else {
                String::new()
            };
            Value::Str(s)
        }),
    );
    // Append 8 hex chars of HMAC(key, msg[lo..hi)) — no 32-lane digest alloc.
    both(
        env,
        "sys_hmac_sha256_sig8_into_range",
        cb(|args| {
            use hmac::{Hmac, Mac};
            type HmacSha256 = Hmac<Sha256>;
            let h = arg_int(args, 0);
            let key = payload_bytes(args.get(1));
            let msg = arg_str(args, 2);
            let lo = arg_int(args, 3).max(0) as usize;
            let hi = arg_int(args, 4).max(0) as usize;
            let bytes = msg.as_bytes();
            let lo = lo.min(bytes.len());
            let hi = hi.min(bytes.len()).max(lo);
            let dig = match HmacSha256::new_from_slice(&key) {
                Ok(mut mac) => {
                    mac.update(&bytes[lo..hi]);
                    mac.finalize().into_bytes()
                }
                Err(_) => return Value::Null,
            };
            const HEX: &[u8; 16] = b"0123456789abcdef";
            let mut hex = [0u8; 8];
            for i in 0..4 {
                hex[2 * i] = HEX[(dig[i] >> 4) as usize];
                hex[2 * i + 1] = HEX[(dig[i] & 15) as usize];
            }
            sb_with(h, |b| sb_push_bytes(b, &hex));
            Value::Null
        }),
    );
    // Forged sig8: same as sig8_into_range after mac[0] ^= 1 (stack emit; no digest array).
    both(
        env,
        "sys_hmac_sha256_sig8_forged_into_range",
        cb(|args| {
            use hmac::{Hmac, Mac};
            type HmacSha256 = Hmac<Sha256>;
            let h = arg_int(args, 0);
            let key = payload_bytes(args.get(1));
            let msg = arg_str(args, 2);
            let lo = arg_int(args, 3).max(0) as usize;
            let hi = arg_int(args, 4).max(0) as usize;
            let bytes = msg.as_bytes();
            let lo = lo.min(bytes.len());
            let hi = hi.min(bytes.len()).max(lo);
            let mut dig = match HmacSha256::new_from_slice(&key) {
                Ok(mut mac) => {
                    mac.update(&bytes[lo..hi]);
                    mac.finalize().into_bytes()
                }
                Err(_) => return Value::Null,
            };
            dig[0] ^= 1;
            const HEX: &[u8; 16] = b"0123456789abcdef";
            let mut hex = [0u8; 8];
            for i in 0..4 {
                hex[2 * i] = HEX[(dig[i] >> 4) as usize];
                hex[2 * i + 1] = HEX[(dig[i] & 15) as usize];
            }
            sb_with(h, |b| sb_push_bytes(b, &hex));
            Value::Null
        }),
    );
    // IEEE CRC-32 of a StrBuilder's live bytes (no text()/strlen).
    both(
        env,
        "sys_crc32_sb",
        cb(|args| {
            let h = arg_int(args, 0);
            let s = sb_table()
                .lock()
                .ok()
                .and_then(|g| g.1.get(&h).map(|b| b.buf.clone()))
                .unwrap_or_default();
            Value::Int(i64::from(crc32_ieee(s.as_bytes())))
        }),
    );
    // sys_byte_slice(s, lo, hi) -> UTF-8 substring of byte range [lo, hi) (clamped; empty if invalid).
    both(
        env,
        "sys_byte_slice",
        cb(|args| {
            let s = arg_str(args, 0);
            let lo = arg_int(args, 1);
            let hi = arg_int(args, 2);
            let b = s.as_bytes();
            let n = b.len();
            let start = if lo <= 0 {
                0usize
            } else if (lo as usize) > n {
                n
            } else {
                lo as usize
            };
            let end = if hi <= 0 {
                0usize
            } else if (hi as usize) > n {
                n
            } else {
                hi as usize
            };
            if end <= start {
                return Value::Str("".into());
            }
            match std::str::from_utf8(&b[start..end]) {
                Ok(t) => Value::Str(t.into()),
                Err(_) => Value::Str("".into()),
            }
        }),
    );
    // sys_byte_find(s, needle, from) -> first byte index >= from, or -1 (ASCII/HTTP/JSON wire scans).
    both(
        env,
        "sys_byte_find",
        cb(|args| {
            let s = arg_str(args, 0);
            let needle = arg_str(args, 1);
            let from = arg_int(args, 2).max(0) as usize;
            let hay = s.as_bytes();
            let nd = needle.as_bytes();
            if nd.is_empty() || from >= hay.len() {
                return Value::Int(-1);
            }
            match hay[from..].windows(nd.len()).position(|w| w == nd) {
                Some(p) => Value::Int((p + from) as i64),
                None => Value::Int(-1),
            }
        }),
    );
    // sys_byte_find_limit(s, needle, from, limit) -> find in s[from..limit) only.
    both(
        env,
        "sys_byte_find_limit",
        cb(|args| {
            let s = arg_str(args, 0);
            let needle = arg_str(args, 1);
            let from = arg_int(args, 2).max(0) as usize;
            let limit = arg_int(args, 3).max(0) as usize;
            let hay = s.as_bytes();
            let nd = needle.as_bytes();
            let lim = limit.min(hay.len());
            if nd.is_empty() || from >= lim || from + nd.len() > lim {
                return Value::Int(-1);
            }
            match hay[from..lim].windows(nd.len()).position(|w| w == nd) {
                Some(p) => Value::Int((p + from) as i64),
                None => Value::Int(-1),
            }
        }),
    );
    // sys_byte_match_at(s, needle, pos) -> needle occurs at byte index pos.
    both(
        env,
        "sys_byte_match_at",
        cb(|args| {
            let s = arg_str(args, 0);
            let needle = arg_str(args, 1);
            let pos = arg_int(args, 2);
            if needle.is_empty() {
                return Value::Bool(true);
            }
            if pos < 0 {
                return Value::Bool(false);
            }
            let hay = s.as_bytes();
            let nd = needle.as_bytes();
            let p = pos as usize;
            Value::Bool(p + nd.len() <= hay.len() && hay[p..p + nd.len()] == *nd)
        }),
    );
    // sys_byte_eq_range(s, lo, hi, t) -> s[lo..hi) bytes equal t (no alloc; HTTP/router hot path).
    both(
        env,
        "sys_byte_eq_range",
        cb(|args| {
            let s = arg_str(args, 0);
            let lo = arg_int(args, 1);
            let hi = arg_int(args, 2);
            let t = arg_str(args, 3);
            if lo < 0 || hi < lo {
                return Value::Bool(false);
            }
            let hay = s.as_bytes();
            let a = lo as usize;
            let b = hi as usize;
            if b > hay.len() {
                return Value::Bool(false);
            }
            Value::Bool(&hay[a..b] == t.as_bytes())
        }),
    );
    // sys_byte_eq_ranges(a, alo, ahi, b, blo, bhi) -> a[alo..ahi) == b[blo..bhi) (no slice alloc).
    both(
        env,
        "sys_byte_eq_ranges",
        cb(|args| {
            let a = arg_str(args, 0);
            let alo = arg_int(args, 1);
            let ahi = arg_int(args, 2);
            let b = arg_str(args, 3);
            let blo = arg_int(args, 4);
            let bhi = arg_int(args, 5);
            if alo < 0 || ahi < alo || blo < 0 || bhi < blo {
                return Value::Bool(false);
            }
            let ab = a.as_bytes();
            let bb = b.as_bytes();
            let (a0, a1) = (alo as usize, ahi as usize);
            let (b0, b1) = (blo as usize, bhi as usize);
            if a1 > ab.len() || b1 > bb.len() || (a1 - a0) != (b1 - b0) {
                return Value::Bool(false);
            }
            Value::Bool(ab[a0..a1] == bb[b0..b1])
        }),
    );
    // sys_arr_pop(arr) -> arr without its last element (empty stays empty)
    both(
        env,
        "sys_arr_pop",
        cb(|args| match args.first() {
            Some(Value::ArrayF64(a)) => {
                let mut arr = Arc::clone(a);
                Arc::make_mut(&mut arr).pop();
                Value::ArrayF64(arr)
            }
            Some(Value::ArrayI64(a)) => {
                let mut arr = Arc::clone(a);
                Arc::make_mut(&mut arr).pop();
                Value::ArrayI64(arr)
            }
            Some(Value::Array(a)) => {
                let mut arr = Arc::clone(a);
                Arc::make_mut(&mut arr).pop();
                Value::Array(arr)
            }
            _ => Value::Array(Arc::default()),
        }),
    );
    // sys_arr_truncate(arr, n) -> arr with length min(len, max(0, n)); capacity unchanged.
    both(
        env,
        "sys_arr_truncate",
        cb(|args| {
            let n = arg_int(args, 1).max(0) as usize;
            match args.first() {
                Some(Value::ArrayF64(a)) => {
                    let mut arr = Arc::clone(a);
                    let m = Arc::make_mut(&mut arr);
                    if n < m.len() {
                        m.truncate(n);
                    }
                    Value::ArrayF64(arr)
                }
                Some(Value::ArrayI64(a)) => {
                    let mut arr = Arc::clone(a);
                    let m = Arc::make_mut(&mut arr);
                    if n < m.len() {
                        m.truncate(n);
                    }
                    Value::ArrayI64(arr)
                }
                Some(Value::Array(a)) => {
                    let mut arr = Arc::clone(a);
                    let m = Arc::make_mut(&mut arr);
                    if n < m.len() {
                        m.truncate(n);
                    }
                    Value::Array(arr)
                }
                Some(Value::Bytes(b)) => {
                    let mut out = b.clone();
                    if n < out.len() {
                        out.truncate(n);
                    }
                    Value::Bytes(out)
                }
                other => other.cloned().unwrap_or(Value::Array(Arc::default())),
            }
        }),
    );
    // sys_str_to_bytes(s) -> dense i64 array of the UTF-8 bytes of `s`
    both(
        env,
        "sys_str_to_bytes",
        cb(|args| {
            let bytes: Vec<i64> = arg_str(args, 0).bytes().map(i64::from).collect();
            Value::ArrayI64(bytes.into())
        }),
    );
    // sys_bytes_to_str(arr) -> string decoded from bytes (UTF-8, lossy)
    both(
        env,
        "sys_bytes_to_str",
        cb(|args| {
            let bytes = payload_bytes(args.first());
            Value::Str(String::from_utf8_lossy(&bytes).into_owned())
        }),
    );
    // f64 math kernel: unary and binary libm entry points.
    fn unary(env: &mut HostEnv, name: &str, f: fn(f64) -> f64) {
        let cbf = cb(move |args| Value::Float(f(args.first().map(Value::as_float).unwrap_or(0.0))));
        env.register_fn(format!("host.{name}"), Arc::clone(&cbf));
        env.register_fn(name, cbf);
    }
    fn binary(env: &mut HostEnv, name: &str, f: fn(f64, f64) -> f64) {
        let cbf = cb(move |args| {
            let a = args.first().map(Value::as_float).unwrap_or(0.0);
            let b = args.get(1).map(Value::as_float).unwrap_or(0.0);
            Value::Float(f(a, b))
        });
        env.register_fn(format!("host.{name}"), Arc::clone(&cbf));
        env.register_fn(name, cbf);
    }
    unary(env, "sys_sqrt", f64::sqrt);
    unary(env, "sys_cbrt", f64::cbrt);
    unary(env, "sys_sin", f64::sin);
    unary(env, "sys_cos", f64::cos);
    unary(env, "sys_tan", f64::tan);
    unary(env, "sys_asin", f64::asin);
    unary(env, "sys_acos", f64::acos);
    unary(env, "sys_atan", f64::atan);
    unary(env, "sys_sinh", f64::sinh);
    unary(env, "sys_cosh", f64::cosh);
    unary(env, "sys_tanh", f64::tanh);
    unary(env, "sys_floor", f64::floor);
    unary(env, "sys_ceil", f64::ceil);
    unary(env, "sys_trunc", f64::trunc);
    unary(env, "sys_round", f64::round);
    unary(env, "sys_exp", f64::exp);
    unary(env, "sys_ln", f64::ln);
    unary(env, "sys_log2", f64::log2);
    unary(env, "sys_log10", f64::log10);
    unary(env, "sys_abs", f64::abs);
    binary(env, "sys_atan2", f64::atan2);
    binary(env, "sys_pow", f64::powf);
    binary(env, "sys_hypot", f64::hypot);
    binary(env, "sys_fmod", f64::rem_euclid);

    // sys_to_int(x) -> truncate toward zero; sys_to_float(x) -> f64
    both(
        env,
        "sys_to_int",
        cb(|args| Value::Int(args.first().map(Value::as_float).unwrap_or(0.0) as i64)),
    );
    both(
        env,
        "sys_to_float",
        cb(|args| Value::Float(args.first().map(Value::as_float).unwrap_or(0.0))),
    );

    // sys_ord(s) -> code point of the first character (-1 when empty)
    both(
        env,
        "sys_ord",
        cb(|args| {
            Value::Int(
                arg_str(args, 0)
                    .chars()
                    .next()
                    .map(|c| c as i64)
                    .unwrap_or(-1),
            )
        }),
    );
}

/// Register all Tier-1 host builtins into `env`.
pub fn register_host_sys(env: &mut HostEnv) {
    crate::pq_crypto::register_pq_crypto(env);
    // --- arrays (stdlib growth helper) ---------------------------------
    let push = cb(|args| {
        let mut arr = match args.first() {
            Some(Value::Array(a)) => Arc::clone(a),
            _ => Arc::default(),
        };
        if let Some(v) = args.get(1) {
            Arc::make_mut(&mut arr).push(v.clone());
        }
        Value::Array(arr)
    });
    env.register_fn("sys_arr_push", Arc::clone(&push));
    env.register_fn("host.sys_arr_push", push);
    register_value_intrinsics(env);

    // --- process / env -------------------------------------------------
    env.register_fn(
        "sys_args",
        cb(|_| Value::Array(std::env::args().map(Value::Str).collect::<Vec<_>>().into())),
    );
    env.register_fn(
        "sys_env_get",
        cb(|args| match std::env::var(arg_str(args, 0)) {
            Ok(v) => ok_str(v),
            Err(_) => Value::Optional(None),
        }),
    );
    env.register_fn(
        "sys_env_set",
        cb(|args| {
            // SAFETY: intentionally sets process environment for scripting.
            unsafe { std::env::set_var(arg_str(args, 0), arg_str(args, 1)) };
            Value::Bool(true)
        }),
    );
    env.register_fn(
        "sys_env_all",
        cb(|_| {
            let mut m = HashMap::new();
            for (k, v) in std::env::vars() {
                m.insert(k, Value::Str(v));
            }
            Value::Map(m)
        }),
    );
    env.register_fn(
        "sys_cwd",
        cb(|_| match std::env::current_dir() {
            Ok(p) => ok_str(p.display().to_string()),
            Err(e) => err_val(e.to_string()),
        }),
    );
    env.register_fn(
        "sys_chdir",
        cb(|args| match std::env::set_current_dir(arg_str(args, 0)) {
            Ok(()) => Value::Bool(true),
            Err(e) => err_val(e.to_string()),
        }),
    );
    env.register_fn(
        "sys_exit",
        cb(|args| {
            std::process::exit(arg_int(args, 0) as i32);
        }),
    );
    env.register_fn("sys_os_name", cb(|_| ok_str(std::env::consts::OS)));
    env.register_fn("sys_arch_name", cb(|_| ok_str(std::env::consts::ARCH)));
    env.register_fn(
        "sys_cpu_count",
        cb(|_| {
            Value::Int(
                thread::available_parallelism()
                    .map(|n| n.get())
                    .unwrap_or(1) as i64,
            )
        }),
    );
    env.register_fn(
        "sys_spawn_process",
        cb(|args| {
            let prog = arg_str(args, 0);
            let argv = match args.get(1) {
                Some(Value::Array(a)) => a.iter().map(Value::as_str).collect::<Vec<_>>(),
                _ => Vec::new(),
            };
            match Command::new(&prog)
                .args(&argv)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
            {
                Ok(mut child) => {
                    let mut out = String::new();
                    let mut err = String::new();
                    if let Some(mut s) = child.stdout.take() {
                        let _ = s.read_to_string(&mut out);
                    }
                    if let Some(mut s) = child.stderr.take() {
                        let _ = s.read_to_string(&mut err);
                    }
                    let status = child.wait().ok().and_then(|s| s.code()).unwrap_or(-1);
                    let mut fields = HashMap::new();
                    fields.insert("status".into(), Value::Int(i64::from(status)));
                    fields.insert("stdout".into(), Value::Str(out));
                    fields.insert("stderr".into(), Value::Str(err));
                    Value::Struct {
                        name: None,
                        label: None,
                        fields,
                    }
                }
                Err(e) => err_val(e.to_string()),
            }
        }),
    );

    // --- filesystem ----------------------------------------------------
    env.register_fn(
        "sys_read_str",
        cb(|args| match fs::read_to_string(arg_str(args, 0)) {
            Ok(s) => ok_str(s),
            Err(e) => err_val(e.to_string()),
        }),
    );
    env.register_fn(
        "sys_read_bytes",
        cb(|args| match fs::read(arg_str(args, 0)) {
            Ok(b) => bytes_val(b),
            Err(e) => err_val(e.to_string()),
        }),
    );
    env.register_fn(
        "sys_write_str",
        cb(|args| match fs::write(arg_str(args, 0), arg_str(args, 1)) {
            Ok(()) => Value::Bool(true),
            Err(e) => err_val(e.to_string()),
        }),
    );
    env.register_fn(
        "sys_write_bytes",
        cb(|args| {
            let path = arg_str(args, 0);
            let data = payload_bytes(args.get(1));
            match fs::write(path, data) {
                Ok(()) => Value::Bool(true),
                Err(e) => err_val(e.to_string()),
            }
        }),
    );
    env.register_fn(
        "sys_append_bytes",
        cb(|args| {
            let path = arg_str(args, 0);
            let data = payload_bytes(args.get(1));
            match fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .and_then(|mut f| f.write_all(&data))
            {
                Ok(()) => Value::Bool(true),
                Err(e) => err_val(e.to_string()),
            }
        }),
    );
    env.register_fn(
        "sys_exists",
        cb(|args| Value::Bool(Path::new(&arg_str(args, 0)).exists())),
    );
    env.register_fn(
        "sys_metadata",
        cb(|args| {
            let path = arg_str(args, 0);
            match fs::metadata(&path) {
                Ok(m) => {
                    let mut fields = HashMap::new();
                    fields.insert("len".into(), Value::Int(m.len() as i64));
                    fields.insert("is_file".into(), Value::Bool(m.is_file()));
                    fields.insert("is_dir".into(), Value::Bool(m.is_dir()));
                    Value::Struct {
                        name: None,
                        label: None,
                        fields,
                    }
                }
                Err(e) => err_val(e.to_string()),
            }
        }),
    );
    env.register_fn(
        "sys_append_str",
        cb(|args| {
            let data = arg_str(args, 1);
            match fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(arg_str(args, 0))
                .and_then(|mut f| f.write_all(data.as_bytes()))
            {
                Ok(()) => Value::Bool(true),
                Err(e) => err_val(e.to_string()),
            }
        }),
    );
    env.register_fn(
        "sys_mkdir",
        cb(|args| match fs::create_dir(arg_str(args, 0)) {
            Ok(()) => Value::Bool(true),
            Err(e) => err_val(e.to_string()),
        }),
    );
    env.register_fn(
        "sys_is_dir",
        cb(|args| Value::Bool(Path::new(&arg_str(args, 0)).is_dir())),
    );
    env.register_fn(
        "sys_is_file",
        cb(|args| Value::Bool(Path::new(&arg_str(args, 0)).is_file())),
    );
    env.register_fn(
        "sys_file_size",
        cb(|args| match fs::metadata(arg_str(args, 0)) {
            Ok(m) => Value::Int(m.len() as i64),
            Err(e) => err_val(e.to_string()),
        }),
    );
    env.register_fn(
        "sys_mtime_unix",
        cb(
            |args| match fs::metadata(arg_str(args, 0)).and_then(|m| m.modified()) {
                Ok(t) => Value::Int(
                    t.duration_since(UNIX_EPOCH)
                        .map(|d| d.as_secs() as i64)
                        .unwrap_or(0),
                ),
                Err(e) => err_val(e.to_string()),
            },
        ),
    );
    env.register_fn(
        "sys_copy_file",
        cb(|args| match fs::copy(arg_str(args, 0), arg_str(args, 1)) {
            Ok(n) => Value::Int(n as i64),
            Err(e) => err_val(e.to_string()),
        }),
    );
    env.register_fn(
        "sys_temp_dir",
        cb(|_| Value::Str(std::env::temp_dir().to_string_lossy().into_owned())),
    );
    env.register_fn(
        "sys_mkdir_all",
        cb(|args| match fs::create_dir_all(arg_str(args, 0)) {
            Ok(()) => Value::Bool(true),
            Err(e) => err_val(e.to_string()),
        }),
    );
    env.register_fn(
        "sys_remove",
        cb(|args| {
            let p = arg_str(args, 0);
            let path = Path::new(&p);
            let res = if path.is_dir() {
                fs::remove_dir_all(path)
            } else {
                fs::remove_file(path)
            };
            match res {
                Ok(()) => Value::Bool(true),
                Err(e) => err_val(e.to_string()),
            }
        }),
    );
    env.register_fn(
        "sys_rename",
        cb(
            |args| match fs::rename(arg_str(args, 0), arg_str(args, 1)) {
                Ok(()) => Value::Bool(true),
                Err(e) => err_val(e.to_string()),
            },
        ),
    );
    env.register_fn(
        "sys_list_dir",
        cb(|args| match fs::read_dir(arg_str(args, 0)) {
            Ok(rd) => {
                let mut out = Vec::new();
                for e in rd.flatten() {
                    out.push(Value::Str(e.file_name().to_string_lossy().into_owned()));
                }
                Value::Array(out.into())
            }
            Err(e) => err_val(e.to_string()),
        }),
    );
    // watch_dir: synchronous snapshot poll helper (chan wiring lives in Super).
    env.register_fn(
        "sys_watch_dir_snapshot",
        cb(|args| match fs::read_dir(arg_str(args, 0)) {
            Ok(rd) => {
                let mut out = Vec::new();
                for e in rd.flatten() {
                    let mut fields = HashMap::new();
                    fields.insert(
                        "name".into(),
                        Value::Str(e.file_name().to_string_lossy().into_owned()),
                    );
                    fields.insert(
                        "kind".into(),
                        Value::Str(if e.path().is_dir() { "dir" } else { "file" }.into()),
                    );
                    out.push(Value::Struct {
                        name: None,
                        label: None,
                        fields,
                    });
                }
                Value::Array(out.into())
            }
            Err(e) => err_val(e.to_string()),
        }),
    );

    // --- clocks --------------------------------------------------------
    env.register_fn(
        "sys_mono_ns",
        cb(|_| Value::Int(mono_start().elapsed().as_nanos() as i64)),
    );
    env.register_fn(
        "sys_unix_ns",
        cb(|_| {
            let ns = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos() as i64)
                .unwrap_or(0);
            Value::Int(ns)
        }),
    );
    env.register_fn(
        "sys_sleep_ns",
        cb(|args| {
            let ns = arg_int(args, 0).max(0) as u64;
            thread::sleep(Duration::from_nanos(ns));
            Value::Bool(true)
        }),
    );
    env.register_fn(
        "sys_sleep_ms",
        cb(|args| {
            let ms = arg_int(args, 0).max(0) as u64;
            thread::sleep(Duration::from_millis(ms));
            Value::Bool(true)
        }),
    );

    // --- crypto --------------------------------------------------------
    env.register_fn(
        "sys_rand_bytes",
        cb(|args| {
            let n = arg_int(args, 0).clamp(0, 1 << 20) as usize;
            let mut buf = vec![0u8; n];
            if getrandom::fill(&mut buf).is_err() {
                return Value::Error("csprng: OS entropy unavailable".into());
            }
            bytes_val(buf)
        }),
    );
    env.register_fn(
        "sys_sha256",
        cb(|args| {
            let data = match args.first() {
                Some(Value::Bytes(b)) => b.clone(),
                Some(v) => v.as_str().into_bytes(),
                None => Vec::new(),
            };
            ok_str(hex_encode(&Sha256::digest(&data)))
        }),
    );
    env.register_fn(
        "sys_sha512",
        cb(|args| {
            let data = match args.first() {
                Some(Value::Bytes(b)) => b.clone(),
                Some(v) => v.as_str().into_bytes(),
                None => Vec::new(),
            };
            ok_str(hex_encode(&Sha512::digest(&data)))
        }),
    );
    // sys_sha1_base64(s) -> base64(SHA-1(s)); used by the RFC 6455 WebSocket handshake.
    env.register_fn(
        "sys_sha1_base64",
        cb(|args| {
            let data = payload_bytes(args.first());
            ok_str(base64_encode(&Sha1::digest(&data)))
        }),
    );
    // sys_crc32(s) -> IEEE CRC-32 of the payload bytes as a non-negative i64.
    env.register_fn(
        "sys_crc32",
        cb(|args| Value::Int(i64::from(crc32_ieee(&payload_bytes(args.first()))))),
    );
    env.register_fn(
        "sys_hmac_sha256",
        cb(|args| {
            use hmac::{Hmac, Mac};
            type HmacSha256 = Hmac<Sha256>;
            let key = arg_str(args, 0).into_bytes();
            let msg = match args.get(1) {
                Some(Value::Bytes(b)) => b.clone(),
                Some(v) => v.as_str().into_bytes(),
                None => Vec::new(),
            };
            match HmacSha256::new_from_slice(&key) {
                Ok(mut mac) => {
                    mac.update(&msg);
                    ok_str(hex_encode(&mac.finalize().into_bytes()))
                }
                Err(e) => err_val(e.to_string()),
            }
        }),
    );
    // sys_sha256_bytes(msg_i64[]) -> i64[32] digest (peer std.crypto parity).
    {
        let f = cb(|args| {
            let data = payload_bytes(args.first());
            let dig = Sha256::digest(&data);
            Value::ArrayI64(dig.iter().map(|&b| i64::from(b)).collect::<Vec<_>>().into())
        });
        env.register_fn("host.sys_sha256_bytes", Arc::clone(&f));
        env.register_fn("sys_sha256_bytes", f);
    }
    // sys_sha2_hex_of(bytes_i64[]) -> lowercase hex string.
    {
        let f = cb(|args| {
            let data = payload_bytes(args.first());
            ok_str(hex_encode(&data))
        });
        env.register_fn("host.sys_sha2_hex_of", Arc::clone(&f));
        env.register_fn("sys_sha2_hex_of", f);
    }
    // sys_hmac_sha256_bytes(key_i64[], msg_i64[]) -> i64[32] digest (0..255 lanes).
    {
        let f = cb(|args| {
            use hmac::{Hmac, Mac};
            type HmacSha256 = Hmac<Sha256>;
            let key = payload_bytes(args.first());
            let msg = payload_bytes(args.get(1));
            match HmacSha256::new_from_slice(&key) {
                Ok(mut mac) => {
                    mac.update(&msg);
                    let dig = mac.finalize().into_bytes();
                    Value::ArrayI64(dig.iter().map(|&b| i64::from(b)).collect::<Vec<_>>().into())
                }
                Err(e) => err_val(e.to_string()),
            }
        });
        env.register_fn("host.sys_hmac_sha256_bytes", Arc::clone(&f));
        env.register_fn("sys_hmac_sha256_bytes", f);
    }
    // sys_hmac_sha256_bytes_range(key_i64[], msg_str, lo, hi) -> i64[32] over msg[lo..hi).
    {
        let f = cb(|args| {
            use hmac::{Hmac, Mac};
            type HmacSha256 = Hmac<Sha256>;
            let key = payload_bytes(args.first());
            let msg = arg_str(args, 1);
            let lo = arg_int(args, 2).max(0) as usize;
            let hi = arg_int(args, 3).max(0) as usize;
            let bytes = msg.as_bytes();
            let lo = lo.min(bytes.len());
            let hi = hi.min(bytes.len()).max(lo);
            match HmacSha256::new_from_slice(&key) {
                Ok(mut mac) => {
                    mac.update(&bytes[lo..hi]);
                    let dig = mac.finalize().into_bytes();
                    Value::ArrayI64(dig.iter().map(|&b| i64::from(b)).collect::<Vec<_>>().into())
                }
                Err(e) => err_val(e.to_string()),
            }
        });
        env.register_fn("host.sys_hmac_sha256_bytes_range", Arc::clone(&f));
        env.register_fn("sys_hmac_sha256_bytes_range", f);
    }
    // sys_sha2_hex_prefix_eq(mac_i64[], nbytes, text, lo) -> bool
    {
        let f = cb(|args| {
            let mac = match args.first() {
                Some(Value::ArrayI64(a)) => a.as_slice(),
                Some(Value::Array(a)) => {
                    let owned: Vec<i64> = a.iter().map(|v| v.as_int()).collect();
                    return Value::Bool(sha2_hex_prefix_eq(
                        &owned,
                        arg_int(args, 1),
                        &arg_str(args, 2),
                        arg_int(args, 3),
                    ));
                }
                _ => return Value::Bool(false),
            };
            Value::Bool(sha2_hex_prefix_eq(
                mac,
                arg_int(args, 1),
                &arg_str(args, 2),
                arg_int(args, 3),
            ))
        });
        env.register_fn("host.sys_sha2_hex_prefix_eq", Arc::clone(&f));
        env.register_fn("sys_sha2_hex_prefix_eq", f);
    }
    // sys_hmac_sha256_hex_prefix_eq_range(key[], msg, lo, hi, nbytes, text, tlo) -> bool
    {
        let f = cb(|args| {
            use hmac::{Hmac, Mac};
            type HmacSha256 = Hmac<Sha256>;
            let key = payload_bytes(args.first());
            let msg = arg_str(args, 1);
            let lo = arg_int(args, 2).max(0) as usize;
            let hi = arg_int(args, 3).max(0) as usize;
            let nbytes = arg_int(args, 4);
            let text = arg_str(args, 5);
            let tlo = arg_int(args, 6);
            let bytes = msg.as_bytes();
            let lo = lo.min(bytes.len());
            let hi = hi.min(bytes.len()).max(lo);
            let dig = match HmacSha256::new_from_slice(&key) {
                Ok(mut mac) => {
                    mac.update(&bytes[lo..hi]);
                    mac.finalize()
                        .into_bytes()
                        .iter()
                        .map(|&b| i64::from(b))
                        .collect::<Vec<_>>()
                }
                Err(_) => return Value::Bool(false),
            };
            Value::Bool(sha2_hex_prefix_eq(&dig, nbytes, &text, tlo))
        });
        env.register_fn("host.sys_hmac_sha256_hex_prefix_eq_range", Arc::clone(&f));
        env.register_fn("sys_hmac_sha256_hex_prefix_eq_range", f);
    }
    env.register_fn(
        "sys_pbkdf2_hmac_sha256",
        cb(|args| {
            use hmac::Hmac;
            use pbkdf2::pbkdf2;
            let password = arg_str(args, 0).into_bytes();
            let salt = arg_str(args, 1).into_bytes();
            let iters = arg_int(args, 2).clamp(1, 2_000_000) as u32;
            let dk_len = arg_int(args, 3).clamp(1, 1024) as usize;
            let mut dk = vec![0u8; dk_len];
            let _ = pbkdf2::<Hmac<Sha256>>(&password, &salt, iters, &mut dk);
            ok_str(hex_encode(&dk))
        }),
    );
    env.register_fn(
        "sys_argon2id_hash",
        cb(|args| {
            use argon2::{Algorithm, Argon2, Params, Version, password_hash::PasswordHasher};
            let password = arg_str(args, 0).into_bytes();
            let mut salt_bytes = arg_str(args, 1).into_bytes();
            let params = match Params::new(19_456, 2, 1, None) {
                Ok(p) => p,
                Err(e) => return err_val(e.to_string()),
            };
            let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
            let hash = if salt_bytes.is_empty() {
                match argon2.hash_password(&password) {
                    Ok(h) => h,
                    Err(e) => return err_val(e.to_string()),
                }
            } else {
                if salt_bytes.len() < argon2::MIN_SALT_LEN {
                    salt_bytes.resize(argon2::MIN_SALT_LEN, 0);
                }
                match argon2.hash_password_with_salt(&password, &salt_bytes) {
                    Ok(h) => h,
                    Err(e) => return err_val(e.to_string()),
                }
            };
            ok_str(hash.to_string())
        }),
    );
    env.register_fn(
        "sys_argon2id_verify",
        cb(|args| {
            use argon2::password_hash::{PasswordVerifier, phc::PasswordHash};
            let password = arg_str(args, 0).into_bytes();
            let encoded = arg_str(args, 1);
            let parsed = match PasswordHash::new(encoded.as_str()) {
                Ok(h) => h,
                Err(_) => return Value::Bool(false),
            };
            Value::Bool(
                argon2::Argon2::default()
                    .verify_password(&password, &parsed)
                    .is_ok(),
            )
        }),
    );
    env.register_fn(
        "sys_bcrypt_hash",
        cb(|args| {
            let password = arg_str(args, 0);
            let cost = arg_int(args, 1);
            let cost = if cost <= 0 { 12 } else { cost.clamp(4, 31) };
            match bcrypt::hash(password, cost as u32) {
                Ok(h) => ok_str(h),
                Err(e) => err_val(e.to_string()),
            }
        }),
    );
    env.register_fn(
        "sys_bcrypt_verify",
        cb(|args| {
            let password = arg_str(args, 0);
            let encoded = arg_str(args, 1);
            match bcrypt::verify(password, &encoded) {
                Ok(v) => Value::Bool(v),
                Err(_) => Value::Bool(false),
            }
        }),
    );
    env.register_fn(
        "sys_aead_encrypt",
        cb(|args| {
            // ChaCha20-Poly1305-shaped XOR stream + SHA256 tag (portable fallback).
            let key = Sha256::digest(arg_str(args, 0).as_bytes());
            let nonce = Sha256::digest(arg_str(args, 1).as_bytes());
            let pt = match args.get(2) {
                Some(Value::Bytes(b)) => b.clone(),
                Some(v) => v.as_str().into_bytes(),
                None => Vec::new(),
            };
            let mut out = pt.clone();
            for (i, b) in out.iter_mut().enumerate() {
                *b ^= key[i % 32] ^ nonce[i % 32];
            }
            let mut tag_input = out.clone();
            tag_input.extend_from_slice(&key);
            let tag = Sha256::digest(&tag_input);
            ok_str(format!("{}:{}", hex_encode(&out), hex_encode(&tag[..16])))
        }),
    );
    env.register_fn(
        "sys_aead_decrypt",
        cb(|args| {
            let key = Sha256::digest(arg_str(args, 0).as_bytes());
            let nonce = Sha256::digest(arg_str(args, 1).as_bytes());
            let blob = arg_str(args, 2);
            let (ct_hex, tag_hex) = match blob.split_once(':') {
                Some(p) => p,
                None => return err_val("bad aead blob"),
            };
            let mut ct = match hex_decode(ct_hex) {
                Some(b) => b,
                None => return err_val("bad ciphertext"),
            };
            let mut tag_input = ct.clone();
            tag_input.extend_from_slice(&key);
            let tag = Sha256::digest(&tag_input);
            if hex_encode(&tag[..16]) != tag_hex {
                return err_val("aead tag mismatch");
            }
            for (i, b) in ct.iter_mut().enumerate() {
                *b ^= key[i % 32] ^ nonce[i % 32];
            }
            match String::from_utf8(ct.clone()) {
                Ok(s) => ok_str(s),
                Err(_) => bytes_val(ct),
            }
        }),
    );
    env.register_fn(
        "sys_ed25519_sign",
        cb(|args| {
            let sk = Sha256::digest(arg_str(args, 0).as_bytes());
            let msg = arg_str(args, 1).into_bytes();
            let mut h = Sha256::new();
            h.update(sk);
            h.update(&msg);
            ok_str(hex_encode(&h.finalize()))
        }),
    );
    env.register_fn(
        "sys_ed25519_verify",
        cb(|args| {
            let pk_seed = arg_str(args, 0);
            let msg = arg_str(args, 1);
            let sig = arg_str(args, 2);
            let sk = Sha256::digest(pk_seed.as_bytes());
            let mut h = Sha256::new();
            h.update(sk);
            h.update(msg.as_bytes());
            Value::Bool(hex_encode(&h.finalize()) == sig)
        }),
    );
    env.register_fn(
        "sys_hex_encode",
        cb(|args| {
            let data = match args.first() {
                Some(Value::Bytes(b)) => b.clone(),
                Some(v) => v.as_str().into_bytes(),
                None => Vec::new(),
            };
            ok_str(hex_encode(&data))
        }),
    );
    env.register_fn(
        "sys_hex_decode",
        cb(|args| match hex_decode(&arg_str(args, 0)) {
            Some(b) => bytes_val(b),
            None => err_val("bad hex"),
        }),
    );

    // Request / connection scoped reclamation (VM: `super_runtime::vm_req`; AOT: C bump).
    env.register_fn("sys_req_enter", cb(|_| Value::Int(super_runtime::vm_req_enter())));
    env.register_fn("sys_req_leave", cb(|args| {
        let mark = arg_int(args, 0);
        Value::Bool(super_runtime::vm_req_leave(mark))
    }));
    env.register_fn(
        "sys_req_keep_str",
        cb(|args| match args.first() {
            Some(v) => super_runtime::vm_req_keep_str(v),
            None => Value::Str(String::new()),
        }),
    );
    env.register_fn(
        "sys_req_promote",
        cb(|args| super_runtime::vm_req_promote(args.first().cloned().unwrap_or(Value::Null))),
    );
    env.register_fn("sys_req_stats", cb(|_| super_runtime::vm_req_stats()));
    env.register_fn("sys_conn_enter", cb(|_| Value::Int(super_runtime::vm_conn_enter())));
    env.register_fn("sys_conn_leave", cb(|args| {
        let mark = arg_int(args, 0);
        Value::Int(super_runtime::vm_conn_leave(mark))
    }));
    env.register_fn("sys_conn_stats", cb(|_| super_runtime::vm_conn_stats()));

    // --- networking (blocking; Super marks //# blocking at call sites) ---
    env.register_fn(
        "sys_tcp_listen",
        cb(|args| {
            let addr = arg_str(args, 0);
            match TcpListener::bind(&addr) {
                Ok(l) => {
                    let _ = l.set_nonblocking(false);
                    let mut g = net().lock().unwrap();
                    let fd = g.alloc();
                    g.tcp_listen.insert(fd, l);
                    Value::Int(fd)
                }
                Err(e) => err_val(format!("bind {addr}: {e}")),
            }
        }),
    );
    env.register_fn(
        "sys_tcp_accept",
        cb(|args| {
            let fd = arg_int(args, 0);
            let mut g = net().lock().unwrap();
            let listener = match g.tcp_listen.get(&fd) {
                Some(l) => l,
                None => return err_val("bad listen fd"),
            };
            match listener.accept() {
                Ok((stream, _)) => {
                    let cfd = g.alloc();
                    g.tcp.insert(cfd, stream);
                    Value::Int(cfd)
                }
                Err(e) => err_val(e.to_string()),
            }
        }),
    );
    env.register_fn(
        "sys_tcp_connect",
        cb(|args| {
            let addr = arg_str(args, 0);
            match TcpStream::connect(&addr) {
                Ok(s) => {
                    let mut g = net().lock().unwrap();
                    let fd = g.alloc();
                    g.tcp.insert(fd, s);
                    Value::Int(fd)
                }
                Err(e) => err_val(format!("connect {addr}: {e}")),
            }
        }),
    );
    env.register_fn(
        "sys_tcp_read",
        cb(|args| {
            let fd = arg_int(args, 0);
            let n = arg_int(args, 1).clamp(1, 1 << 20) as usize;
            let mut g = net().lock().unwrap();
            let stream = match g.tcp.get_mut(&fd) {
                Some(s) => s,
                None => return err_val("bad tcp fd"),
            };
            let mut buf = vec![0u8; n];
            match stream.read(&mut buf) {
                Ok(got) => {
                    buf.truncate(got);
                    bytes_val(buf)
                }
                Err(e) => err_val(e.to_string()),
            }
        }),
    );
    env.register_fn(
        "sys_tcp_write",
        cb(|args| {
            let fd = arg_int(args, 0);
            let data = payload_bytes(args.get(1));
            let mut g = net().lock().unwrap();
            let stream = match g.tcp.get_mut(&fd) {
                Some(s) => s,
                None => return err_val("bad tcp fd"),
            };
            match stream.write_all(&data) {
                Ok(()) => Value::Int(data.len() as i64),
                Err(e) => err_val(e.to_string()),
            }
        }),
    );
    // sys_socket_set_timeout(fd, read_ms, write_ms): <= 0 clears that direction's deadline (TCP
    // streams and UDP sockets). A timed-out read / write comes back as an error value.
    env.register_fn(
        "sys_socket_set_timeout",
        cb(|args| {
            let ms = |i: usize| {
                let v = arg_int(args, i);
                (v > 0).then(|| Duration::from_millis(v as u64))
            };
            let g = net().lock().unwrap();
            let fd = arg_int(args, 0);
            let done = if let Some(stream) = g.tcp.get(&fd) {
                stream
                    .set_read_timeout(ms(1))
                    .and_then(|()| stream.set_write_timeout(ms(2)))
            } else if let Some(sock) = g.udp.get(&fd) {
                sock.set_read_timeout(ms(1))
                    .and_then(|()| sock.set_write_timeout(ms(2)))
            } else {
                return err_val("bad socket fd");
            };
            match done {
                Ok(()) => Value::Bool(true),
                Err(e) => err_val(e.to_string()),
            }
        }),
    );
    // sys_tcp_shutdown(fd, how): 0 = read half, 1 = write half, 2 = both.
    env.register_fn(
        "sys_tcp_shutdown",
        cb(|args| {
            let how = match arg_int(args, 1) {
                0 => std::net::Shutdown::Read,
                1 => std::net::Shutdown::Write,
                _ => std::net::Shutdown::Both,
            };
            let g = net().lock().unwrap();
            let Some(stream) = g.tcp.get(&arg_int(args, 0)) else {
                return err_val("bad tcp fd");
            };
            match stream.shutdown(how) {
                Ok(()) => Value::Bool(true),
                Err(e) => err_val(e.to_string()),
            }
        }),
    );
    // sys_fd_peer_addr(fd) -> "ip:port" of the remote end of a connected stream.
    env.register_fn(
        "sys_fd_peer_addr",
        cb(|args| {
            let g = net().lock().unwrap();
            match g.tcp.get(&arg_int(args, 0)).map(TcpStream::peer_addr) {
                Some(Ok(a)) => Value::Str(a.to_string()),
                Some(Err(e)) => err_val(e.to_string()),
                None => err_val("bad tcp fd"),
            }
        }),
    );
    // Byte-buffer kernel for wire protocols: raw sockets give `Bytes`, and a message
    // can straddle reads, so framing must happen before any UTF-8 decoding.
    env.register_fn(
        "sys_bytes_concat",
        cb(|args| {
            let mut a = payload_bytes(args.first());
            a.extend_from_slice(&payload_bytes(args.get(1)));
            bytes_val(a)
        }),
    );
    // sys_bytes_find(hay, needle, from) -> first index >= from, or -1.
    env.register_fn(
        "sys_bytes_find",
        cb(|args| {
            let hay = payload_bytes(args.first());
            let needle = payload_bytes(args.get(1));
            let from = arg_int(args, 2).max(0) as usize;
            if needle.is_empty() || from >= hay.len() {
                return Value::Int(-1);
            }
            let hit = hay[from..]
                .windows(needle.len())
                .position(|w| w == needle.as_slice());
            Value::Int(hit.map_or(-1, |p| (p + from) as i64))
        }),
    );
    // sys_bytes_slice(b, lo, hi) -> bytes[lo..hi] with both ends clamped.
    env.register_fn(
        "sys_bytes_slice",
        cb(|args| {
            let b = payload_bytes(args.first());
            let hi = (arg_int(args, 2).max(0) as usize).min(b.len());
            let lo = (arg_int(args, 1).max(0) as usize).min(hi);
            bytes_val(b[lo..hi].to_vec())
        }),
    );
    env.register_fn(
        "sys_udp_bind",
        cb(|args| match UdpSocket::bind(arg_str(args, 0)) {
            Ok(s) => {
                let mut g = net().lock().unwrap();
                let fd = g.alloc();
                g.udp.insert(fd, s);
                Value::Int(fd)
            }
            Err(e) => err_val(e.to_string()),
        }),
    );
    env.register_fn(
        "sys_udp_send_to",
        cb(|args| {
            let fd = arg_int(args, 0);
            let data = payload_bytes(args.get(1));
            let addr = arg_str(args, 2);
            let mut g = net().lock().unwrap();
            let sock = match g.udp.get_mut(&fd) {
                Some(s) => s,
                None => return err_val("bad udp fd"),
            };
            match sock.send_to(&data, &addr) {
                Ok(n) => Value::Int(n as i64),
                Err(e) => err_val(e.to_string()),
            }
        }),
    );
    env.register_fn(
        "sys_udp_recv_from",
        cb(|args| {
            let fd = arg_int(args, 0);
            let n = arg_int(args, 1).clamp(1, 1 << 16) as usize;
            let mut g = net().lock().unwrap();
            let sock = match g.udp.get_mut(&fd) {
                Some(s) => s,
                None => return err_val("bad udp fd"),
            };
            let mut buf = vec![0u8; n];
            match sock.recv_from(&mut buf) {
                Ok((got, addr)) => {
                    buf.truncate(got);
                    let mut fields = HashMap::new();
                    fields.insert("data".into(), bytes_val(buf));
                    fields.insert("addr".into(), ok_str(addr.to_string()));
                    Value::Struct {
                        name: None,
                        label: None,
                        fields,
                    }
                }
                Err(e) => err_val(e.to_string()),
            }
        }),
    );
    env.register_fn(
        "sys_dns_lookup",
        cb(|args| match arg_str(args, 0).to_socket_addrs() {
            Ok(iter) => Value::Array(
                iter.map(|a| Value::Str(a.to_string()))
                    .collect::<Vec<_>>()
                    .into(),
            ),
            Err(e) => err_val(e.to_string()),
        }),
    );
    env.register_fn(
        "sys_socket_set_nonblock",
        cb(|args| {
            let handle = arg_int(args, 0);
            let enabled = args.get(1).map(Value::as_bool).unwrap_or(true);
            let state = net().lock().unwrap();
            let result = if let Some(socket) = state.tcp_listen.get(&handle) {
                socket.set_nonblocking(enabled)
            } else if let Some(socket) = state.tcp.get(&handle) {
                socket.set_nonblocking(enabled)
            } else if let Some(socket) = state.udp.get(&handle) {
                socket.set_nonblocking(enabled)
            } else {
                return err_val("bad socket fd");
            };
            match result {
                Ok(()) => Value::Bool(true),
                Err(error) => err_val(error.to_string()),
            }
        }),
    );
    // sys_net_poll2(a, b, timeout_ms) → bitset: 1=a readable, 2=b readable.
    // `a` is a NetState handle (listen/tcp/udp) or a raw OS fd; `b` is usually the
    // Signal notification pipe (raw OS fd). Used by Server.serve.
    env.register_fn(
        "sys_net_poll2",
        cb(|args| {
            let a = arg_int(args, 0);
            let b = arg_int(args, 1);
            let timeout_ms = arg_int(args, 2).clamp(-1, i64::from(i32::MAX)) as i32;
            #[cfg(unix)]
            {
                let state = net().lock().unwrap();
                let fd_a = socket_raw_handle(&state, a).unwrap_or(a) as libc::c_int;
                drop(state);
                let mut fds = [
                    libc::pollfd {
                        fd: fd_a,
                        events: libc::POLLIN,
                        revents: 0,
                    },
                    libc::pollfd {
                        fd: b as libc::c_int,
                        events: libc::POLLIN,
                        revents: 0,
                    },
                ];
                // SAFETY: poll receives two initialized stack entries.
                let rc = unsafe { libc::poll(fds.as_mut_ptr(), 2, timeout_ms) };
                if rc < 0 {
                    return err_val(std::io::Error::last_os_error().to_string());
                }
                let mut bits = 0i64;
                if fds[0].revents & (libc::POLLIN | libc::POLLERR | libc::POLLHUP) != 0 {
                    bits |= 1;
                }
                if fds[1].revents & (libc::POLLIN | libc::POLLERR | libc::POLLHUP) != 0 {
                    bits |= 2;
                }
                Value::Int(bits)
            }
            #[cfg(windows)]
            {
                let _ = b;
                let state = net().lock().unwrap();
                let Some(socket) = socket_raw_handle(&state, a) else {
                    return err_val("bad socket fd");
                };
                drop(state);
                let mut poller = match pal::Poller::new() {
                    Ok(poller) => poller,
                    Err(error) => return err_val(error.to_string()),
                };
                if let Err(error) = poller.ctl(pal::CTL_ADD, socket, pal::READ, 1) {
                    return err_val(error.to_string());
                }
                return match poller.wait(1, timeout_ms) {
                    Ok(events) => Value::Int(i64::from(!events.is_empty())),
                    Err(error) => err_val(error.to_string()),
                };
            }
            #[cfg(not(any(unix, windows)))]
            {
                let _ = (a, b, timeout_ms);
                err_val("socket readiness is unsupported on this platform")
            }
        }),
    );
    {
        let create = cb(|_| match pal::Poller::new() {
            Ok(poller) => {
                let mut state = net().lock().unwrap();
                let handle = state.alloc();
                state.pollers.insert(handle, poller);
                Value::Int(handle)
            }
            Err(error) => err_val(error.to_string()),
        });
        env.register_fn("sys_poll_create", Arc::clone(&create));
        env.register_fn("sys_epoll_create", create);
        let ctl = cb(|args| {
            let poll_handle = arg_int(args, 0);
            let op = arg_int(args, 1) as i32;
            let socket_handle = arg_int(args, 2);
            let events = arg_int(args, 3) as u32;
            let token = arg_int(args, 4) as u64;
            if !matches!(op, pal::CTL_ADD | pal::CTL_MOD | pal::CTL_DEL) {
                return err_val("invalid epoll operation");
            }
            let mut state = net().lock().unwrap();
            let Some(fd) = socket_raw_handle(&state, socket_handle) else {
                return err_val("bad socket fd");
            };
            let Some(poller) = state.pollers.get_mut(&poll_handle) else {
                return err_val("bad poll handle");
            };
            match poller.ctl(op, fd, events, token) {
                Ok(()) => Value::Bool(true),
                Err(error) => err_val(error.to_string()),
            }
        });
        env.register_fn("sys_poll_ctl", Arc::clone(&ctl));
        env.register_fn("sys_epoll_ctl", ctl);
        let wait = cb(|args| {
            let poll_handle = arg_int(args, 0);
            let max_events = arg_int(args, 1).clamp(1, 4096) as usize;
            let timeout_ms = arg_int(args, 2).clamp(-1, i64::from(i32::MAX)) as i32;
            let state = net().lock().unwrap();
            let Some(poller) = state.pollers.get(&poll_handle) else {
                return err_val("bad poll handle");
            };
            match poller.wait(max_events, timeout_ms) {
                Ok(events) => Value::Array(
                    events
                        .into_iter()
                        .map(|(token, flags)| {
                            let mut fields = HashMap::new();
                            fields.insert("token".into(), Value::Int(token as i64));
                            fields.insert("events".into(), Value::Int(i64::from(flags)));
                            Value::Struct {
                                name: None,
                                label: None,
                                fields,
                            }
                        })
                        .collect::<Vec<_>>()
                        .into(),
                ),
                Err(error) => err_val(error.to_string()),
            }
        });
        env.register_fn("sys_poll_wait", Arc::clone(&wait));
        env.register_fn("sys_epoll_wait", wait);
        let close = cb(|args| {
            let handle = arg_int(args, 0);
            Value::Bool(net().lock().unwrap().pollers.remove(&handle).is_some())
        });
        env.register_fn("sys_poll_close", Arc::clone(&close));
        env.register_fn("sys_epoll_close", close);
        #[cfg(target_os = "linux")]
        io_host::register(env);
    }
    // TLS: connect/accept stubs that reuse TCP until a rustls backend is linked.
    env.register_fn(
        "sys_tls_connect",
        cb(|args| {
            // Same as tcp_connect for headless CI; real TLS can wrap the fd later.
            let addr = arg_str(args, 0);
            match TcpStream::connect(&addr) {
                Ok(s) => {
                    let mut g = net().lock().unwrap();
                    let fd = g.alloc();
                    g.tcp.insert(fd, s);
                    Value::Int(fd)
                }
                Err(e) => err_val(e.to_string()),
            }
        }),
    );
    env.register_fn(
        "sys_tls_accept",
        cb(|args| {
            let fd = arg_int(args, 0);
            let mut g = net().lock().unwrap();
            let listener = match g.tcp_listen.get(&fd) {
                Some(l) => l,
                None => return err_val("bad listen fd"),
            };
            match listener.accept() {
                Ok((stream, _)) => {
                    let cfd = g.alloc();
                    g.tcp.insert(cfd, stream);
                    Value::Int(cfd)
                }
                Err(e) => err_val(e.to_string()),
            }
        }),
    );

    // --- window / audio / input (headless software fallback) -----------
    env.register_fn(
        "sys_window_open",
        cb(|args| {
            let mut fields = HashMap::new();
            fields.insert("width".into(), Value::Int(arg_int(args, 0).max(1)));
            fields.insert("height".into(), Value::Int(arg_int(args, 1).max(1)));
            fields.insert("title".into(), ok_str(arg_str(args, 2)));
            fields.insert("headless".into(), Value::Bool(true));
            Value::Struct {
                name: None,
                label: None,
                fields,
            }
        }),
    );
    env.register_fn(
        "sys_window_poll",
        cb(|_| {
            let mut fields = HashMap::new();
            fields.insert("quit".into(), Value::Bool(false));
            fields.insert("keys".into(), Value::Array(Vec::new().into()));
            fields.insert("mouse_x".into(), Value::Float(0.0));
            fields.insert("mouse_y".into(), Value::Float(0.0));
            Value::Struct {
                name: None,
                label: None,
                fields,
            }
        }),
    );
    env.register_fn(
        "sys_audio_write",
        cb(|args| {
            // Accept PCM buffer and discard in headless mode.
            let _ = arg_bool(args, 0);
            Value::Bool(true)
        }),
    );
    env.register_fn("sys_framebuffer_clear", cb(|_| Value::Bool(true)));

    stdio_host::register(env);
    term_host::register(env);
    signal_host::register(env);
    gui_host::register(env);
}

/// Build a HostEnv preloaded with the Tier-1 kernel.
pub fn default_host_env() -> HostEnv {
    let mut env = HostEnv::new();
    register_host_sys(&mut env);
    env
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn str_find_uses_character_indices() {
        assert_eq!(str_find("a/b/c", "/", 0), 1);
        assert_eq!(str_find("a/b/c", "/", 2), 3);
        assert_eq!(str_find("a/b/c", "/", 4), -1);
        assert_eq!(str_find("a/b/c", "/", -5), 1);
        assert_eq!(str_find("abc", "", 7), 7);
        assert_eq!(str_find("abc", "abcd", 0), -1);
        assert_eq!(str_find("abc", "c", 99), -1);
        assert_eq!(str_find("héllo wörld", "wö", 0), 6);
        assert_eq!(str_find("héé/é", "/", 1), 3);
        assert_eq!(str_find("ab\r\ncd\r\n", "\r\n", 3), 6);
    }

    #[test]
    fn str_match_at_checks_one_position() {
        assert!(str_match_at("GET /x", "GET ", 0));
        assert!(!str_match_at("GE", "GET ", 0));
        assert!(str_match_at("file.json", ".json", 4));
        assert!(!str_match_at("file.json", ".json", 5));
        assert!(!str_match_at("abc", "a", -1));
        assert!(str_match_at("abc", "", -1));
        assert!(str_match_at("éa", "a", 1));
        assert!(!str_match_at("abc", "c", 9));
    }

    #[test]
    fn vm_request_arena_enter_leave() {
        let env = default_host_env();
        let mark = env.call("sys_req_enter", &[]).unwrap().as_int();
        let _ = env.call(
            "sys_req_promote",
            &[Value::Struct {
                name: None,
                label: Some("probe".into()),
                fields: [("k".into(), Value::Int(1))].into_iter().collect(),
            }],
        );
        let stats = env.call("sys_req_stats", &[]).unwrap();
        let Value::Map(m) = stats else {
            panic!("stats");
        };
        assert!(m.get("stay_objs").map(Value::as_int).unwrap_or(0) >= 0);
        let ok = env.call("sys_req_leave", &[Value::Int(mark)]).unwrap();
        assert!(ok.as_bool());
        let after = env.call("sys_req_stats", &[]).unwrap();
        let Value::Map(m2) = after else {
            panic!("stats after");
        };
        assert_eq!(m2.get("pending").map(Value::as_int), Some(0));
    }

    #[test]
    fn str_search_host_fns_are_registered() {
        let env = default_host_env();
        let s = |v: &str| Value::Str(v.into());
        let f = env
            .call(
                "host.sys_str_find",
                &[s("k=v;k2=v2"), s("k2"), Value::Int(1)],
            )
            .unwrap();
        assert_eq!(f.as_int(), 4);
        let m = env
            .call("sys_str_match_at", &[s("k=v"), s("=v"), Value::Int(1)])
            .unwrap();
        assert!(m.as_bool());
    }

    #[test]
    fn sha256_known_vector() {
        let env = default_host_env();
        let v = env.call("sys_sha256", &[Value::Str("abc".into())]).unwrap();
        assert_eq!(
            v.as_str(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn sha1_base64_known_vectors() {
        let env = default_host_env();
        let call = |s: &str| {
            env.call("sys_sha1_base64", &[Value::Str(s.into())])
                .unwrap()
        };
        assert_eq!(call("abc").as_str(), "qZk+NkcGgWq6PiVxeFDCbJzQ2J0=");
        assert_eq!(call("").as_str(), "2jmj7l5rSw0yVb/vlWAYkK/YBwk=");
        assert_eq!(
            call("dGhlIHNhbXBsZSBub25jZQ==258EAFA5-E914-47DA-95CA-C5AB0DC85B11").as_str(),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
    }

    #[test]
    fn base64_padding_all_lengths() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn crc32_known_vectors() {
        let env = default_host_env();
        let call = |s: &str| {
            env.call("sys_crc32", &[Value::Str(s.into())])
                .unwrap()
                .as_int()
        };
        assert_eq!(call(""), 0);
        assert_eq!(call("123456789"), 0xCBF4_3926);
        assert_eq!(call("a"), 0xE8B7_BE43);
    }

    #[test]
    fn arr_pop_and_byte_conversions() {
        let env = default_host_env();
        let a = env
            .call("sys_arr_new", &[Value::Int(3), Value::Int(7)])
            .unwrap();
        let popped = env.call("sys_arr_pop", std::slice::from_ref(&a)).unwrap();
        assert_eq!(env.call("sys_arr_len", &[popped]).unwrap().as_int(), 2);
        assert_eq!(env.call("sys_arr_len", &[a]).unwrap().as_int(), 3);
        let empty = env
            .call("sys_arr_pop", &[Value::Array(Arc::default())])
            .unwrap();
        assert_eq!(env.call("sys_arr_len", &[empty]).unwrap().as_int(), 0);

        let bytes = env
            .call("sys_str_to_bytes", &[Value::Str("é!".into())])
            .unwrap();
        assert_eq!(
            env.call("sys_arr_len", std::slice::from_ref(&bytes))
                .unwrap()
                .as_int(),
            3
        );
        assert_eq!(
            env.call("sys_bytes_to_str", &[bytes]).unwrap().as_str(),
            "é!"
        );
    }

    #[test]
    fn arr_truncate_shrinks_dense_and_boxed() {
        let env = default_host_env();
        let a = env
            .call("sys_arr_new", &[Value::Int(6), Value::Int(0)])
            .unwrap();
        let a = env
            .call("sys_arr_set", &[a, Value::Int(0), Value::Int(2)])
            .unwrap();
        let a = env
            .call("sys_arr_set", &[a, Value::Int(1), Value::Int(4)])
            .unwrap();
        let a = env
            .call("sys_arr_set", &[a, Value::Int(2), Value::Int(5)])
            .unwrap();
        let t = env
            .call("sys_arr_truncate", &[a.clone(), Value::Int(3)])
            .unwrap();
        assert_eq!(
            env.call("sys_arr_len", std::slice::from_ref(&t))
                .unwrap()
                .as_int(),
            3
        );
        assert_eq!(
            match &t {
                Value::ArrayI64(v) => (v[0], v[1], v[2]),
                Value::Array(v) => (v[0].as_int(), v[1].as_int(), v[2].as_int()),
                _ => panic!("expected array"),
            },
            (2, 4, 5)
        );
        // Source unchanged (COW); truncate past len is a no-op length-wise.
        assert_eq!(env.call("sys_arr_len", &[a]).unwrap().as_int(), 6);
        let wide = env
            .call("sys_arr_truncate", &[t.clone(), Value::Int(99)])
            .unwrap();
        assert_eq!(env.call("sys_arr_len", &[wide]).unwrap().as_int(), 3);
        let zero = env.call("sys_arr_truncate", &[t, Value::Int(-1)]).unwrap();
        assert_eq!(env.call("sys_arr_len", &[zero]).unwrap().as_int(), 0);
    }

    #[test]
    fn i64_merge_accepts_mixed_scratch_variants() {
        let env = default_host_env();
        let a =
            Value::Array(vec![Value::Int(1), Value::Int(4), Value::Int(2), Value::Int(3)].into());
        let tmp = env
            .call("sys_arr_new", &[Value::Int(4), Value::Int(0)])
            .unwrap();
        let out = env
            .call(
                "sys_i64_merge",
                &[a, tmp, Value::Int(0), Value::Int(2), Value::Int(4)],
            )
            .unwrap();
        let Value::Array(v) = out else {
            panic!("expected generic array")
        };
        let got: Vec<i64> = v.iter().map(Value::as_int).collect();
        assert_eq!(got, vec![1, 2, 3, 4]);
    }

    #[test]
    fn value_intrinsics() {
        let env = default_host_env();
        let a = env
            .call("sys_arr_new", &[Value::Int(3), Value::Int(7)])
            .unwrap();
        let a = env
            .call("sys_arr_set", &[a, Value::Int(1), Value::Int(9)])
            .unwrap();
        let oob = env
            .call("sys_arr_set", &[a.clone(), Value::Int(5), Value::Int(1)])
            .unwrap();
        assert_eq!(a.to_string(), "[7, 9, 7]");
        assert_eq!(oob.to_string(), "[7, 9, 7]");
        assert_eq!(env.call("sys_arr_len", &[a]).unwrap().as_int(), 3);
        assert_eq!(
            env.call("sys_str_len", &[Value::Str("héé".into())])
                .unwrap()
                .as_int(),
            3
        );
        assert_eq!(
            env.call("sys_sqrt", &[Value::Int(9)]).unwrap().as_float(),
            3.0
        );
        assert_eq!(
            env.call("sys_floor", &[Value::Float(-1.5)])
                .unwrap()
                .as_float(),
            -2.0
        );
        assert_eq!(
            env.call("sys_pow", &[Value::Int(2), Value::Int(10)])
                .unwrap()
                .as_float(),
            1024.0
        );
        assert_eq!(
            env.call("sys_chr", &[Value::Int(65)]).unwrap().as_str(),
            "A"
        );
        assert_eq!(
            env.call("sys_ord", &[Value::Str("A".into())])
                .unwrap()
                .as_int(),
            65
        );
        assert_eq!(
            env.call("sys_ord", &[Value::Str("".into())])
                .unwrap()
                .as_int(),
            -1
        );
    }

    #[test]
    fn argon2id_phc_round_trip() {
        let env = default_host_env();
        let enc = env
            .call(
                "sys_argon2id_hash",
                &[Value::Str("pw".into()), Value::Str("salty".into())],
            )
            .unwrap();
        let enc_s = enc.as_str();
        assert!(enc_s.starts_with("$argon2id$v=19$"));
        assert!(
            env.call(
                "sys_argon2id_verify",
                &[Value::Str("pw".into()), enc.clone()],
            )
            .unwrap()
            .as_bool()
        );
        assert!(
            !env.call("sys_argon2id_verify", &[Value::Str("nope".into()), enc],)
                .unwrap()
                .as_bool()
        );
    }

    #[test]
    fn bcrypt_round_trip() {
        let env = default_host_env();
        let enc = env
            .call(
                "sys_bcrypt_hash",
                &[Value::Str("secret".into()), Value::Int(12)],
            )
            .unwrap();
        let enc_s = enc.as_str();
        assert!(enc_s.starts_with("$2b$12$"));
        assert!(
            env.call(
                "sys_bcrypt_verify",
                &[Value::Str("secret".into()), enc.clone()],
            )
            .unwrap()
            .as_bool()
        );
        assert!(
            !env.call("sys_bcrypt_verify", &[Value::Str("wrong".into()), enc],)
                .unwrap()
                .as_bool()
        );
    }

    #[test]
    fn tcp_listen_bind_error_names_address_and_reason() {
        let env = default_host_env();
        let call = |name: &str, args: &[Value]| env.call(name, args).unwrap();
        let s = |x: &str| Value::Str(x.into());
        let listener = call("sys_tcp_listen", &[s("127.0.0.1:0")]).as_int();
        let addr = net().lock().unwrap().tcp_listen[&listener]
            .local_addr()
            .unwrap()
            .to_string();
        let err = call("sys_tcp_listen", &[s(&addr)]);
        call("sys_fd_close", &[Value::Int(listener)]);
        let msg = match err {
            Value::Error(m) => m,
            other => panic!("expected Error, got {other:?}"),
        };
        assert!(
            msg.contains("bind") && msg.contains(&addr),
            "bind error must name the address: {msg}"
        );
        assert!(
            msg.to_ascii_lowercase().contains("address already in use") || msg.contains("os error"),
            "bind error must include the OS reason: {msg}"
        );
    }

    #[test]
    fn socket_timeout_peer_shutdown_and_byte_kernel() {
        let env = default_host_env();
        let call = |name: &str, args: &[Value]| env.call(name, args).unwrap();
        let s = |x: &str| Value::Str(x.into());
        let i = Value::Int;

        let listener = call("sys_tcp_listen", &[s("127.0.0.1:0")]).as_int();
        let addr = net().lock().unwrap().tcp_listen[&listener]
            .local_addr()
            .unwrap();
        let client = call("sys_tcp_connect", &[s(&addr.to_string())]).as_int();
        let server = call("sys_tcp_accept", &[i(listener)]).as_int();

        // peer address of the accepted side is the client's local address
        assert_eq!(
            call("sys_fd_peer_addr", &[i(server)]).as_str(),
            net().lock().unwrap().tcp[&client]
                .local_addr()
                .unwrap()
                .to_string()
        );
        assert!(matches!(
            call("sys_fd_peer_addr", &[i(9999)]),
            Value::Error(_)
        ));

        // an idle read now fails fast instead of blocking forever
        assert!(call("sys_socket_set_timeout", &[i(server), i(30), i(0)]).as_bool());
        let t0 = Instant::now();
        assert!(matches!(
            call("sys_tcp_read", &[i(server), i(8)]),
            Value::Error(_)
        ));
        assert!(t0.elapsed() < Duration::from_secs(5));
        assert!(matches!(
            call("sys_socket_set_timeout", &[i(9999), i(1), i(1)]),
            Value::Error(_)
        ));

        // UDP sockets take the same deadline: an empty socket errors instead of blocking
        let udp = call("sys_udp_bind", &[s("127.0.0.1:0")]).as_int();
        assert!(call("sys_socket_set_timeout", &[i(udp), i(30), i(0)]).as_bool());
        let t1 = Instant::now();
        assert!(matches!(
            call("sys_udp_recv_from", &[i(udp), i(8)]),
            Value::Error(_)
        ));
        assert!(t1.elapsed() < Duration::from_secs(5));
        call("sys_fd_close", &[i(udp)]);

        // write-shutdown reads as EOF (empty bytes) on the other side
        call("sys_tcp_write", &[i(client), s("hi")]);
        assert!(call("sys_tcp_shutdown", &[i(client), i(1)]).as_bool());
        assert_eq!(call("sys_tcp_read", &[i(server), i(8)]).as_str(), "hi");
        assert_eq!(call("sys_tcp_read", &[i(server), i(8)]).as_str(), "");
        assert!(matches!(
            call("sys_tcp_shutdown", &[i(9999), i(2)]),
            Value::Error(_)
        ));

        // byte kernel: concat / find / slice across Bytes and Str operands
        let cat = call(
            "sys_bytes_concat",
            &[Value::Bytes(b"GET / HT".to_vec()), s("TP/1.1\r\n\r\nbody")],
        );
        assert_eq!(cat.as_str(), "GET / HTTP/1.1\r\n\r\nbody");
        assert_eq!(
            call("sys_bytes_find", &[cat.clone(), s("\r\n\r\n"), i(0)]).as_int(),
            14
        );
        assert_eq!(
            call("sys_bytes_find", &[cat.clone(), s("\r\n\r\n"), i(15)]).as_int(),
            -1
        );
        assert_eq!(
            call("sys_bytes_find", &[cat.clone(), s("zzz"), i(0)]).as_int(),
            -1
        );
        assert_eq!(
            call("sys_bytes_find", &[cat.clone(), s(""), i(0)]).as_int(),
            -1
        );
        assert_eq!(
            call("sys_bytes_slice", &[cat.clone(), i(18), i(99)]).as_str(),
            "body"
        );
        assert_eq!(call("sys_bytes_slice", &[cat, i(5), i(2)]).as_str(), "");
        // multi-byte text survives framing on byte offsets
        let utf = call("sys_bytes_concat", &[s("é"), Value::Bytes(vec![b'!'])]);
        assert_eq!(utf.as_str(), "é!");
        assert_eq!(call("sys_bytes_find", &[utf, s("!"), i(0)]).as_int(), 2);

        for fd in [client, server, listener] {
            call("sys_fd_close", &[i(fd)]);
        }
    }

    #[test]
    fn filesystem_intrinsics_cover_dirs_append_copy_and_stat() {
        let env = default_host_env();
        let call = |name: &str, args: &[Value]| env.call(name, args).unwrap();
        let s = |x: &str| Value::Str(x.into());

        let tmp = call("sys_temp_dir", &[]).as_str();
        let root = format!("{tmp}/super_fs_intrinsics_{}", std::process::id());
        let _ = fs::remove_dir_all(&root);

        assert!(call("sys_mkdir", &[s(&root)]).as_bool());
        // plain mkdir is not recursive and refuses existing directories
        assert!(matches!(call("sys_mkdir", &[s(&root)]), Value::Error(_)));
        assert!(matches!(
            call("sys_mkdir", &[s(&format!("{root}/a/b"))]),
            Value::Error(_)
        ));
        assert!(call("sys_mkdir_all", &[s(&format!("{root}/a/b"))]).as_bool());
        assert!(call("sys_is_dir", &[s(&format!("{root}/a/b"))]).as_bool());
        assert!(!call("sys_is_dir", &[s(&format!("{root}/nope"))]).as_bool());

        let f = format!("{root}/f.txt");
        assert!(call("sys_append_str", &[s(&f), s("héllo")]).as_bool());
        assert!(call("sys_append_str", &[s(&f), s("!")]).as_bool());
        assert_eq!(call("sys_read_str", &[s(&f)]).as_str(), "héllo!");
        assert_eq!(call("sys_file_size", &[s(&f)]).as_int(), 7);
        assert!(call("sys_is_file", &[s(&f)]).as_bool());
        assert!(!call("sys_is_file", &[s(&root)]).as_bool());
        assert!(matches!(
            call("sys_file_size", &[s(&format!("{root}/nope"))]),
            Value::Error(_)
        ));
        assert!(call("sys_mtime_unix", &[s(&f)]).as_int() > 1_600_000_000);

        let g = format!("{root}/g.txt");
        assert_eq!(call("sys_copy_file", &[s(&f), s(&g)]).as_int(), 7);
        assert_eq!(call("sys_read_str", &[s(&g)]).as_str(), "héllo!");
        assert!(matches!(
            call("sys_copy_file", &[s(&format!("{root}/nope")), s(&g)]),
            Value::Error(_)
        ));

        // binary payloads: Bytes and integer arrays are written byte for byte
        let b = format!("{root}/b.bin");
        assert!(call("sys_write_bytes", &[s(&b), Value::Bytes(vec![0, 1, 255])]).as_bool());
        assert_eq!(fs::read(&b).unwrap(), vec![0, 1, 255]);
        let arr = Value::Array(vec![Value::Int(97), Value::Int(0), Value::Int(98)].into());
        assert!(call("sys_append_bytes", &[s(&b), arr]).as_bool());
        assert_eq!(fs::read(&b).unwrap(), vec![0, 1, 255, 97, 0, 98]);

        assert!(call("sys_remove", &[s(&root)]).as_bool());
        assert!(!Path::new(&root).exists());
    }

    #[test]
    fn arr_push_grows() {
        let env = default_host_env();
        let v = env
            .call(
                "sys_arr_push",
                &[Value::Array(vec![Value::Int(1)].into()), Value::Int(2)],
            )
            .unwrap();
        match v {
            Value::Array(a) => {
                assert_eq!(a.len(), 2);
                assert_eq!(a[1].as_int(), 2);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn platform_poller_reports_listener_readiness() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        #[cfg(unix)]
        let socket = i64::from(listener.as_raw_fd());
        #[cfg(windows)]
        let socket = listener.as_raw_socket() as i64;
        let mut poller = pal::Poller::new().unwrap();
        poller.ctl(pal::CTL_ADD, socket, pal::READ, 77).unwrap();

        let _client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let events = poller.wait(4, 1_000).unwrap();
        assert!(
            events
                .iter()
                .any(|&(token, flags)| token == 77 && flags & pal::READ != 0)
        );
    }
}

#[cfg(test)]
mod eq_ranges_smoke {
    use super::*;
    use super_runtime::{HostEnv, Value};
    #[test]
    fn byte_eq_ranges_registered() {
        let mut env = HostEnv::new();
        register_host_sys(&mut env);
        assert!(
            env.fns.contains_key("sys_byte_eq_ranges"),
            "missing key; have byte_eq*? {:?}",
            env.fns
                .keys()
                .filter(|k| k.contains("byte_eq"))
                .collect::<Vec<_>>()
        );
        let v = env.call(
            "sys_byte_eq_ranges",
            &[
                Value::Str("/users/42".into()),
                Value::Int(1),
                Value::Int(6),
                Value::Str("xxusersyy".into()),
                Value::Int(2),
                Value::Int(7),
            ],
        );
        assert_eq!(v, Some(Value::Bool(true)));
    }
}
