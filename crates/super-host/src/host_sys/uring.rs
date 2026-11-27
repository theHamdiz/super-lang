//! Real Linux `io_uring` engine for `std.io.uring`, with an epoll-backed fallback that
//! exposes the identical proactor-style API.
//!
//! * [`Native`] talks to the kernel through the `io_uring_setup` / `io_uring_enter` /
//!   `io_uring_register` system calls (via `libc::syscall`), `mmap`s the SQ/CQ rings and
//!   the SQE array, and drives them with the documented acquire/release protocol.
//!   Supports batched submission, optional `IORING_SETUP_SQPOLL`, fixed (registered)
//!   buffers and fixed (registered) files.
//! * [`Emulated`] is used on `ENOSYS` / `EPERM` / `EACCES` / `EINVAL` from setup (old
//!   kernels, seccomp sandboxes, `io_uring_disabled=2`): queued operations are executed
//!   when their descriptor becomes ready, using `poll(2)` for the readiness check and an
//!   `epoll` instance to sleep until something can make progress.
//!
//! Every operation owns its buffers for as long as it is in flight, so the kernel never
//! writes into memory systems peer has freed. All `unsafe` in this file is confined to the
//! syscall wrappers, the [`Mapping`] RAII type and a handful of libc calls, each with a
//! `SAFETY` note.

use std::collections::{HashMap, VecDeque};
use std::io;
use std::mem::{align_of, size_of};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::sync::atomic::{AtomicU32, Ordering, fence};
use std::time::{Duration, Instant};

/// Kernel ABI constants (stable since the respective kernel version noted).
mod abi {
    pub const OFF_SQ_RING: i64 = 0;
    pub const OFF_CQ_RING: i64 = 0x0800_0000;
    pub const OFF_SQES: i64 = 0x1000_0000;

    pub const SETUP_SQPOLL: u32 = 1 << 1;

    pub const FEAT_SINGLE_MMAP: u32 = 1 << 0;
    pub const FEAT_EXT_ARG: u32 = 1 << 8;

    pub const ENTER_GETEVENTS: u32 = 1 << 0;
    pub const ENTER_SQ_WAKEUP: u32 = 1 << 1;
    pub const ENTER_SQ_WAIT: u32 = 1 << 2;
    pub const ENTER_EXT_ARG: u32 = 1 << 3;

    pub const SQ_NEED_WAKEUP: u32 = 1 << 0;
    pub const SQE_FIXED_FILE: u8 = 1 << 0;

    pub const REGISTER_BUFFERS: u32 = 0;
    pub const UNREGISTER_BUFFERS: u32 = 1;
    pub const REGISTER_FILES: u32 = 2;
    pub const UNREGISTER_FILES: u32 = 3;

    pub const OP_NOP: u8 = 0;
    pub const OP_READV: u8 = 1;
    pub const OP_WRITEV: u8 = 2;
    pub const OP_FSYNC: u8 = 3;
    pub const OP_READ_FIXED: u8 = 4;
    pub const OP_WRITE_FIXED: u8 = 5;
    pub const OP_POLL_ADD: u8 = 6;
    pub const OP_TIMEOUT: u8 = 11;
    pub const OP_ACCEPT: u8 = 13;
    pub const OP_READ: u8 = 22;
    pub const OP_WRITE: u8 = 23;
    pub const OP_SEND: u8 = 26;
    pub const OP_RECV: u8 = 27;
}

/// `-ETIME`: result of a `Timeout` operation whose timer fired.
pub const ETIME_RESULT: i64 = -(libc::ETIME as i64);
/// Offset sentinel meaning "use (and advance) the file's current position".
pub const CUR_POS: u64 = u64::MAX;
const MAX_IOV: usize = 1024;
const MAX_ENTRIES: u32 = 4096;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct SqRingOffsets {
    head: u32,
    tail: u32,
    ring_mask: u32,
    ring_entries: u32,
    flags: u32,
    dropped: u32,
    array: u32,
    resv1: u32,
    user_addr: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct CqRingOffsets {
    head: u32,
    tail: u32,
    ring_mask: u32,
    ring_entries: u32,
    overflow: u32,
    cqes: u32,
    flags: u32,
    resv1: u32,
    user_addr: u64,
}

/// `struct io_uring_params`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Params {
    sq_entries: u32,
    cq_entries: u32,
    flags: u32,
    sq_thread_cpu: u32,
    sq_thread_idle: u32,
    features: u32,
    wq_fd: u32,
    resv: [u32; 3],
    sq_off: SqRingOffsets,
    cq_off: CqRingOffsets,
}

/// `struct io_uring_sqe` (64 bytes).
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Sqe {
    opcode: u8,
    flags: u8,
    ioprio: u16,
    fd: i32,
    off: u64,
    addr: u64,
    len: u32,
    op_flags: u32,
    user_data: u64,
    buf_index: u16,
    personality: u16,
    splice_fd_in: i32,
    addr3: u64,
    pad: u64,
}

/// `struct io_uring_cqe` (16 bytes).
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Cqe {
    user_data: u64,
    res: i32,
    flags: u32,
}

/// `struct io_uring_getevents_arg`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct GetEventsArg {
    sigmask: u64,
    sigmask_sz: u32,
    pad: u32,
    ts: u64,
}

/// `struct __kernel_timespec`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Timespec {
    sec: i64,
    nsec: i64,
}

/// Layout twin of `struct iovec` that stays `Send` (addresses as integers).
#[repr(C)]
#[derive(Clone, Copy)]
struct IoVec {
    base: usize,
    len: usize,
}

const _: () = {
    assert!(size_of::<Sqe>() == 64);
    assert!(size_of::<Cqe>() == 16);
    assert!(size_of::<Params>() == 120);
    assert!(size_of::<GetEventsArg>() == 24);
    assert!(size_of::<IoVec>() == size_of::<libc::iovec>());
};

fn errno() -> i32 {
    io::Error::last_os_error()
        .raw_os_error()
        .unwrap_or(libc::EIO)
}

fn os_err(code: i32) -> io::Error {
    io::Error::from_raw_os_error(code)
}

fn to_timespec(d: Duration) -> Timespec {
    Timespec {
        sec: i64::try_from(d.as_secs()).unwrap_or(i64::MAX),
        nsec: i64::from(d.subsec_nanos()),
    }
}

// ---------------------------------------------------------------------------
// Raw system calls
// ---------------------------------------------------------------------------

/// `io_uring_setup(2)`. Returns the ring descriptor.
fn io_uring_setup(entries: u32, params: &mut Params) -> io::Result<OwnedFd> {
    // SAFETY: `params` is a valid, exclusively borrowed `io_uring_params`; the kernel
    // reads and writes only that struct.
    let ret = unsafe {
        libc::syscall(
            libc::SYS_io_uring_setup,
            libc::c_long::from(entries),
            std::ptr::from_mut(params),
        )
    };
    if ret < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a non-negative return is a freshly created descriptor nobody else owns.
    Ok(unsafe { OwnedFd::from_raw_fd(ret as RawFd) })
}

/// `io_uring_enter2(2)`. `arg`/`argsz` carry either a sigset (unused) or an extended arg.
fn io_uring_enter(
    ring: RawFd,
    to_submit: u32,
    min_complete: u32,
    flags: u32,
    arg: *const libc::c_void,
    argsz: usize,
) -> io::Result<u32> {
    // SAFETY: callers pass either null or a pointer to a live `GetEventsArg` of size
    // `argsz`; the kernel only reads it during the call.
    let ret = unsafe {
        libc::syscall(
            libc::SYS_io_uring_enter,
            libc::c_long::from(ring),
            libc::c_long::from(to_submit),
            libc::c_long::from(min_complete),
            libc::c_long::from(flags),
            arg,
            argsz as libc::c_long,
        )
    };
    if ret < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(ret as u32)
    }
}

/// `io_uring_register(2)`.
fn io_uring_register(
    ring: RawFd,
    opcode: u32,
    arg: *const libc::c_void,
    nr_args: u32,
) -> io::Result<()> {
    // SAFETY: callers pass null or a pointer to `nr_args` valid elements of the type the
    // opcode expects; the kernel copies it during the call.
    let ret = unsafe {
        libc::syscall(
            libc::SYS_io_uring_register,
            libc::c_long::from(ring),
            libc::c_long::from(opcode),
            arg,
            libc::c_long::from(nr_args),
        )
    };
    if ret < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// mmap RAII
// ---------------------------------------------------------------------------

/// A shared mapping of ring memory. All accessors bounds- and alignment-check.
struct Mapping {
    ptr: *mut u8,
    len: usize,
}

// SAFETY: the mapping is exclusively owned by one `Native` ring and only touched through
// `&self` accessors that use atomics / volatile accesses; it carries no thread affinity.
unsafe impl Send for Mapping {}

impl Mapping {
    fn map(fd: RawFd, len: usize, offset: i64) -> io::Result<Self> {
        // SAFETY: anonymous placement (null hint), kernel-validated fd and offset; the
        // result is checked against MAP_FAILED before use.
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED | libc::MAP_POPULATE,
                fd,
                offset,
            )
        };
        if ptr == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            ptr: ptr.cast(),
            len,
        })
    }

    fn check<T>(&self, off: usize) {
        assert!(
            off.checked_add(size_of::<T>())
                .is_some_and(|e| e <= self.len)
                && off.is_multiple_of(align_of::<T>()),
            "io_uring mapping access out of bounds"
        );
    }

    fn atomic(&self, off: u32) -> &AtomicU32 {
        let off = off as usize;
        self.check::<AtomicU32>(off);
        // SAFETY: in bounds and 4-aligned (checked); the kernel shares this memory and
        // `AtomicU32` is the correct type to access it concurrently.
        unsafe { &*self.ptr.add(off).cast::<AtomicU32>() }
    }

    fn read<T: Copy>(&self, off: usize) -> T {
        self.check::<T>(off);
        // SAFETY: in bounds and aligned (checked); `T` is plain-old-data from the ABI.
        unsafe { self.ptr.add(off).cast::<T>().read_volatile() }
    }

    fn write<T: Copy>(&self, off: usize, value: T) {
        self.check::<T>(off);
        // SAFETY: in bounds and aligned (checked); the slot is owned by the producer
        // (us) until the tail is published with release ordering.
        unsafe { self.ptr.add(off).cast::<T>().write_volatile(value) }
    }
}

impl Drop for Mapping {
    fn drop(&mut self) {
        // SAFETY: `ptr`/`len` came from a successful mmap and are unmapped exactly once.
        unsafe {
            libc::munmap(self.ptr.cast(), self.len);
        }
    }
}

// ---------------------------------------------------------------------------
// Public data model
// ---------------------------------------------------------------------------

/// Which engine is driving a [`Ring`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    IoUring,
    Epoll,
}

impl Backend {
    pub fn name(self) -> &'static str {
        match self {
            Backend::IoUring => "io_uring",
            Backend::Epoll => "epoll",
        }
    }
}

/// Engine selection policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Preference {
    /// Native io_uring, falling back to the epoll bridge when unavailable.
    #[default]
    Auto,
    /// Native io_uring only; setup errors are returned.
    Native,
    /// Force the epoll bridge (tests, sandboxes).
    Emulated,
}

#[derive(Clone, Copy, Debug)]
pub struct Options {
    pub entries: u32,
    pub sqpoll: bool,
    pub backend: Preference,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            entries: 256,
            sqpoll: false,
            backend: Preference::Auto,
        }
    }
}

/// Where an operation's descriptor comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Fd(RawFd),
    /// Index into the registered-file table.
    Fixed(u32),
}

/// One submission-queue operation (buffers travel separately, see [`Ring::prep`]).
#[derive(Clone, Copy, Debug)]
pub enum Op {
    Nop,
    Read {
        target: Target,
        offset: u64,
    },
    Write {
        target: Target,
        offset: u64,
    },
    Readv {
        target: Target,
        offset: u64,
    },
    Writev {
        target: Target,
        offset: u64,
    },
    ReadFixed {
        target: Target,
        buf: u16,
        len: u32,
        offset: u64,
    },
    WriteFixed {
        target: Target,
        buf: u16,
        len: u32,
        offset: u64,
    },
    Accept {
        target: Target,
    },
    Recv {
        target: Target,
    },
    Send {
        target: Target,
    },
    PollAdd {
        target: Target,
        events: u32,
    },
    Timeout {
        nanos: u64,
    },
    Fsync {
        target: Target,
    },
}

impl Op {
    fn target(&self) -> Option<Target> {
        match *self {
            Op::Nop | Op::Timeout { .. } => None,
            Op::Read { target, .. }
            | Op::Write { target, .. }
            | Op::Readv { target, .. }
            | Op::Writev { target, .. }
            | Op::ReadFixed { target, .. }
            | Op::WriteFixed { target, .. }
            | Op::Accept { target }
            | Op::Recv { target }
            | Op::Send { target }
            | Op::PollAdd { target, .. }
            | Op::Fsync { target } => Some(target),
        }
    }
}

/// A reaped completion.
#[derive(Debug)]
pub struct Completion {
    pub token: u64,
    /// Bytes transferred, ready events, or `-errno`.
    pub result: i64,
    pub flags: u32,
    /// Bytes produced by read-like operations (truncated to `result`).
    pub data: Vec<u8>,
    /// The connection returned by a successful `Accept`.
    pub accepted: Option<OwnedFd>,
    /// True when this completion belongs to a `PollAdd`.
    pub is_poll: bool,
}

/// A registered, kernel-pinned buffer.
struct FixedBuf {
    data: Vec<u8>,
    busy: bool,
}

/// A queued or in-flight operation and the memory the kernel may touch for it.
struct Inflight {
    token: u64,
    op: Op,
    bufs: Vec<Vec<u8>>,
    iov: Vec<IoVec>,
    ts: Box<Timespec>,
}

struct RawCqe {
    id: u64,
    res: i32,
    flags: u32,
}

struct Ctx<'a> {
    ops: &'a mut HashMap<u64, Inflight>,
    fixed: &'a mut Vec<FixedBuf>,
}

trait Engine: Send {
    fn backend(&self) -> Backend;
    fn sqpoll(&self) -> bool;
    fn cq_entries(&self) -> usize;
    /// Stage an operation (not yet visible to the kernel).
    fn queue(&mut self, id: u64, fl: &Inflight, fixed: &[FixedBuf]) -> io::Result<()>;
    /// Number of staged-but-unsubmitted operations.
    fn queued(&self) -> usize;
    /// Flush staged operations; returns how many were handed over.
    fn submit(&mut self, ctx: &mut Ctx) -> io::Result<usize>;
    /// Flush, then wait for `min` completions (or `timeout`) and move up to `max` into `out`.
    fn wait(
        &mut self,
        ctx: &mut Ctx,
        min: usize,
        timeout: Option<Duration>,
        max: usize,
        out: &mut Vec<RawCqe>,
    ) -> io::Result<()>;
    fn register_buffers(&mut self, fixed: &[FixedBuf]) -> io::Result<()>;
    fn unregister_buffers(&mut self) -> io::Result<()>;
    fn register_files(&mut self, fds: &[RawFd]) -> io::Result<()>;
    fn unregister_files(&mut self) -> io::Result<()>;
}

// ---------------------------------------------------------------------------
// Native io_uring engine
// ---------------------------------------------------------------------------

struct Native {
    sq_map: Mapping,
    cq_map: Option<Mapping>,
    sqes: Mapping,
    sq_head: u32,
    sq_tail: u32,
    sq_flags: u32,
    sq_mask: u32,
    sq_array: u32,
    cq_head: u32,
    cq_tail: u32,
    cq_mask: u32,
    cqes_off: u32,
    sq_entries: u32,
    cq_entries: u32,
    local_tail: u32,
    unsubmitted: u32,
    sqpoll: bool,
    // Declared last so the mappings are released before the ring descriptor closes.
    fd: OwnedFd,
}

impl Native {
    fn new(entries: u32, sqpoll: bool) -> io::Result<Self> {
        let mut p = Params::default();
        if sqpoll {
            p.flags |= abi::SETUP_SQPOLL;
            p.sq_thread_idle = 100; // ms before the kernel thread sleeps
        }
        let fd = io_uring_setup(entries.clamp(1, MAX_ENTRIES), &mut p)?;
        if p.features & abi::FEAT_EXT_ARG == 0 {
            // Pre-5.11 kernel: no timed waits. The epoll bridge covers it.
            return Err(os_err(libc::ENOSYS));
        }
        let sq_len = p.sq_off.array as usize + p.sq_entries as usize * size_of::<u32>();
        let cq_len = p.cq_off.cqes as usize + p.cq_entries as usize * size_of::<Cqe>();
        let single = p.features & abi::FEAT_SINGLE_MMAP != 0;
        let sq_map = Mapping::map(
            fd.as_raw_fd(),
            if single { sq_len.max(cq_len) } else { sq_len },
            abi::OFF_SQ_RING,
        )?;
        let cq_map = if single {
            None
        } else {
            Some(Mapping::map(fd.as_raw_fd(), cq_len, abi::OFF_CQ_RING)?)
        };
        let sqes = Mapping::map(
            fd.as_raw_fd(),
            p.sq_entries as usize * size_of::<Sqe>(),
            abi::OFF_SQES,
        )?;
        let sq_tail_now = sq_map.atomic(p.sq_off.tail).load(Ordering::Relaxed);
        Ok(Self {
            sq_head: p.sq_off.head,
            sq_tail: p.sq_off.tail,
            sq_flags: p.sq_off.flags,
            sq_mask: sq_map.atomic(p.sq_off.ring_mask).load(Ordering::Relaxed),
            sq_array: p.sq_off.array,
            cq_head: p.cq_off.head,
            cq_tail: p.cq_off.tail,
            cq_mask: cq_map
                .as_ref()
                .unwrap_or(&sq_map)
                .atomic(p.cq_off.ring_mask)
                .load(Ordering::Relaxed),
            cqes_off: p.cq_off.cqes,
            sq_entries: p.sq_entries,
            cq_entries: p.cq_entries,
            local_tail: sq_tail_now,
            unsubmitted: 0,
            sqpoll: p.flags & abi::SETUP_SQPOLL != 0,
            sq_map,
            cq_map,
            sqes,
            fd,
        })
    }

    fn cq(&self) -> &Mapping {
        self.cq_map.as_ref().unwrap_or(&self.sq_map)
    }

    fn resolve(target: Target) -> (i32, u8) {
        match target {
            Target::Fd(fd) => (fd, 0),
            Target::Fixed(index) => (index as i32, abi::SQE_FIXED_FILE),
        }
    }

    fn encode(&self, id: u64, fl: &Inflight, fixed: &[FixedBuf]) -> Sqe {
        let mut sqe = Sqe {
            user_data: id,
            ..Sqe::default()
        };
        let set_target = |sqe: &mut Sqe, target: Target| {
            let (fd, flags) = Self::resolve(target);
            sqe.fd = fd;
            sqe.flags |= flags;
        };
        let first = fl.bufs.first();
        match fl.op {
            Op::Nop => sqe.opcode = abi::OP_NOP,
            Op::Read { target, offset } => {
                sqe.opcode = abi::OP_READ;
                set_target(&mut sqe, target);
                sqe.off = offset;
                if let Some(b) = first {
                    sqe.addr = b.as_ptr() as u64;
                    sqe.len = b.len() as u32;
                }
            }
            Op::Write { target, offset } => {
                sqe.opcode = abi::OP_WRITE;
                set_target(&mut sqe, target);
                sqe.off = offset;
                if let Some(b) = first {
                    sqe.addr = b.as_ptr() as u64;
                    sqe.len = b.len() as u32;
                }
            }
            Op::Readv { target, offset } | Op::Writev { target, offset } => {
                sqe.opcode = if matches!(fl.op, Op::Readv { .. }) {
                    abi::OP_READV
                } else {
                    abi::OP_WRITEV
                };
                set_target(&mut sqe, target);
                sqe.off = offset;
                sqe.addr = fl.iov.as_ptr() as u64;
                sqe.len = fl.iov.len() as u32;
            }
            Op::ReadFixed {
                target,
                buf,
                len,
                offset,
            }
            | Op::WriteFixed {
                target,
                buf,
                len,
                offset,
            } => {
                sqe.opcode = if matches!(fl.op, Op::ReadFixed { .. }) {
                    abi::OP_READ_FIXED
                } else {
                    abi::OP_WRITE_FIXED
                };
                set_target(&mut sqe, target);
                sqe.off = offset;
                sqe.addr = fixed[buf as usize].data.as_ptr() as u64;
                sqe.len = len;
                sqe.buf_index = buf;
            }
            Op::Accept { target } => {
                sqe.opcode = abi::OP_ACCEPT;
                set_target(&mut sqe, target);
                sqe.op_flags = libc::SOCK_CLOEXEC as u32;
            }
            Op::Recv { target } | Op::Send { target } => {
                sqe.opcode = if matches!(fl.op, Op::Recv { .. }) {
                    abi::OP_RECV
                } else {
                    abi::OP_SEND
                };
                set_target(&mut sqe, target);
                if matches!(fl.op, Op::Send { .. }) {
                    sqe.op_flags = libc::MSG_NOSIGNAL as u32;
                }
                if let Some(b) = first {
                    sqe.addr = b.as_ptr() as u64;
                    sqe.len = b.len() as u32;
                }
            }
            Op::PollAdd { target, events } => {
                sqe.opcode = abi::OP_POLL_ADD;
                set_target(&mut sqe, target);
                sqe.op_flags = if cfg!(target_endian = "big") {
                    events.rotate_left(16)
                } else {
                    events
                };
            }
            Op::Timeout { .. } => {
                sqe.opcode = abi::OP_TIMEOUT;
                sqe.fd = -1;
                sqe.addr = std::ptr::from_ref::<Timespec>(&fl.ts) as u64;
                sqe.len = 1;
            }
            Op::Fsync { target } => {
                sqe.opcode = abi::OP_FSYNC;
                set_target(&mut sqe, target);
            }
        }
        sqe
    }

    fn sq_full(&self) -> bool {
        let head = self.sq_map.atomic(self.sq_head).load(Ordering::Acquire);
        self.local_tail.wrapping_sub(head) >= self.sq_entries
    }

    /// Publish staged entries and enter the kernel.
    fn flush(&mut self, min_complete: u32, timeout: Option<Duration>) -> io::Result<usize> {
        let handed = self.unsubmitted as usize;
        self.sq_map
            .atomic(self.sq_tail)
            .store(self.local_tail, Ordering::Release);
        if self.sqpoll {
            // The kernel thread consumes the SQ on its own; wake it only if it sleeps.
            fence(Ordering::SeqCst);
            let mut flags = 0;
            if self.sq_map.atomic(self.sq_flags).load(Ordering::Relaxed) & abi::SQ_NEED_WAKEUP != 0
            {
                flags |= abi::ENTER_SQ_WAKEUP;
            }
            if min_complete > 0 {
                flags |= abi::ENTER_GETEVENTS;
            }
            if flags != 0 {
                self.enter(0, min_complete, flags, timeout)?;
            }
            self.unsubmitted = 0;
            return Ok(handed);
        }
        loop {
            let consumed = self.enter(
                self.unsubmitted,
                min_complete,
                abi::ENTER_GETEVENTS,
                timeout,
            )?;
            self.unsubmitted = self.unsubmitted.saturating_sub(consumed);
            if self.unsubmitted == 0 || consumed == 0 {
                break;
            }
        }
        Ok(handed - self.unsubmitted as usize)
    }

    fn enter(
        &self,
        to_submit: u32,
        min_complete: u32,
        mut flags: u32,
        timeout: Option<Duration>,
    ) -> io::Result<u32> {
        let ts;
        let arg;
        let (ptr, size) = match timeout {
            Some(t) if min_complete > 0 => {
                ts = to_timespec(t);
                arg = GetEventsArg {
                    sigmask: 0,
                    sigmask_sz: 0,
                    pad: 0,
                    ts: std::ptr::from_ref(&ts) as u64,
                };
                flags |= abi::ENTER_EXT_ARG;
                (
                    std::ptr::from_ref(&arg).cast::<libc::c_void>(),
                    size_of::<GetEventsArg>(),
                )
            }
            _ => (std::ptr::null(), 8),
        };
        match io_uring_enter(
            self.fd.as_raw_fd(),
            to_submit,
            min_complete,
            flags,
            ptr,
            size,
        ) {
            Ok(n) => Ok(n),
            // Timer expired, signal, or CQ overflow backpressure: not failures; the
            // caller drains the CQ and any staged entries remain queued.
            Err(e)
                if matches!(
                    e.raw_os_error(),
                    Some(libc::ETIME | libc::EINTR | libc::EAGAIN | libc::EBUSY)
                ) =>
            {
                Ok(0)
            }
            Err(e) => Err(e),
        }
    }

    fn drain(&mut self, max: usize, out: &mut Vec<RawCqe>) {
        let cq = self.cq();
        let tail = cq.atomic(self.cq_tail).load(Ordering::Acquire);
        let head_cell = cq.atomic(self.cq_head);
        let mut head = head_cell.load(Ordering::Relaxed);
        let mut taken = 0;
        while head != tail && taken < max {
            let slot = self.cqes_off as usize + (head & self.cq_mask) as usize * size_of::<Cqe>();
            let cqe: Cqe = cq.read(slot);
            out.push(RawCqe {
                id: cqe.user_data,
                res: cqe.res,
                flags: cqe.flags,
            });
            head = head.wrapping_add(1);
            taken += 1;
        }
        head_cell.store(head, Ordering::Release);
    }
}

impl Engine for Native {
    fn backend(&self) -> Backend {
        Backend::IoUring
    }

    fn sqpoll(&self) -> bool {
        self.sqpoll
    }

    fn cq_entries(&self) -> usize {
        self.cq_entries as usize
    }

    fn queue(&mut self, id: u64, fl: &Inflight, fixed: &[FixedBuf]) -> io::Result<()> {
        let sqe = self.encode(id, fl, fixed);
        if self.sq_full() {
            self.flush(0, None)?;
            if self.sq_full() && self.sqpoll {
                self.enter(0, 0, abi::ENTER_SQ_WAIT, None)?;
            }
            if self.sq_full() {
                return Err(os_err(libc::EBUSY));
            }
        }
        let index = self.local_tail & self.sq_mask;
        self.sqes.write(index as usize * size_of::<Sqe>(), sqe);
        self.sq_map.write(
            self.sq_array as usize + index as usize * size_of::<u32>(),
            index,
        );
        self.local_tail = self.local_tail.wrapping_add(1);
        self.unsubmitted += 1;
        Ok(())
    }

    fn queued(&self) -> usize {
        self.unsubmitted as usize
    }

    fn submit(&mut self, _ctx: &mut Ctx) -> io::Result<usize> {
        self.flush(0, None)
    }

    fn wait(
        &mut self,
        _ctx: &mut Ctx,
        min: usize,
        timeout: Option<Duration>,
        max: usize,
        out: &mut Vec<RawCqe>,
    ) -> io::Result<()> {
        self.flush(u32::try_from(min).unwrap_or(u32::MAX), timeout)?;
        self.drain(max, out);
        Ok(())
    }

    fn register_buffers(&mut self, fixed: &[FixedBuf]) -> io::Result<()> {
        let iov: Vec<IoVec> = fixed
            .iter()
            .map(|b| IoVec {
                base: b.data.as_ptr() as usize,
                len: b.data.len(),
            })
            .collect();
        io_uring_register(
            self.fd.as_raw_fd(),
            abi::REGISTER_BUFFERS,
            iov.as_ptr().cast(),
            iov.len() as u32,
        )
    }

    fn unregister_buffers(&mut self) -> io::Result<()> {
        io_uring_register(
            self.fd.as_raw_fd(),
            abi::UNREGISTER_BUFFERS,
            std::ptr::null(),
            0,
        )
    }

    fn register_files(&mut self, fds: &[RawFd]) -> io::Result<()> {
        io_uring_register(
            self.fd.as_raw_fd(),
            abi::REGISTER_FILES,
            fds.as_ptr().cast(),
            fds.len() as u32,
        )
    }

    fn unregister_files(&mut self) -> io::Result<()> {
        io_uring_register(
            self.fd.as_raw_fd(),
            abi::UNREGISTER_FILES,
            std::ptr::null(),
            0,
        )
    }
}

// ---------------------------------------------------------------------------
// Epoll bridge engine (fallback)
// ---------------------------------------------------------------------------

enum Step {
    Done(i32),
    Wait { fd: RawFd, events: u32 },
}

struct Pending {
    id: u64,
    /// Timers complete at their deadline with `-ETIME`.
    deadline: Option<Instant>,
}

struct Emulated {
    epfd: OwnedFd,
    staged: Vec<u64>,
    pending: Vec<Pending>,
    done: VecDeque<RawCqe>,
    /// Duplicated descriptors, mimicking the kernel's registered-file references.
    files: Vec<OwnedFd>,
    entries: usize,
}

impl Emulated {
    fn new(entries: u32) -> io::Result<Self> {
        // SAFETY: epoll_create1 takes no pointers; the result is checked below.
        let raw = unsafe { libc::epoll_create1(libc::EPOLL_CLOEXEC) };
        if raw < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            // SAFETY: `raw` is a fresh descriptor owned by nobody else.
            epfd: unsafe { OwnedFd::from_raw_fd(raw) },
            staged: Vec::new(),
            pending: Vec::new(),
            done: VecDeque::new(),
            files: Vec::new(),
            entries: entries.clamp(1, MAX_ENTRIES) as usize,
        })
    }

    fn raw(&self, target: Target) -> Result<RawFd, i32> {
        match target {
            Target::Fd(fd) => Ok(fd),
            Target::Fixed(i) => self
                .files
                .get(i as usize)
                .map(AsRawFd::as_raw_fd)
                .ok_or(libc::EBADF),
        }
    }

    /// Non-blocking `poll(2)`: ready events, or `-errno`.
    fn poll_now(fd: RawFd, events: u32) -> i32 {
        let mut p = libc::pollfd {
            fd,
            events: events as i16,
            revents: 0,
        };
        loop {
            // SAFETY: `p` is a valid pollfd and nfds is 1.
            let r = unsafe { libc::poll(&mut p, 1, 0) };
            if r < 0 {
                if errno() == libc::EINTR {
                    continue;
                }
                return -errno();
            }
            if p.revents & libc::POLLNVAL != 0 {
                return -libc::EBADF;
            }
            return i32::from(p.revents);
        }
    }

    /// `Some(step)` when the descriptor is not ready (or broken), `None` to proceed.
    fn gate(fd: RawFd, events: u32) -> Option<Step> {
        let r = Self::poll_now(fd, events);
        if r < 0 {
            return Some(Step::Done(r));
        }
        let wake = events as i32 | i32::from(libc::POLLERR) | i32::from(libc::POLLHUP);
        if r & wake == 0 {
            return Some(Step::Wait { fd, events });
        }
        None
    }

    fn file_kind(fd: RawFd) -> Option<u32> {
        // SAFETY: zeroed `stat` is a valid out-parameter for fstat.
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        // SAFETY: `st` is valid for writes; a bad fd is reported via the return value.
        (unsafe { libc::fstat(fd, &mut st) } == 0).then_some(st.st_mode & libc::S_IFMT)
    }

    fn finish(r: isize, fd: RawFd, events: u32) -> Step {
        if r >= 0 {
            return Step::Done(i32::try_from(r).unwrap_or(i32::MAX));
        }
        match errno() {
            libc::EAGAIN | libc::EINTR => Step::Wait { fd, events },
            e => Step::Done(-e),
        }
    }

    fn read_vec(fd: RawFd, iov: &[libc::iovec], offset: u64) -> Step {
        let kind = Self::file_kind(fd);
        if let Some(libc::S_IFSOCK) = kind {
            // SAFETY: zeroed msghdr with only iov fields set; `iov` outlives the call.
            let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
            msg.msg_iov = iov.as_ptr().cast_mut();
            msg.msg_iovlen = iov.len() as _;
            // SAFETY: `msg` references valid, writable buffers for the whole call.
            let r = unsafe { libc::recvmsg(fd, &mut msg, libc::MSG_DONTWAIT) };
            return Self::finish(r, fd, libc::EPOLLIN as u32);
        }
        // SAFETY: `iov` entries point at live, writable buffers owned by the operation.
        let r = unsafe {
            if kind == Some(libc::S_IFREG) && offset != CUR_POS {
                libc::preadv(fd, iov.as_ptr(), iov.len() as i32, offset as i64)
            } else {
                libc::readv(fd, iov.as_ptr(), iov.len() as i32)
            }
        };
        Self::finish(r, fd, libc::EPOLLIN as u32)
    }

    fn write_vec(fd: RawFd, iov: &[libc::iovec], offset: u64) -> Step {
        let kind = Self::file_kind(fd);
        if let Some(libc::S_IFSOCK) = kind {
            // SAFETY: zeroed msghdr with only iov fields set; `iov` outlives the call.
            let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
            msg.msg_iov = iov.as_ptr().cast_mut();
            msg.msg_iovlen = iov.len() as _;
            // SAFETY: `msg` references valid readable buffers for the whole call.
            let r = unsafe { libc::sendmsg(fd, &msg, libc::MSG_DONTWAIT | libc::MSG_NOSIGNAL) };
            return Self::finish(r, fd, libc::EPOLLOUT as u32);
        }
        // SAFETY: `iov` entries point at live, readable buffers owned by the operation.
        let r = unsafe {
            if kind == Some(libc::S_IFREG) && offset != CUR_POS {
                libc::pwritev(fd, iov.as_ptr(), iov.len() as i32, offset as i64)
            } else {
                libc::writev(fd, iov.as_ptr(), iov.len() as i32)
            }
        };
        Self::finish(r, fd, libc::EPOLLOUT as u32)
    }

    fn iovecs(bufs: &mut [Vec<u8>]) -> Vec<libc::iovec> {
        bufs.iter_mut()
            .map(|b| libc::iovec {
                iov_base: b.as_mut_ptr().cast(),
                iov_len: b.len(),
            })
            .collect()
    }

    fn step(&self, fl: &mut Inflight, fixed: &mut [FixedBuf]) -> Step {
        let fd = match fl.op.target() {
            None => 0,
            Some(t) => match self.raw(t) {
                Ok(fd) => fd,
                Err(e) => return Step::Done(-e),
            },
        };
        let pollin = libc::EPOLLIN as u32;
        let pollout = libc::EPOLLOUT as u32;
        match fl.op {
            Op::Nop => Step::Done(0),
            Op::Timeout { .. } => Step::Done(ETIME_RESULT as i32),
            Op::PollAdd { events, .. } => {
                let r = Self::poll_now(fd, events);
                if r < 0 || r & (events as i32 | 0x18) != 0 {
                    Step::Done(r)
                } else {
                    Step::Wait { fd, events }
                }
            }
            Op::Accept { .. } => {
                if let Some(s) = Self::gate(fd, pollin) {
                    return s;
                }
                // SAFETY: null address/length is permitted; the new fd is returned.
                let r = unsafe {
                    libc::accept4(
                        fd,
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        libc::SOCK_CLOEXEC,
                    )
                };
                Self::finish(r as isize, fd, pollin)
            }
            Op::Fsync { .. } => {
                // SAFETY: plain syscall on a descriptor; errors come back via errno.
                let r = unsafe { libc::fsync(fd) };
                Self::finish(r as isize, fd, pollout)
            }
            Op::Read { offset, .. } | Op::Readv { offset, .. } => {
                if let Some(s) = Self::gate(fd, pollin) {
                    return s;
                }
                let iov = Self::iovecs(&mut fl.bufs);
                Self::read_vec(fd, &iov, offset)
            }
            Op::Recv { .. } => {
                if let Some(s) = Self::gate(fd, pollin) {
                    return s;
                }
                let iov = Self::iovecs(&mut fl.bufs);
                Self::read_vec(fd, &iov, CUR_POS)
            }
            Op::Write { offset, .. } | Op::Writev { offset, .. } => {
                if let Some(s) = Self::gate(fd, pollout) {
                    return s;
                }
                let iov = Self::iovecs(&mut fl.bufs);
                Self::write_vec(fd, &iov, offset)
            }
            Op::Send { .. } => {
                if let Some(s) = Self::gate(fd, pollout) {
                    return s;
                }
                let iov = Self::iovecs(&mut fl.bufs);
                Self::write_vec(fd, &iov, CUR_POS)
            }
            Op::ReadFixed {
                buf, len, offset, ..
            } => {
                if let Some(s) = Self::gate(fd, pollin) {
                    return s;
                }
                let data = &mut fixed[buf as usize].data;
                let iov = [libc::iovec {
                    iov_base: data.as_mut_ptr().cast(),
                    iov_len: len as usize,
                }];
                Self::read_vec(fd, &iov, offset)
            }
            Op::WriteFixed {
                buf, len, offset, ..
            } => {
                if let Some(s) = Self::gate(fd, pollout) {
                    return s;
                }
                let data = &mut fixed[buf as usize].data;
                let iov = [libc::iovec {
                    iov_base: data.as_mut_ptr().cast(),
                    iov_len: len as usize,
                }];
                Self::write_vec(fd, &iov, offset)
            }
        }
    }

    /// Hand staged operations to the pending set (timers start now).
    fn flush_staged(&mut self, ctx: &Ctx) -> usize {
        let n = self.staged.len();
        let now = Instant::now();
        for id in std::mem::take(&mut self.staged) {
            let deadline = match ctx.ops.get(&id).map(|f| f.op) {
                Some(Op::Timeout { nanos }) => Some(now + Duration::from_nanos(nanos)),
                _ => None,
            };
            self.pending.push(Pending { id, deadline });
        }
        n
    }

    /// Sleep until one of `waits` may be ready, or `wait_for` elapses.
    fn block(&self, waits: &[(RawFd, u32)], wait_for: Option<Duration>) {
        let mut merged: Vec<(RawFd, u32)> = Vec::new();
        for &(fd, ev) in waits {
            match merged.iter_mut().find(|(f, _)| *f == fd) {
                Some(slot) => slot.1 |= ev,
                None => merged.push((fd, ev)),
            }
        }
        let epfd = self.epfd.as_raw_fd();
        let mut armed = 0usize;
        for &(fd, ev) in &merged {
            let mut event = libc::epoll_event {
                events: ev | libc::EPOLLONESHOT as u32,
                u64: fd as u64,
            };
            // SAFETY: `event` is a valid epoll_event for the duration of each call.
            let mut r = unsafe { libc::epoll_ctl(epfd, libc::EPOLL_CTL_MOD, fd, &mut event) };
            if r < 0 && errno() == libc::ENOENT {
                // SAFETY: as above.
                r = unsafe { libc::epoll_ctl(epfd, libc::EPOLL_CTL_ADD, fd, &mut event) };
            }
            if r == 0 {
                armed += 1;
            }
        }
        if armed < merged.len() {
            // Some descriptor cannot be epoll-ed: poll it on a short cadence instead.
            let slice = Duration::from_millis(1);
            std::thread::sleep(wait_for.map_or(slice, |w| w.min(slice)));
            return;
        }
        let ms = wait_for.map_or(-1, |d| {
            i32::try_from(d.as_nanos().div_ceil(1_000_000)).unwrap_or(i32::MAX)
        });
        let mut events = [libc::epoll_event { events: 0, u64: 0 }; 16];
        // SAFETY: `events` provides 16 writable slots; the kernel fills at most that many.
        unsafe {
            libc::epoll_wait(epfd, events.as_mut_ptr(), events.len() as i32, ms);
        }
    }
}

impl Engine for Emulated {
    fn backend(&self) -> Backend {
        Backend::Epoll
    }

    fn sqpoll(&self) -> bool {
        false
    }

    fn cq_entries(&self) -> usize {
        self.entries * 2
    }

    fn queue(&mut self, id: u64, _fl: &Inflight, _fixed: &[FixedBuf]) -> io::Result<()> {
        if self.staged.len() >= self.entries {
            return Err(os_err(libc::EBUSY));
        }
        self.staged.push(id);
        Ok(())
    }

    fn queued(&self) -> usize {
        self.staged.len()
    }

    fn submit(&mut self, ctx: &mut Ctx) -> io::Result<usize> {
        Ok(self.flush_staged(ctx))
    }

    fn wait(
        &mut self,
        ctx: &mut Ctx,
        min: usize,
        timeout: Option<Duration>,
        max: usize,
        out: &mut Vec<RawCqe>,
    ) -> io::Result<()> {
        self.flush_staged(ctx);
        let limit = timeout.map(|t| Instant::now() + t);
        loop {
            let mut waits: Vec<(RawFd, u32)> = Vec::new();
            let mut next_timer: Option<Instant> = None;
            let now = Instant::now();
            let mut i = 0;
            while i < self.pending.len() {
                let id = self.pending[i].id;
                let deadline = self.pending[i].deadline;
                let Some(fl) = ctx.ops.get_mut(&id) else {
                    self.pending.remove(i);
                    continue;
                };
                let step = match deadline {
                    Some(d) if now >= d => Step::Done(ETIME_RESULT as i32),
                    Some(d) => {
                        next_timer = Some(next_timer.map_or(d, |t| t.min(d)));
                        i += 1;
                        continue;
                    }
                    None => self.step(fl, ctx.fixed),
                };
                match step {
                    Step::Done(res) => {
                        self.done.push_back(RawCqe { id, res, flags: 0 });
                        self.pending.remove(i);
                    }
                    Step::Wait { fd, events } => {
                        waits.push((fd, events));
                        i += 1;
                    }
                }
            }
            let now = Instant::now();
            let expired = limit.is_some_and(|l| now >= l);
            if self.done.len() >= min || expired || self.pending.is_empty() {
                break;
            }
            let mut wait_for = limit.map(|l| l.saturating_duration_since(now));
            if let Some(t) = next_timer {
                let until = t.saturating_duration_since(now);
                wait_for = Some(wait_for.map_or(until, |w| w.min(until)));
            }
            self.block(&waits, wait_for);
        }
        while out.len() < max {
            let Some(cqe) = self.done.pop_front() else {
                break;
            };
            out.push(cqe);
        }
        Ok(())
    }

    fn register_buffers(&mut self, _fixed: &[FixedBuf]) -> io::Result<()> {
        Ok(())
    }

    fn unregister_buffers(&mut self) -> io::Result<()> {
        Ok(())
    }

    fn register_files(&mut self, fds: &[RawFd]) -> io::Result<()> {
        if !self.files.is_empty() {
            return Err(os_err(libc::EBUSY));
        }
        let mut dups = Vec::with_capacity(fds.len());
        for &fd in fds {
            // SAFETY: F_DUPFD_CLOEXEC takes an integer argument and returns a new fd.
            let dup = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0) };
            if dup < 0 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: `dup` is a fresh descriptor owned by nobody else.
            dups.push(unsafe { OwnedFd::from_raw_fd(dup) });
        }
        self.files = dups;
        Ok(())
    }

    fn unregister_files(&mut self) -> io::Result<()> {
        self.files.clear();
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Ring: the public handle
// ---------------------------------------------------------------------------

/// An io_uring instance (native or epoll-bridged) with owned in-flight buffers.
pub struct Ring {
    engine: Box<dyn Engine>,
    ops: HashMap<u64, Inflight>,
    fixed: Vec<FixedBuf>,
    next_id: u64,
    files_registered: u32,
}

/// Errors that mean "io_uring is not usable here" rather than "bad request".
fn is_unavailable(err: &io::Error) -> bool {
    matches!(
        err.raw_os_error(),
        Some(
            libc::ENOSYS
                | libc::EPERM
                | libc::EACCES
                | libc::EINVAL
                | libc::EOPNOTSUPP
                | libc::ENOMEM
        )
    )
}

impl Ring {
    pub fn open(opts: Options) -> io::Result<Self> {
        let entries = opts.entries.clamp(1, MAX_ENTRIES);
        let native = || -> io::Result<Native> {
            if opts.sqpoll {
                Native::new(entries, true).or_else(|_| Native::new(entries, false))
            } else {
                Native::new(entries, false)
            }
        };
        let engine: Box<dyn Engine> = match opts.backend {
            Preference::Emulated => Box::new(Emulated::new(entries)?),
            Preference::Native => Box::new(native()?),
            Preference::Auto => match native() {
                Ok(n) => Box::new(n),
                Err(e) if is_unavailable(&e) => Box::new(Emulated::new(entries)?),
                Err(e) => return Err(e),
            },
        };
        Ok(Self {
            engine,
            ops: HashMap::new(),
            fixed: Vec::new(),
            next_id: 1,
            files_registered: 0,
        })
    }

    /// Whether the kernel offers a usable native ring on this host.
    pub fn native_available() -> bool {
        Native::new(2, false).is_ok()
    }

    pub fn backend(&self) -> Backend {
        self.engine.backend()
    }

    pub fn sqpoll_active(&self) -> bool {
        self.engine.sqpoll()
    }

    /// Operations submitted or staged whose completion has not been reaped.
    pub fn inflight(&self) -> usize {
        self.ops.len()
    }

    /// Staged operations not yet handed to the kernel.
    pub fn queued(&self) -> usize {
        self.engine.queued()
    }

    pub fn fixed_buffer_count(&self) -> usize {
        self.fixed.len()
    }

    pub fn fixed_file_count(&self) -> u32 {
        self.files_registered
    }

    fn validate(&self, op: &Op, bufs: &[Vec<u8>]) -> io::Result<()> {
        let bad = || Err(os_err(libc::EINVAL));
        if let Some(Target::Fixed(i)) = op.target()
            && i >= self.files_registered
        {
            return Err(os_err(libc::EBADF));
        }
        let fits = |b: &Vec<u8>| u32::try_from(b.len()).is_ok();
        match op {
            Op::Read { .. } | Op::Recv { .. } => {
                if bufs.len() != 1 || bufs[0].is_empty() || !fits(&bufs[0]) {
                    return bad();
                }
            }
            Op::Write { .. } | Op::Send { .. } => {
                if bufs.len() != 1 || !fits(&bufs[0]) {
                    return bad();
                }
            }
            Op::Readv { .. } | Op::Writev { .. } => {
                if bufs.is_empty() || bufs.len() > MAX_IOV || !bufs.iter().all(fits) {
                    return bad();
                }
            }
            Op::ReadFixed { buf, len, .. } | Op::WriteFixed { buf, len, .. } => {
                let Some(slot) = self.fixed.get(*buf as usize) else {
                    return bad();
                };
                if slot.busy || *len as usize > slot.data.len() {
                    return Err(os_err(if slot.busy { libc::EBUSY } else { libc::EINVAL }));
                }
                match op {
                    Op::WriteFixed { .. } => {
                        if bufs.len() != 1 || bufs[0].len() != *len as usize {
                            return bad();
                        }
                    }
                    _ if !bufs.is_empty() => return bad(),
                    _ => {}
                }
            }
            Op::Nop
            | Op::Accept { .. }
            | Op::PollAdd { .. }
            | Op::Timeout { .. }
            | Op::Fsync { .. } => {
                if !bufs.is_empty() {
                    return bad();
                }
            }
        }
        Ok(())
    }

    /// Stage one operation. Read-like operations pass zeroed destination buffers
    /// (their lengths define how much to read); write-like operations pass the payload.
    pub fn prep(&mut self, token: u64, op: Op, bufs: Vec<Vec<u8>>) -> io::Result<()> {
        if self.ops.len() >= self.engine.cq_entries() {
            // More in flight than the CQ can hold would overflow it: reap first.
            return Err(os_err(libc::EBUSY));
        }
        self.validate(&op, &bufs)?;
        let id = self.next_id;
        let iov = bufs
            .iter()
            .map(|b| IoVec {
                base: b.as_ptr() as usize,
                len: b.len(),
            })
            .collect();
        let ts = Box::new(match op {
            Op::Timeout { nanos } => to_timespec(Duration::from_nanos(nanos)),
            _ => Timespec::default(),
        });
        let fl = Inflight {
            token,
            op,
            bufs,
            iov,
            ts,
        };
        let fixed_slot = match op {
            Op::ReadFixed { buf, .. } | Op::WriteFixed { buf, .. } => Some(buf as usize),
            _ => None,
        };
        if let (Some(slot), Op::WriteFixed { len, .. }) = (fixed_slot, op) {
            self.fixed[slot].data[..len as usize].copy_from_slice(&fl.bufs[0]);
        }
        if let Some(slot) = fixed_slot {
            self.fixed[slot].busy = true;
        }
        if let Err(e) = self.engine.queue(id, &fl, &self.fixed) {
            if let Some(slot) = fixed_slot {
                self.fixed[slot].busy = false;
            }
            return Err(e);
        }
        self.next_id += 1;
        self.ops.insert(id, fl);
        Ok(())
    }

    pub fn prep_nop(&mut self, token: u64) -> io::Result<()> {
        self.prep(token, Op::Nop, Vec::new())
    }

    pub fn prep_read(
        &mut self,
        token: u64,
        target: Target,
        len: usize,
        offset: u64,
    ) -> io::Result<()> {
        self.prep(token, Op::Read { target, offset }, vec![vec![0; len]])
    }

    pub fn prep_write(
        &mut self,
        token: u64,
        target: Target,
        data: Vec<u8>,
        offset: u64,
    ) -> io::Result<()> {
        self.prep(token, Op::Write { target, offset }, vec![data])
    }

    pub fn prep_readv(
        &mut self,
        token: u64,
        target: Target,
        lens: &[usize],
        offset: u64,
    ) -> io::Result<()> {
        let bufs = lens.iter().map(|&n| vec![0; n]).collect();
        self.prep(token, Op::Readv { target, offset }, bufs)
    }

    pub fn prep_writev(
        &mut self,
        token: u64,
        target: Target,
        bufs: Vec<Vec<u8>>,
        offset: u64,
    ) -> io::Result<()> {
        self.prep(token, Op::Writev { target, offset }, bufs)
    }

    pub fn prep_read_fixed(
        &mut self,
        token: u64,
        target: Target,
        buf: u16,
        len: u32,
        offset: u64,
    ) -> io::Result<()> {
        self.prep(
            token,
            Op::ReadFixed {
                target,
                buf,
                len,
                offset,
            },
            Vec::new(),
        )
    }

    pub fn prep_write_fixed(
        &mut self,
        token: u64,
        target: Target,
        buf: u16,
        data: Vec<u8>,
        offset: u64,
    ) -> io::Result<()> {
        let len = u32::try_from(data.len()).map_err(|_| os_err(libc::EINVAL))?;
        self.prep(
            token,
            Op::WriteFixed {
                target,
                buf,
                len,
                offset,
            },
            vec![data],
        )
    }

    pub fn prep_accept(&mut self, token: u64, target: Target) -> io::Result<()> {
        self.prep(token, Op::Accept { target }, Vec::new())
    }

    pub fn prep_recv(&mut self, token: u64, target: Target, len: usize) -> io::Result<()> {
        self.prep(token, Op::Recv { target }, vec![vec![0; len]])
    }

    pub fn prep_send(&mut self, token: u64, target: Target, data: Vec<u8>) -> io::Result<()> {
        self.prep(token, Op::Send { target }, vec![data])
    }

    pub fn prep_poll_add(&mut self, token: u64, target: Target, events: u32) -> io::Result<()> {
        self.prep(token, Op::PollAdd { target, events }, Vec::new())
    }

    pub fn prep_timeout(&mut self, token: u64, after: Duration) -> io::Result<()> {
        let nanos = u64::try_from(after.as_nanos()).unwrap_or(u64::MAX);
        self.prep(token, Op::Timeout { nanos }, Vec::new())
    }

    pub fn prep_fsync(&mut self, token: u64, target: Target) -> io::Result<()> {
        self.prep(token, Op::Fsync { target }, Vec::new())
    }

    /// Hand all staged operations to the kernel in one batch.
    pub fn submit(&mut self) -> io::Result<usize> {
        let mut ctx = Ctx {
            ops: &mut self.ops,
            fixed: &mut self.fixed,
        };
        self.engine.submit(&mut ctx)
    }

    /// Submit staged operations, wait for `min` completions (or `timeout`; `None` waits
    /// indefinitely) and return up to `max` of them.
    pub fn submit_and_wait(
        &mut self,
        min: usize,
        timeout: Option<Duration>,
        max: usize,
    ) -> io::Result<Vec<Completion>> {
        let min = min.min(self.ops.len());
        let mut raw = Vec::new();
        {
            let mut ctx = Ctx {
                ops: &mut self.ops,
                fixed: &mut self.fixed,
            };
            self.engine
                .wait(&mut ctx, min, timeout, max.max(1), &mut raw)?;
        }
        Ok(raw.into_iter().filter_map(|c| self.finish(c)).collect())
    }

    /// Submit and reap whatever is already complete, without blocking.
    pub fn poll(&mut self, max: usize) -> io::Result<Vec<Completion>> {
        self.submit_and_wait(0, Some(Duration::ZERO), max)
    }

    fn finish(&mut self, raw: RawCqe) -> Option<Completion> {
        let mut fl = self.ops.remove(&raw.id)?;
        let result = i64::from(raw.res);
        let mut data = Vec::new();
        let mut accepted = None;
        match fl.op {
            Op::ReadFixed { buf, .. } => {
                let slot = &mut self.fixed[buf as usize];
                slot.busy = false;
                if result > 0 {
                    data = slot.data[..result as usize].to_vec();
                }
            }
            Op::WriteFixed { buf, .. } => self.fixed[buf as usize].busy = false,
            Op::Read { .. } | Op::Recv { .. } | Op::Readv { .. } if result > 0 => {
                let mut left = result as usize;
                if fl.bufs.len() == 1 {
                    data = fl.bufs.pop().unwrap_or_default();
                    data.truncate(left);
                } else {
                    for b in &fl.bufs {
                        let take = b.len().min(left);
                        data.extend_from_slice(&b[..take]);
                        left -= take;
                    }
                }
            }
            Op::Accept { .. } if result >= 0 => {
                // SAFETY: a non-negative accept result is a new descriptor the kernel
                // created for us; nothing else owns it.
                accepted = Some(unsafe { OwnedFd::from_raw_fd(raw.res) });
            }
            _ => {}
        }
        Some(Completion {
            token: fl.token,
            result,
            flags: raw.flags,
            data,
            accepted,
            is_poll: matches!(fl.op, Op::PollAdd { .. }),
        })
    }

    /// Allocate and register `count` fixed buffers of `size` bytes each.
    pub fn register_buffers(&mut self, count: usize, size: usize) -> io::Result<()> {
        if !self.fixed.is_empty() {
            return Err(os_err(libc::EBUSY));
        }
        if count == 0 || count > u16::MAX as usize || size == 0 || size > (1 << 30) {
            return Err(os_err(libc::EINVAL));
        }
        let fixed: Vec<FixedBuf> = (0..count)
            .map(|_| FixedBuf {
                data: vec![0; size],
                busy: false,
            })
            .collect();
        self.engine.register_buffers(&fixed)?;
        self.fixed = fixed;
        Ok(())
    }

    pub fn unregister_buffers(&mut self) -> io::Result<()> {
        if self.fixed.iter().any(|b| b.busy) {
            return Err(os_err(libc::EBUSY));
        }
        if self.fixed.is_empty() {
            return Ok(());
        }
        self.engine.unregister_buffers()?;
        self.fixed.clear();
        Ok(())
    }

    /// Register descriptors; operations may then use [`Target::Fixed`] indices.
    pub fn register_files(&mut self, fds: &[RawFd]) -> io::Result<()> {
        if self.files_registered != 0 {
            return Err(os_err(libc::EBUSY));
        }
        if fds.is_empty() {
            return Err(os_err(libc::EINVAL));
        }
        self.engine.register_files(fds)?;
        self.files_registered = fds.len() as u32;
        Ok(())
    }

    pub fn unregister_files(&mut self) -> io::Result<()> {
        if self.files_registered == 0 {
            return Ok(());
        }
        self.engine.unregister_files()?;
        self.files_registered = 0;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::FileExt;

    const SECS: Option<Duration> = Some(Duration::from_secs(5));

    /// Every backend this host can run; Native is skipped (not faked) when the kernel
    /// refuses io_uring.
    fn backends() -> Vec<Ring> {
        let mut rings = vec![
            Ring::open(Options {
                backend: Preference::Emulated,
                ..Options::default()
            })
            .unwrap(),
        ];
        if Ring::native_available() {
            rings.push(
                Ring::open(Options {
                    backend: Preference::Native,
                    ..Options::default()
                })
                .unwrap(),
            );
        } else {
            eprintln!("io_uring unavailable on this host: native tests skipped");
        }
        rings
    }

    fn temp_path(tag: &str) -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "super-uring-{tag}-{}-{:?}",
            std::process::id(),
            Instant::now()
        ));
        p
    }

    #[test]
    fn abi_layout() {
        assert_eq!(size_of::<Sqe>(), 64);
        assert_eq!(size_of::<Cqe>(), 16);
        assert_eq!(size_of::<Params>(), 120);
    }

    #[test]
    fn nop_batch_completes_with_tokens() {
        for mut ring in backends() {
            for token in 0..32 {
                ring.prep_nop(token).unwrap();
            }
            assert_eq!(ring.queued(), 32);
            let done = ring.submit_and_wait(32, SECS, 64).unwrap();
            assert_eq!(done.len(), 32, "{:?}", ring.backend());
            let mut tokens: Vec<u64> = done.iter().map(|c| c.token).collect();
            tokens.sort_unstable();
            assert_eq!(tokens, (0..32).collect::<Vec<_>>());
            assert!(done.iter().all(|c| c.result == 0));
            assert_eq!(ring.inflight(), 0);
        }
    }

    #[test]
    fn native_backend_is_reported_when_available() {
        let ring = Ring::open(Options::default()).unwrap();
        if Ring::native_available() {
            assert_eq!(ring.backend(), Backend::IoUring);
        } else {
            assert_eq!(ring.backend(), Backend::Epoll);
        }
        let forced = Ring::open(Options {
            backend: Preference::Emulated,
            ..Options::default()
        })
        .unwrap();
        assert_eq!(forced.backend(), Backend::Epoll);
        assert_eq!(forced.backend().name(), "epoll");
    }

    #[test]
    fn file_write_then_read_round_trip() {
        let path = temp_path("rw");
        for mut ring in backends() {
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(true)
                .open(&path)
                .unwrap();
            let fd = Target::Fd(file.as_raw_fd());
            ring.prep_write(1, fd, b"hello io_uring".to_vec(), 0)
                .unwrap();
            ring.prep_fsync(2, fd).unwrap();
            let done = ring.submit_and_wait(2, SECS, 8).unwrap();
            let write = done.iter().find(|c| c.token == 1).unwrap();
            assert_eq!(write.result, 14);
            ring.prep_read(3, fd, 64, 0).unwrap();
            let done = ring.submit_and_wait(1, SECS, 8).unwrap();
            assert_eq!(done[0].result, 14);
            assert_eq!(done[0].data, b"hello io_uring");
            // read at an offset
            ring.prep_read(4, fd, 4, 6).unwrap();
            let done = ring.submit_and_wait(1, SECS, 8).unwrap();
            assert_eq!(done[0].data, b"io_u");
            // EOF returns 0 bytes and no data
            ring.prep_read(5, fd, 8, 100).unwrap();
            let done = ring.submit_and_wait(1, SECS, 8).unwrap();
            assert_eq!(done[0].result, 0);
            assert!(done[0].data.is_empty());
        }
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn vectored_io_gathers_and_scatters() {
        let path = temp_path("v");
        for mut ring in backends() {
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(true)
                .open(&path)
                .unwrap();
            let fd = Target::Fd(file.as_raw_fd());
            ring.prep_writev(
                1,
                fd,
                vec![b"abc".to_vec(), b"def".to_vec(), b"gh".to_vec()],
                0,
            )
            .unwrap();
            let done = ring.submit_and_wait(1, SECS, 4).unwrap();
            assert_eq!(done[0].result, 8);
            let mut back = vec![0u8; 8];
            file.read_exact_at(&mut back, 0).unwrap();
            assert_eq!(back, b"abcdefgh");
            ring.prep_readv(2, fd, &[2, 3, 3], 0).unwrap();
            let done = ring.submit_and_wait(1, SECS, 4).unwrap();
            assert_eq!(done[0].result, 8);
            assert_eq!(done[0].data, b"abcdefgh");
            // short vectored read concatenates only what arrived
            ring.prep_readv(3, fd, &[4, 4, 4], 4).unwrap();
            let done = ring.submit_and_wait(1, SECS, 4).unwrap();
            assert_eq!(done[0].data, b"efgh");
        }
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn accept_recv_send_over_loopback() {
        for mut ring in backends() {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            ring.prep_accept(10, Target::Fd(listener.as_raw_fd()))
                .unwrap();
            ring.submit().unwrap();
            let mut client = TcpStream::connect(addr).unwrap();
            let mut done = ring.submit_and_wait(1, SECS, 4).unwrap();
            let accepted = done.pop().unwrap();
            assert_eq!(accepted.token, 10);
            let conn = accepted.accepted.expect("accept returns an owned fd");
            assert_eq!(accepted.result, i64::from(conn.as_raw_fd()));

            // recv armed before the data exists: must stay in flight, then complete.
            ring.prep_recv(11, Target::Fd(conn.as_raw_fd()), 64)
                .unwrap();
            ring.submit().unwrap();
            assert!(ring.poll(4).unwrap().is_empty());
            client.write_all(b"ping").unwrap();
            let done = ring.submit_and_wait(1, SECS, 4).unwrap();
            assert_eq!(done[0].data, b"ping");

            ring.prep_send(12, Target::Fd(conn.as_raw_fd()), b"pong".to_vec())
                .unwrap();
            let done = ring.submit_and_wait(1, SECS, 4).unwrap();
            assert_eq!(done[0].result, 4);
            let mut got = [0u8; 4];
            client.read_exact(&mut got).unwrap();
            assert_eq!(&got, b"pong");

            // peer close shows up as a zero-byte recv
            ring.prep_recv(13, Target::Fd(conn.as_raw_fd()), 8).unwrap();
            drop(client);
            let done = ring.submit_and_wait(1, SECS, 4).unwrap();
            assert_eq!(done[0].result, 0);
        }
    }

    #[test]
    fn poll_add_reports_readiness() {
        for mut ring in backends() {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            ring.prep_poll_add(7, Target::Fd(listener.as_raw_fd()), libc::EPOLLIN as u32)
                .unwrap();
            ring.submit().unwrap();
            assert!(ring.poll(4).unwrap().is_empty());
            let _client = TcpStream::connect(addr).unwrap();
            let done = ring.submit_and_wait(1, SECS, 4).unwrap();
            assert_eq!(done[0].token, 7);
            assert!(done[0].is_poll);
            assert_ne!(done[0].result & i64::from(libc::EPOLLIN), 0);
        }
    }

    #[test]
    fn timeout_fires_with_etime_and_wait_times_out() {
        for mut ring in backends() {
            ring.prep_timeout(5, Duration::from_millis(30)).unwrap();
            let start = Instant::now();
            let done = ring.submit_and_wait(1, SECS, 4).unwrap();
            assert_eq!(done[0].result, ETIME_RESULT, "{:?}", ring.backend());
            assert!(start.elapsed() >= Duration::from_millis(25));

            // A wait with nothing in flight returns immediately, empty.
            let start = Instant::now();
            assert!(ring.submit_and_wait(1, SECS, 4).unwrap().is_empty());
            assert!(start.elapsed() < Duration::from_secs(1));

            // A bounded wait on something that never completes returns when time is up.
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            ring.prep_accept(6, Target::Fd(listener.as_raw_fd()))
                .unwrap();
            let start = Instant::now();
            let done = ring
                .submit_and_wait(1, Some(Duration::from_millis(40)), 4)
                .unwrap();
            assert!(done.is_empty());
            assert!(start.elapsed() >= Duration::from_millis(35));
            assert_eq!(ring.inflight(), 1);
        }
    }

    #[test]
    fn fixed_buffers_and_files() {
        let path = temp_path("fixed");
        for mut ring in backends() {
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(true)
                .open(&path)
                .unwrap();
            ring.register_buffers(2, 4096).unwrap();
            ring.register_files(&[file.as_raw_fd()]).unwrap();
            assert_eq!(ring.fixed_buffer_count(), 2);
            assert_eq!(ring.fixed_file_count(), 1);
            let fd = Target::Fixed(0);
            ring.prep_write_fixed(1, fd, 0, b"fixed payload".to_vec(), 0)
                .unwrap();
            // buffer 0 is busy until the write completes
            assert!(ring.prep_write_fixed(2, fd, 0, b"x".to_vec(), 0).is_err());
            let done = ring.submit_and_wait(1, SECS, 4).unwrap();
            assert_eq!(done[0].result, 13);
            ring.prep_read_fixed(3, fd, 1, 13, 0).unwrap();
            let done = ring.submit_and_wait(1, SECS, 4).unwrap();
            assert_eq!(done[0].data, b"fixed payload");
            // plain fd and fixed index agree on the same file
            ring.prep_read(4, Target::Fd(file.as_raw_fd()), 5, 0)
                .unwrap();
            let done = ring.submit_and_wait(1, SECS, 4).unwrap();
            assert_eq!(done[0].data, b"fixed");
            // out-of-table index is rejected up front
            assert!(ring.prep_read(5, Target::Fixed(3), 1, 0).is_err());
            assert!(ring.register_buffers(1, 64).is_err());
            ring.unregister_buffers().unwrap();
            ring.unregister_files().unwrap();
            assert_eq!(ring.fixed_buffer_count(), 0);
            assert_eq!(ring.fixed_file_count(), 0);
        }
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn invalid_requests_are_rejected_not_submitted() {
        for mut ring in backends() {
            let t = Target::Fd(0);
            assert!(ring.prep_read(1, t, 0, 0).is_err());
            assert!(ring.prep_readv(1, t, &[], 0).is_err());
            assert!(ring.prep_read_fixed(1, t, 0, 8, 0).is_err());
            assert!(ring.register_buffers(0, 16).is_err());
            assert!(ring.register_files(&[]).is_err());
            assert_eq!(ring.inflight(), 0);
            assert_eq!(ring.queued(), 0);
        }
    }

    #[test]
    fn bad_descriptor_surfaces_as_negative_errno() {
        for mut ring in backends() {
            ring.prep_read(1, Target::Fd(987_654), 8, 0).unwrap();
            let done = ring.submit_and_wait(1, SECS, 4).unwrap();
            assert_eq!(
                done[0].result,
                -i64::from(libc::EBADF),
                "{:?}",
                ring.backend()
            );
        }
    }

    #[test]
    fn inflight_is_bounded_by_the_completion_queue() {
        let mut ring = Ring::open(Options {
            entries: 4,
            ..Options::default()
        })
        .unwrap();
        let mut accepted = 0;
        while ring.prep_nop(accepted).is_ok() {
            accepted += 1;
            assert!(accepted < 1000);
        }
        assert!(accepted >= 4);
        let done = ring.submit_and_wait(accepted as usize, SECS, 64).unwrap();
        assert_eq!(done.len(), accepted as usize);
        ring.prep_nop(99).unwrap();
    }

    #[test]
    fn sq_overflow_flushes_transparently() {
        let mut ring = Ring::open(Options {
            entries: 4,
            backend: Preference::Native,
            ..Options::default()
        });
        let Ok(ring) = ring.as_mut() else {
            return;
        };
        // 8 CQ slots, 4 SQ slots: staging 6 NOPs forces an intermediate flush.
        for t in 0..6 {
            ring.prep_nop(t).unwrap();
        }
        let done = ring.submit_and_wait(6, SECS, 16).unwrap();
        assert_eq!(done.len(), 6);
    }

    #[test]
    fn sqpoll_is_optional_and_functional() {
        let ring = Ring::open(Options {
            sqpoll: true,
            backend: Preference::Auto,
            ..Options::default()
        });
        let mut ring = ring.unwrap();
        for t in 0..8 {
            ring.prep_nop(t).unwrap();
        }
        let done = ring.submit_and_wait(8, SECS, 16).unwrap();
        assert_eq!(done.len(), 8);
        // Whether SQPOLL was granted depends on privileges; the API works either way.
        eprintln!(
            "backend={:?} sqpoll={}",
            ring.backend(),
            ring.sqpoll_active()
        );
    }

    #[test]
    fn dropping_a_ring_with_inflight_ops_is_clean() {
        for mut ring in backends() {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            ring.prep_accept(1, Target::Fd(listener.as_raw_fd()))
                .unwrap();
            ring.prep_timeout(2, Duration::from_secs(60)).unwrap();
            ring.submit().unwrap();
            drop(ring);
        }
    }

    #[test]
    fn unavailable_errors_trigger_fallback_classification() {
        for code in [libc::ENOSYS, libc::EPERM, libc::EACCES, libc::EINVAL] {
            assert!(is_unavailable(&os_err(code)));
        }
        assert!(!is_unavailable(&os_err(libc::EMFILE)));
    }
}
