//! Scoped data stacks with O(1) lexical / request / frame resets.
//!
//! Virtual-memory bump arenas via `mmap` (Unix) behind a safe RAII API.

use std::alloc::{Layout, alloc, dealloc};
use std::cell::Cell;
use std::ptr::NonNull;

/// A single bump region with mark/rewind scopes.
pub struct DataStack {
    base: NonNull<u8>,
    capacity: usize,
    cursor: Cell<usize>,
    marks: Vec<usize>,
}

// SAFETY: DataStack owns its allocation exclusively; cursor is interior-mutable
// but not shared across threads without Sync (we don't implement Sync).
unsafe impl Send for DataStack {}

impl DataStack {
    pub fn with_capacity(capacity: usize) -> Self {
        let capacity = capacity.max(4096);
        let layout = Layout::from_size_align(capacity, 16).expect("layout");
        // Prefer mmap on Unix for large arenas; fall back to allocator.
        let base = map_region(capacity).unwrap_or_else(|| {
            let ptr = unsafe { alloc(layout) };
            NonNull::new(ptr).expect("oom")
        });
        Self {
            base,
            capacity,
            cursor: Cell::new(0),
            marks: Vec::with_capacity(32),
        }
    }

    #[inline]
    pub fn mark(&mut self) {
        self.marks.push(self.cursor.get());
    }

    /// O(1) rewind to last mark (lexical scope / request / frame).
    #[inline]
    pub fn rewind(&mut self) {
        if let Some(m) = self.marks.pop() {
            self.cursor.set(m);
        }
    }

    #[inline]
    pub fn reset(&mut self) {
        self.marks.clear();
        self.cursor.set(0);
    }

    #[inline]
    pub fn used(&self) -> usize {
        self.cursor.get()
    }

    #[inline]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Allocate `size` bytes aligned to `align`, returning a relative 32-bit offset.
    pub fn alloc(&self, size: usize, align: usize) -> Option<u32> {
        let align = align.max(1);
        let cur = self.cursor.get();
        let aligned = (cur + align - 1) & !(align - 1);
        let next = aligned.checked_add(size)?;
        if next > self.capacity {
            return None;
        }
        self.cursor.set(next);
        Some(aligned as u32)
    }

    /// Destination-driven construction: reserve space and return absolute pointer.
    pub fn ret_ptr(&self, size: usize, align: usize) -> Option<*mut u8> {
        let off = self.alloc(size, align)?;
        Some(unsafe { self.base.as_ptr().add(off as usize) })
    }

    pub fn ptr_at(&self, offset: u32) -> *mut u8 {
        unsafe { self.base.as_ptr().add(offset as usize) }
    }
}

impl Drop for DataStack {
    fn drop(&mut self) {
        unmap_region(self.base, self.capacity);
    }
}

/// Pool of independently growing stacks for simultaneous collections / sum payloads.
pub struct StackPool {
    stacks: Vec<DataStack>,
    active: usize,
}

impl StackPool {
    pub fn new(bytes_per_stack: usize) -> Self {
        Self {
            stacks: vec![DataStack::with_capacity(bytes_per_stack)],
            active: 0,
        }
    }

    pub fn primary(&mut self) -> &mut DataStack {
        &mut self.stacks[self.active]
    }

    pub fn push_stack(&mut self, bytes: usize) -> usize {
        self.stacks.push(DataStack::with_capacity(bytes));
        self.stacks.len() - 1
    }

    pub fn mark_all(&mut self) {
        for s in &mut self.stacks {
            s.mark();
        }
    }

    pub fn rewind_all(&mut self) {
        for s in &mut self.stacks {
            s.rewind();
        }
    }

    pub fn reset_frame(&mut self) {
        for s in &mut self.stacks {
            s.reset();
        }
    }
}

/// Per-fiber micro data stack: non-atomic bump allocate, O(1) reset on task/scope exit.
pub struct TaskStack {
    inner: DataStack,
}

impl TaskStack {
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: DataStack::with_capacity(capacity),
        }
    }

    #[inline]
    pub fn mark(&mut self) {
        self.inner.mark();
    }

    #[inline]
    pub fn rewind(&mut self) {
        self.inner.rewind();
    }

    #[inline]
    pub fn reset(&mut self) {
        self.inner.reset();
    }

    #[inline]
    pub fn alloc(&self, size: usize, align: usize) -> Option<u32> {
        self.inner.alloc(size, align)
    }

    #[inline]
    pub fn used(&self) -> usize {
        self.inner.used()
    }

    #[inline]
    pub fn capacity(&self) -> usize {
        self.inner.capacity()
    }
}

/// Thread-local bank of virtual-memory data stacks for M:N workers.
pub struct WorkerStackBank {
    stacks: Vec<DataStack>,
    free: Vec<usize>,
}

impl WorkerStackBank {
    pub fn new(count: usize, bytes: usize) -> Self {
        let stacks: Vec<_> = (0..count.max(1))
            .map(|_| DataStack::with_capacity(bytes))
            .collect();
        let free: Vec<_> = (0..stacks.len()).collect();
        Self { stacks, free }
    }

    pub fn acquire(&mut self) -> Option<usize> {
        self.free.pop()
    }

    pub fn release(&mut self, idx: usize) {
        if let Some(s) = self.stacks.get_mut(idx) {
            s.reset();
        }
        self.free.push(idx);
    }

    pub fn get(&mut self, idx: usize) -> Option<&mut DataStack> {
        self.stacks.get_mut(idx)
    }
}

#[cfg(unix)]
fn map_region(capacity: usize) -> Option<NonNull<u8>> {
    unsafe {
        let ptr = libc_mmap(capacity)?;
        NonNull::new(ptr)
    }
}

#[cfg(unix)]
fn unmap_region(base: NonNull<u8>, capacity: usize) {
    unsafe {
        if libc_munmap(base.as_ptr(), capacity).is_err() {
            let layout = Layout::from_size_align(capacity, 16).unwrap();
            dealloc(base.as_ptr(), layout);
        }
    }
}

#[cfg(not(unix))]
fn map_region(_capacity: usize) -> Option<NonNull<u8>> {
    None
}

#[cfg(not(unix))]
fn unmap_region(base: NonNull<u8>, capacity: usize) {
    let layout = Layout::from_size_align(capacity, 16).unwrap();
    unsafe { dealloc(base.as_ptr(), layout) };
}

#[cfg(unix)]
unsafe fn libc_mmap(len: usize) -> Option<*mut u8> {
    unsafe { raw_mmap(len) }
}

#[cfg(unix)]
unsafe fn libc_munmap(ptr: *mut u8, len: usize) -> Result<(), ()> {
    unsafe { raw_munmap(ptr, len) }
}

#[cfg(unix)]
unsafe fn raw_mmap(len: usize) -> Option<*mut u8> {
    // PROT_READ|PROT_WRITE = 3, MAP_PRIVATE|MAP_ANONYMOUS = 0x22 on linux
    const PROT_READ: i32 = 1;
    const PROT_WRITE: i32 = 2;
    const MAP_PRIVATE: i32 = 0x02;
    const MAP_ANONYMOUS: i32 = 0x20;
    unsafe extern "C" {
        fn mmap(
            addr: *mut u8,
            length: usize,
            prot: i32,
            flags: i32,
            fd: i32,
            offset: i64,
        ) -> *mut u8;
    }
    let ptr = unsafe {
        mmap(
            std::ptr::null_mut(),
            len,
            PROT_READ | PROT_WRITE,
            MAP_PRIVATE | MAP_ANONYMOUS,
            -1,
            0,
        )
    };
    if ptr as isize == -1 { None } else { Some(ptr) }
}

#[cfg(unix)]
unsafe fn raw_munmap(ptr: *mut u8, len: usize) -> Result<(), ()> {
    unsafe extern "C" {
        fn munmap(addr: *mut u8, length: usize) -> i32;
    }
    if unsafe { munmap(ptr, len) } == 0 {
        Ok(())
    } else {
        Err(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mark_rewind_is_o1() {
        let mut s = DataStack::with_capacity(4096);
        let _ = s.alloc(64, 8).unwrap();
        s.mark();
        let a = s.used();
        let _ = s.alloc(128, 8).unwrap();
        assert!(s.used() > a);
        s.rewind();
        assert_eq!(s.used(), a);
    }

    #[test]
    fn ret_ptr_destination() {
        let s = DataStack::with_capacity(4096);
        let p = s.ret_ptr(16, 8).unwrap();
        assert!(!p.is_null());
    }
}
