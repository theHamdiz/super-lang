//! Platform readiness abstraction used by the host networking surface.
//!
//! Linux uses epoll, macOS and FreeBSD use kqueue, and Windows uses WSAPoll.
//! The public event bits intentionally match the long-standing Super readiness
//! ABI (`READ=1`, `WRITE=4`, `ERROR=8`, `HANGUP=16`).

use std::collections::HashMap;
use std::io;

pub const CTL_ADD: i32 = 1;
pub const CTL_DEL: i32 = 2;
pub const CTL_MOD: i32 = 3;
pub const READ: u32 = 0x001;
pub const WRITE: u32 = 0x004;
pub const ERROR: u32 = 0x008;
pub const HANGUP: u32 = 0x010;
pub const EDGE: u32 = 1 << 31;

#[derive(Clone, Copy)]
struct Interest {
    #[cfg_attr(not(windows), allow(dead_code))]
    events: u32,
    #[cfg_attr(target_os = "linux", allow(dead_code))]
    token: u64,
}

pub struct Poller {
    interests: HashMap<i64, Interest>,
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "freebsd"))]
    fd: std::os::fd::OwnedFd,
}

impl Poller {
    pub fn new() -> io::Result<Self> {
        #[cfg(target_os = "linux")]
        let fd = {
            use std::os::fd::FromRawFd;
            // SAFETY: the successful result is a newly owned descriptor.
            let raw = unsafe { libc::epoll_create1(libc::EPOLL_CLOEXEC) };
            if raw < 0 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: ownership of `raw` transfers into OwnedFd exactly once.
            unsafe { std::os::fd::OwnedFd::from_raw_fd(raw) }
        };
        #[cfg(any(target_os = "macos", target_os = "freebsd"))]
        let fd = {
            use std::os::fd::FromRawFd;
            // SAFETY: kqueue takes no pointers and returns a new descriptor.
            let raw = unsafe { libc::kqueue() };
            if raw < 0 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: ownership of `raw` transfers into OwnedFd exactly once.
            unsafe { std::os::fd::OwnedFd::from_raw_fd(raw) }
        };
        Ok(Self {
            interests: HashMap::new(),
            #[cfg(any(target_os = "linux", target_os = "macos", target_os = "freebsd"))]
            fd,
        })
    }

    pub fn ctl(&mut self, operation: i32, socket: i64, events: u32, token: u64) -> io::Result<()> {
        if !matches!(operation, CTL_ADD | CTL_DEL | CTL_MOD) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid readiness operation",
            ));
        }
        let events = events & (READ | WRITE | ERROR | HANGUP | EDGE);
        #[cfg(target_os = "linux")]
        self.ctl_epoll(operation, socket, events, token)?;
        #[cfg(any(target_os = "macos", target_os = "freebsd"))]
        self.ctl_kqueue(operation, socket, events)?;
        match operation {
            CTL_ADD | CTL_MOD => {
                self.interests.insert(socket, Interest { events, token });
            }
            CTL_DEL => {
                self.interests.remove(&socket);
            }
            _ => unreachable!(),
        }
        Ok(())
    }

    pub fn wait(&self, max_events: usize, timeout_ms: i32) -> io::Result<Vec<(u64, u32)>> {
        #[cfg(target_os = "linux")]
        return self.wait_epoll(max_events, timeout_ms);
        #[cfg(any(target_os = "macos", target_os = "freebsd"))]
        return self.wait_kqueue(max_events, timeout_ms);
        #[cfg(windows)]
        return self.wait_windows(max_events, timeout_ms);
        #[cfg(not(any(
            target_os = "linux",
            target_os = "macos",
            target_os = "freebsd",
            windows
        )))]
        {
            let _ = (max_events, timeout_ms);
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "no readiness backend for this platform",
            ))
        }
    }

    #[cfg(target_os = "linux")]
    fn ctl_epoll(&self, operation: i32, socket: i64, events: u32, token: u64) -> io::Result<()> {
        use std::os::fd::AsRawFd;
        let mut event = libc::epoll_event { events, u64: token };
        let event_ptr = if operation == CTL_DEL {
            std::ptr::null_mut()
        } else {
            &mut event
        };
        // SAFETY: the descriptors are values and event_ptr is null for delete
        // or points to a live, initialized event for the duration of the call.
        let result = unsafe {
            libc::epoll_ctl(
                self.fd.as_raw_fd(),
                operation,
                socket as libc::c_int,
                event_ptr,
            )
        };
        if result == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    #[cfg(target_os = "linux")]
    fn wait_epoll(&self, max_events: usize, timeout_ms: i32) -> io::Result<Vec<(u64, u32)>> {
        use std::os::fd::AsRawFd;
        let mut events = vec![libc::epoll_event { events: 0, u64: 0 }; max_events.clamp(1, 4096)];
        // SAFETY: `events` owns enough initialized slots for the requested count.
        let count = unsafe {
            libc::epoll_wait(
                self.fd.as_raw_fd(),
                events.as_mut_ptr(),
                events.len() as libc::c_int,
                timeout_ms,
            )
        };
        if count < 0 {
            return Err(io::Error::last_os_error());
        }
        events.truncate(count as usize);
        Ok(events
            .into_iter()
            .map(|event| (event.u64, event.events))
            .collect())
    }

    #[cfg(any(target_os = "macos", target_os = "freebsd"))]
    fn ctl_kqueue(&self, operation: i32, socket: i64, events: u32) -> io::Result<()> {
        use std::os::fd::AsRawFd;
        let previous = self
            .interests
            .get(&socket)
            .map_or(0, |interest| interest.events);
        let mut changes = Vec::with_capacity(2);
        for (filter, bit) in [(libc::EVFILT_READ, READ), (libc::EVFILT_WRITE, WRITE)] {
            let enabled = events & bit != 0 && operation != CTL_DEL;
            let was_enabled = previous & bit != 0;
            let flags = match (enabled, was_enabled, operation) {
                (true, _, _) => {
                    libc::EV_ADD
                        | libc::EV_ENABLE
                        | if events & EDGE != 0 {
                            libc::EV_CLEAR
                        } else {
                            0
                        }
                }
                (false, true, CTL_MOD | CTL_DEL) => libc::EV_DELETE,
                _ => continue,
            };
            // SAFETY: zero is a valid baseline for every supported kevent ABI;
            // the common fields are populated before the value reaches libc.
            let mut change: libc::kevent = unsafe { std::mem::zeroed() };
            change.ident = socket as libc::uintptr_t;
            change.filter = filter;
            change.flags = flags;
            changes.push(change);
        }
        if changes.is_empty() {
            return Ok(());
        }
        // SAFETY: both slices remain live for the call; no output buffer is used.
        let result = unsafe {
            libc::kevent(
                self.fd.as_raw_fd(),
                changes.as_ptr(),
                changes.len() as libc::c_int,
                std::ptr::null_mut(),
                0,
                std::ptr::null(),
            )
        };
        if result >= 0 || operation == CTL_DEL {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    #[cfg(any(target_os = "macos", target_os = "freebsd"))]
    fn wait_kqueue(&self, max_events: usize, timeout_ms: i32) -> io::Result<Vec<(u64, u32)>> {
        use std::os::fd::AsRawFd;
        let mut events: Vec<libc::kevent> =
            vec![unsafe { std::mem::zeroed() }; max_events.clamp(1, 4096)];
        let timeout = (timeout_ms >= 0).then_some(libc::timespec {
            tv_sec: (timeout_ms / 1000).into(),
            tv_nsec: i64::from(timeout_ms % 1000) * 1_000_000,
        });
        // SAFETY: the output vector has the advertised capacity and timeout, when
        // present, remains alive and immutable throughout the call.
        let count = unsafe {
            libc::kevent(
                self.fd.as_raw_fd(),
                std::ptr::null(),
                0,
                events.as_mut_ptr(),
                events.len() as libc::c_int,
                timeout
                    .as_ref()
                    .map_or(std::ptr::null(), std::ptr::from_ref),
            )
        };
        if count < 0 {
            return Err(io::Error::last_os_error());
        }
        let mut merged: HashMap<u64, u32> = HashMap::new();
        for event in events.into_iter().take(count as usize) {
            let socket = event.ident as i64;
            let Some(interest) = self.interests.get(&socket) else {
                continue;
            };
            let bits = merged.entry(interest.token).or_default();
            if event.filter == libc::EVFILT_READ {
                *bits |= READ;
            }
            if event.filter == libc::EVFILT_WRITE {
                *bits |= WRITE;
            }
            if event.flags & libc::EV_ERROR != 0 {
                *bits |= ERROR;
            }
            if event.flags & libc::EV_EOF != 0 {
                *bits |= HANGUP;
            }
        }
        Ok(merged.into_iter().collect())
    }

    #[cfg(windows)]
    fn wait_windows(&self, max_events: usize, timeout_ms: i32) -> io::Result<Vec<(u64, u32)>> {
        #[repr(C)]
        struct PollFd {
            fd: usize,
            events: i16,
            revents: i16,
        }
        #[link(name = "ws2_32")]
        unsafe extern "system" {
            fn WSAPoll(fds: *mut PollFd, count: u32, timeout: i32) -> i32;
            fn WSAGetLastError() -> i32;
        }
        const READ_READY: i16 = 0x0100;
        const WRITE_READY: i16 = 0x0010;
        const ERROR_READY: i16 = 0x0001 | 0x0002 | 0x0004 | 0x0008 | 0x0010;
        let selected: Vec<_> = self.interests.iter().take(max_events.max(1)).collect();
        let mut fds: Vec<_> = selected
            .iter()
            .map(|(socket, interest)| PollFd {
                fd: **socket as usize,
                events: (if interest.events & READ != 0 {
                    READ_READY
                } else {
                    0
                }) | (if interest.events & WRITE != 0 {
                    WRITE_READY
                } else {
                    0
                }),
                revents: 0,
            })
            .collect();
        // SAFETY: `fds` is contiguous and writable for exactly `count` entries.
        let count = unsafe { WSAPoll(fds.as_mut_ptr(), fds.len() as u32, timeout_ms) };
        if count < 0 {
            // SAFETY: WSAGetLastError has no preconditions.
            return Err(io::Error::from_raw_os_error(unsafe { WSAGetLastError() }));
        }
        let mut ready = Vec::with_capacity(count as usize);
        for ((_, interest), fd) in selected.into_iter().zip(fds) {
            if fd.revents == 0 {
                continue;
            }
            let mut bits = 0;
            if fd.revents & READ_READY != 0 {
                bits |= READ;
            }
            if fd.revents & WRITE_READY != 0 {
                bits |= WRITE;
            }
            if fd.revents & ERROR_READY != 0 {
                bits |= ERROR;
            }
            ready.push((interest.token, bits));
        }
        Ok(ready)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unknown_control_operation() {
        let mut poller = Poller::new().expect("platform poller");
        let error = poller.ctl(99, 0, READ, 1).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }
}
