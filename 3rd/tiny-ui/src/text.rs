//! Fixed-capacity UTF-8 label storage.
/// UTF-8 inline text; truncates on a character boundary at 96 bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InlineText {
    bytes: [u8; 96],
    len: usize,
}
impl InlineText {
    pub fn new(text: &str) -> Self {
        let mut n = text.len().min(96);
        while !text.is_char_boundary(n) {
            n -= 1;
        }
        let mut out = Self {
            bytes: [0; 96],
            len: n,
        };
        out.bytes[..n].copy_from_slice(&text.as_bytes()[..n]);
        out
    }
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("")
    }
}
impl core::fmt::Write for InlineText {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        if self.len + s.len() > 96 {
            return Err(core::fmt::Error);
        }
        self.bytes[self.len..self.len + s.len()].copy_from_slice(s.as_bytes());
        self.len += s.len();
        Ok(())
    }
}
