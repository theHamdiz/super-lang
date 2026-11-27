//! Source locations for diagnostics, LSP, and AST nodes.

use core::fmt;

/// Absolute byte offset into a source file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BytePos(pub u32);

impl BytePos {
    #[inline]
    pub const fn new(offset: u32) -> Self {
        Self(offset)
    }

    #[inline]
    pub const fn as_usize(self) -> usize {
        self.0 as usize
    }
}

/// Half-open byte range `[lo, hi)` within a source file.
///
/// `file` distinguishes modules that are parsed separately and then merged.
/// Byte offsets alone collide across files, and expression types are keyed by
/// span, so a dependency and the entry file must not share a key.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Span {
    pub lo: BytePos,
    pub hi: BytePos,
    /// Module index assigned at parse time (`0` is the entry file) in the low
    /// 16 bits; the high 16 bits hold a macro-expansion mark (see
    /// [`Span::with_expansion_mark`]).
    pub file: u32,
}

const MARK_SHIFT: u32 = 16;
const FILE_MASK: u32 = (1 << MARK_SHIFT) - 1;

impl Span {
    #[inline]
    pub const fn new(lo: BytePos, hi: BytePos) -> Self {
        Self { lo, hi, file: 0 }
    }

    #[inline]
    pub const fn from_offsets(lo: u32, hi: u32) -> Self {
        Self {
            lo: BytePos(lo),
            hi: BytePos(hi),
            file: 0,
        }
    }

    #[inline]
    pub const fn empty(at: BytePos) -> Self {
        Self {
            lo: at,
            hi: at,
            file: 0,
        }
    }

    #[inline]
    pub const fn with_file(mut self, file: u32) -> Self {
        self.file = file;
        self
    }

    /// Module index without the expansion mark.
    #[inline]
    pub const fn file_index(self) -> u32 {
        self.file & FILE_MASK
    }

    /// Non-zero for tokens a macro template introduced.
    #[inline]
    pub const fn expansion_mark(self) -> u16 {
        (self.file >> MARK_SHIFT) as u16
    }

    /// Tag a span with a macro-expansion mark. Expansion tokens reuse the call
    /// site's byte range for diagnostics; the mark keeps them distinct from each
    /// other wherever spans act as keys (expression types).
    #[inline]
    pub const fn with_expansion_mark(mut self, mark: u16) -> Self {
        self.file = (self.file & FILE_MASK) | ((mark as u32) << MARK_SHIFT);
        self
    }

    #[inline]
    pub fn merge(self, other: Span) -> Span {
        let (a, b) = (self.expansion_mark(), other.expansion_mark());
        let file = if a == b {
            self.file
        } else {
            let mixed = (a.wrapping_mul(0x9E37) ^ b.wrapping_mul(0x85EB).rotate_left(5)) | 1;
            self.with_expansion_mark(mixed).file
        };
        Span {
            lo: if self.lo < other.lo {
                self.lo
            } else {
                other.lo
            },
            hi: if self.hi > other.hi {
                self.hi
            } else {
                other.hi
            },
            file,
        }
    }

    #[inline]
    pub const fn len(self) -> u32 {
        self.hi.0.saturating_sub(self.lo.0)
    }

    #[inline]
    pub const fn is_empty(self) -> bool {
        self.len() == 0
    }

    #[inline]
    pub fn slice(self, source: &str) -> &str {
        let lo = self.lo.as_usize().min(source.len());
        let hi = self.hi.as_usize().min(source.len());
        &source[lo..hi]
    }
}

impl fmt::Display for Span {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}..{}", self.lo.0, self.hi.0)
    }
}

/// Owned source text with path metadata for LSP / diagnostics.
#[derive(Clone, Debug)]
pub struct SourceFile {
    pub path: String,
    pub text: String,
    line_starts: Vec<u32>,
}

impl SourceFile {
    pub fn new(path: impl Into<String>, text: impl Into<String>) -> Self {
        let text = text.into();
        let line_starts = compute_line_starts(&text);
        Self {
            path: path.into(),
            text,
            line_starts,
        }
    }

    pub fn line_col(&self, pos: BytePos) -> (u32, u32) {
        let offset = pos.0;
        let line = match self.line_starts.binary_search(&offset) {
            Ok(i) => i,
            Err(i) => i.saturating_sub(1),
        };
        let line_start = self.line_starts[line];
        let col = offset.saturating_sub(line_start);
        (line as u32 + 1, col + 1)
    }

    pub fn span_line_col(&self, span: Span) -> ((u32, u32), (u32, u32)) {
        (self.line_col(span.lo), self.line_col(span.hi))
    }

    /// 0-based LSP `(line, character)` using UTF-16 code units (LSP default).
    pub fn lsp_position(&self, pos: BytePos) -> (u32, u32) {
        let offset = pos.0.min(self.text.len() as u32);
        let line = match self.line_starts.binary_search(&offset) {
            Ok(i) => i,
            Err(i) => i.saturating_sub(1),
        };
        let line_start = self.line_starts[line] as usize;
        let line_end = self
            .line_starts
            .get(line + 1)
            .map(|o| *o as usize)
            .unwrap_or(self.text.len());
        let content_end = line_content_end(&self.text, line_start, line_end);
        let within = (offset as usize)
            .saturating_sub(line_start)
            .min(content_end.saturating_sub(line_start));
        let character = utf16_len(&self.text[line_start..line_start + within]);
        (line as u32, character)
    }

    pub fn lsp_span_range(&self, span: Span) -> ((u32, u32), (u32, u32)) {
        (self.lsp_position(span.lo), self.lsp_position(span.hi))
    }

    /// 1-based line text (without requiring the caller to re-split the file).
    pub fn line_text(&self, line_1based: u32) -> Option<&str> {
        let idx = line_1based.saturating_sub(1) as usize;
        if idx >= self.line_starts.len() {
            return None;
        }
        let start = self.line_starts[idx] as usize;
        let end = self
            .line_starts
            .get(idx + 1)
            .map(|o| *o as usize)
            .unwrap_or(self.text.len());
        Some(&self.text[start.min(self.text.len())..end.min(self.text.len())])
    }

    /// Byte offset of the start of a 1-based line.
    pub fn line_start_offset(&self, line_1based: u32) -> BytePos {
        let idx = line_1based.saturating_sub(1) as usize;
        let start = self
            .line_starts
            .get(idx)
            .copied()
            .unwrap_or(self.text.len() as u32);
        BytePos(start)
    }

    /// Convert 0-based LSP `(line, character)` (UTF-16) to a byte offset.
    ///
    /// `character` may equal the line length (position after the last character),
    /// which is required for incremental `textDocument/didChange` ranges.
    pub fn offset_at_lsp(&self, line_0based: u32, character_0based: u32) -> BytePos {
        let idx = line_0based as usize;
        let start = self
            .line_starts
            .get(idx)
            .copied()
            .unwrap_or(self.text.len() as u32) as usize;
        let line_end = self
            .line_starts
            .get(idx + 1)
            .map(|o| *o as usize)
            .unwrap_or(self.text.len());
        let content_end = line_content_end(&self.text, start, line_end);
        let line = &self.text[start..content_end];
        let byte_col = utf16_to_byte_index(line, character_0based);
        BytePos((start + byte_col) as u32)
    }
}

fn compute_line_starts(text: &str) -> Vec<u32> {
    let mut starts = vec![0u32];
    for (i, b) in text.bytes().enumerate() {
        if b == b'\n' {
            starts.push((i + 1) as u32);
        }
    }
    starts
}

fn line_content_end(text: &str, start: usize, line_end: usize) -> usize {
    let mut end = line_end.min(text.len());
    if end > start && text.as_bytes()[end - 1] == b'\n' {
        end -= 1;
        if end > start && text.as_bytes()[end - 1] == b'\r' {
            end -= 1;
        }
    }
    end
}

fn utf16_len(text: &str) -> u32 {
    text.encode_utf16().count() as u32
}

fn utf16_to_byte_index(text: &str, utf16_col: u32) -> usize {
    if utf16_col == 0 {
        return 0;
    }
    let mut seen = 0u32;
    for (byte_idx, ch) in text.char_indices() {
        if seen >= utf16_col {
            return byte_idx;
        }
        seen += ch.len_utf16() as u32;
    }
    text.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_col_accounts_for_newlines() {
        let src = SourceFile::new("t.sr", "a\nbc\ndef");
        assert_eq!(src.line_col(BytePos(0)), (1, 1));
        assert_eq!(src.line_col(BytePos(2)), (2, 1));
        assert_eq!(src.line_col(BytePos(5)), (3, 1));
    }

    #[test]
    fn span_slice() {
        let text = "hello world";
        let span = Span::from_offsets(6, 11);
        assert_eq!(span.slice(text), "world");
    }

    #[test]
    fn offset_at_lsp_allows_end_of_line_and_utf16() {
        let src = SourceFile::new("t.sr", "ab🙂c\nxy");
        // After 'b' (UTF-16 index 2) before the emoji.
        assert_eq!(src.offset_at_lsp(0, 2), BytePos(2));
        // After emoji: 'a','b' = 2, emoji = 2 → character 4 points at 'c'.
        assert_eq!(src.offset_at_lsp(0, 4), BytePos(6));
        // End of first line (after 'c') — valid incremental range end.
        assert_eq!(src.offset_at_lsp(0, 5), BytePos(7));
        assert_eq!(src.lsp_position(BytePos(2)), (0, 2));
        assert_eq!(src.lsp_position(BytePos(6)), (0, 4));
    }

    #[test]
    fn expansion_mark_keeps_file_index_and_range() {
        let span = Span::from_offsets(3, 9)
            .with_file(7)
            .with_expansion_mark(42);
        assert_eq!(span.file_index(), 7);
        assert_eq!(span.expansion_mark(), 42);
        assert_eq!((span.lo.0, span.hi.0), (3, 9));
        assert_eq!(span.with_expansion_mark(0).file, 7);
    }

    #[test]
    fn merging_differently_marked_spans_yields_a_new_mark() {
        let call = Span::from_offsets(0, 10);
        let a = call.with_expansion_mark(1);
        let b = call.with_expansion_mark(2);
        let merged = a.merge(b);
        assert_eq!((merged.lo.0, merged.hi.0), (0, 10));
        assert_ne!(merged.expansion_mark(), 0);
        assert_ne!(merged, a);
        assert_ne!(merged, b);
        assert_eq!(a.merge(a), a);
        let plain = Span::from_offsets(2, 4).with_file(3);
        assert_eq!(plain.merge(Span::from_offsets(5, 8).with_file(3)).file, 3);
    }
}
