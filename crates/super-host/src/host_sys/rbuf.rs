//! Fixed-capacity circular byte buffer behind `std.io.buf`.
//!
//! The readable bytes and the free space are each exposed as at most two contiguous
//! segments, which is exactly the shape `readv` / `writev` want: a reader scatters one
//! `readv` over the free segments, a writer gathers one `writev` from the readable ones.

#[derive(Debug)]
pub struct RingBuf {
    data: Vec<u8>,
    head: usize,
    len: usize,
}

impl RingBuf {
    pub fn new(capacity: usize) -> Self {
        Self {
            data: vec![0; capacity.max(1)],
            head: 0,
            len: 0,
        }
    }

    pub fn capacity(&self) -> usize {
        self.data.len()
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn free(&self) -> usize {
        self.capacity() - self.len
    }

    pub fn clear(&mut self) {
        self.head = 0;
        self.len = 0;
    }

    /// Append as much of `src` as fits; returns the number of bytes accepted.
    pub fn put(&mut self, src: &[u8]) -> usize {
        let n = src.len().min(self.free());
        let cap = self.capacity();
        let tail = (self.head + self.len) % cap;
        let first = n.min(cap - tail);
        self.data[tail..tail + first].copy_from_slice(&src[..first]);
        self.data[..n - first].copy_from_slice(&src[first..n]);
        self.len += n;
        n
    }

    /// The readable region as `(first, second)`; `second` is empty unless it wraps.
    pub fn readable(&self) -> (&[u8], &[u8]) {
        let first = self.len.min(self.capacity() - self.head);
        (
            &self.data[self.head..self.head + first],
            &self.data[..self.len - first],
        )
    }

    /// Copy out up to `n` bytes without consuming them.
    pub fn peek(&self, n: usize) -> Vec<u8> {
        let n = n.min(self.len);
        let (a, b) = self.readable();
        let mut out = Vec::with_capacity(n);
        out.extend_from_slice(&a[..n.min(a.len())]);
        out.extend_from_slice(&b[..n.saturating_sub(a.len())]);
        out
    }

    /// Drop up to `n` readable bytes; returns how many were dropped.
    pub fn consume(&mut self, n: usize) -> usize {
        let n = n.min(self.len);
        self.head = (self.head + n) % self.capacity();
        self.len -= n;
        if self.len == 0 {
            self.head = 0;
        }
        n
    }

    /// Remove and return up to `n` bytes.
    pub fn take(&mut self, n: usize) -> Vec<u8> {
        let out = self.peek(n);
        self.consume(out.len());
        out
    }

    /// Offset (from the read cursor) of the first `byte`, if any.
    pub fn find(&self, byte: u8) -> Option<usize> {
        let (a, b) = self.readable();
        a.iter()
            .position(|&x| x == byte)
            .or_else(|| b.iter().position(|&x| x == byte).map(|i| i + a.len()))
    }

    /// Lengths of the free region as `(first, second)` contiguous segments.
    pub fn writable_lens(&self) -> (usize, usize) {
        let cap = self.capacity();
        let tail = (self.head + self.len) % cap;
        let free = self.free();
        let first = free.min(cap - tail);
        (first, free - first)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn put_take_preserves_order_across_the_wrap() {
        let mut rb = RingBuf::new(8);
        assert_eq!(rb.put(b"abcdef"), 6);
        assert_eq!(rb.take(4), b"abcd");
        assert_eq!(rb.put(b"ghijkl"), 6);
        assert_eq!(rb.len(), 8);
        assert_eq!(rb.free(), 0);
        let (a, b) = rb.readable();
        assert_eq!([a, b].concat(), b"efghijkl");
        assert!(!b.is_empty(), "data must wrap in this scenario");
        assert_eq!(rb.take(100), b"efghijkl");
        assert!(rb.is_empty());
    }

    #[test]
    fn put_is_partial_when_full() {
        let mut rb = RingBuf::new(4);
        assert_eq!(rb.put(b"abcdefgh"), 4);
        assert_eq!(rb.put(b"x"), 0);
        assert_eq!(rb.peek(10), b"abcd");
        assert_eq!(rb.len(), 4, "peek does not consume");
    }

    #[test]
    fn writable_segments_follow_the_read_cursor() {
        let mut rb = RingBuf::new(8);
        assert_eq!(rb.writable_lens(), (8, 0));
        rb.put(b"123456");
        rb.consume(4);
        // tail at 6, head at 4: free is [6..8) then [0..4)
        assert_eq!(rb.writable_lens(), (2, 4));
        rb.put(b"ab");
        assert_eq!(rb.writable_lens(), (4, 0));
    }

    #[test]
    fn consume_and_clear_reset_state() {
        let mut rb = RingBuf::new(4);
        rb.put(b"abc");
        assert_eq!(rb.consume(10), 3);
        assert_eq!(rb.writable_lens(), (4, 0));
        rb.put(b"zz");
        rb.clear();
        assert!(rb.is_empty());
        assert_eq!(rb.free(), 4);
    }

    #[test]
    fn find_searches_across_the_wrap() {
        let mut rb = RingBuf::new(6);
        rb.put(b"abcd");
        rb.consume(3);
        rb.put(b"ef\ngh");
        assert_eq!(rb.find(b'\n'), Some(3));
        assert_eq!(rb.find(b'd'), Some(0));
        assert_eq!(rb.find(b'z'), None);
    }

    #[test]
    fn zero_capacity_is_clamped() {
        let mut rb = RingBuf::new(0);
        assert_eq!(rb.capacity(), 1);
        assert_eq!(rb.put(b"ab"), 1);
    }

    #[test]
    fn long_random_walk_matches_a_model() {
        let mut rb = RingBuf::new(13);
        let mut model: std::collections::VecDeque<u8> = Default::default();
        let mut seed = 0x2545_f491_u32;
        for step in 0..2000u32 {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let n = (seed >> 24) as usize % 9;
            if seed & 0x100 == 0 {
                let chunk: Vec<u8> = (0..n).map(|k| (step as u8).wrapping_add(k as u8)).collect();
                let took = rb.put(&chunk);
                model.extend(&chunk[..took]);
            } else {
                let got = rb.take(n);
                let want: Vec<u8> = (0..got.len()).filter_map(|_| model.pop_front()).collect();
                assert_eq!(got, want);
            }
            assert_eq!(rb.len(), model.len());
        }
    }
}
