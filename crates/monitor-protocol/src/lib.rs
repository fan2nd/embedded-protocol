//! PCMON1 messages on a newline-delimited byte stream, independent of transport or board.
//! No allocation; discard oversized lines to resynchronize at the next newline.
#![no_std]

pub const HELLO: &[u8] = b"HELLO\n";
pub const READY: &[u8] = b"PCMON1 READY\n";
pub const MAX_LINE_BYTES: usize = 128;
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Metrics {
    pub sequence: u32,
    pub cpu: u32, // tenths of a percent, 0..=1000
    pub memory: u32,
    pub rx_kib: u32,
    pub tx_kib: u32,
    pub uptime_s: u32,
}

impl Metrics {
    /// Encode one complete PCMON1 line into any caller-owned formatting buffer.
    pub fn write_line(&self, out: &mut impl core::fmt::Write) -> core::fmt::Result {
        if self.cpu > 1000 || self.memory > 1000 {
            return Err(core::fmt::Error);
        }
        writeln!(
            out,
            "PCMON1,{},{},{},{},{},{}",
            self.sequence, self.cpu, self.memory, self.rx_kib, self.tx_kib, self.uptime_s
        )
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Message {
    Hello,
    Metrics(Metrics),
}

fn number(s: &str) -> Option<u32> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

pub fn parse(line: &[u8]) -> Option<Message> {
    let s = core::str::from_utf8(line).ok()?.trim_end_matches('\r');
    if s == "HELLO" {
        return Some(Message::Hello);
    }
    let mut fields = s.split(',');
    if fields.next()? != "PCMON1" {
        return None;
    }
    let m = Metrics {
        sequence: number(fields.next()?)?,
        cpu: number(fields.next()?)?,
        memory: number(fields.next()?)?,
        rx_kib: number(fields.next()?)?,
        tx_kib: number(fields.next()?)?,
        uptime_s: number(fields.next()?)?,
    };
    if fields.next().is_some() || m.cpu > 1000 || m.memory > 1000 {
        return None;
    }
    Some(Message::Metrics(m))
}

pub struct Decoder {
    bytes: [u8; MAX_LINE_BYTES],
    len: usize,
    dropping: bool,
}
impl Default for Decoder {
    fn default() -> Self {
        Self::new()
    }
}
impl Decoder {
    pub const fn new() -> Self {
        Self {
            bytes: [0; MAX_LINE_BYTES],
            len: 0,
            dropping: false,
        }
    }
    pub fn feed(&mut self, b: u8) -> Option<Message> {
        if b == b'\n' {
            let result = if self.dropping {
                None
            } else {
                parse(&self.bytes[..self.len])
            };
            self.len = 0;
            self.dropping = false;
            return result;
        }
        if self.len == self.bytes.len() {
            self.dropping = true;
        }
        if !self.dropping {
            self.bytes[self.len] = b;
            self.len += 1;
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn valid_and_boundaries() {
        assert_eq!(parse(b"HELLO\r"), Some(Message::Hello));
        assert_eq!(
            parse(b"PCMON1,4294967295,1000,0,12,34,99"),
            Some(Message::Metrics(Metrics {
                sequence: u32::MAX,
                cpu: 1000,
                memory: 0,
                rx_kib: 12,
                tx_kib: 34,
                uptime_s: 99,
            }))
        );
    }
    #[test]
    fn rejects_bad_data() {
        for line in [
            "PCMON2,1,2,3,4,5,6",
            "PCMON1,1,1001,2,3,4,5",
            "PCMON1,1,2,1001,3,4,5",
            "PCMON1,-1,2,3,4,5,6",
            "PCMON1,1,2,3,4,5",
            "PCMON1,1,2,3,4,5,6,7",
            "PCMON1,4294967296,2,3,4,5,6",
            "PCMON1,1,NaN,3,4,5,6",
            "PCMON1,1,,3,4,5,6",
        ] {
            assert_eq!(parse(line.as_bytes()), None, "{line}");
        }
    }
    #[test]
    fn fragmented_coalesced_and_overflow_recovery() {
        let mut d = Decoder::new();
        for b in b"PCMON1,1,2," {
            assert_eq!(d.feed(*b), None);
        }
        let mut count = 0;
        for b in b"3,4,5,6\nHELLO\n" {
            if d.feed(*b).is_some() {
                count += 1;
            }
        }
        assert_eq!(count, 2);
        for _ in 0..200 {
            assert_eq!(d.feed(b'x'), None);
        }
        assert_eq!(d.feed(b'\n'), None);
        for b in b"HELLO" {
            assert_eq!(d.feed(*b), None);
        }
        assert_eq!(d.feed(b'\n'), Some(Message::Hello));
    }
}
