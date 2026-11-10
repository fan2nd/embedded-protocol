//! Diagnostic protocol, deliberately separate from the PCMON-BLE1 draft.
#![no_std]
pub mod session;
pub mod window;
pub const MAX_PAYLOAD: usize = 1024;
pub const OVERHEAD: usize = 31;
pub const MAX_FRAME: usize = MAX_PAYLOAD + OVERHEAD;
pub const HELLO: u8 = 17;
pub const READY: u8 = 49;
pub const DATA: u8 = 33;
pub const REPORT: u8 = 18;
pub const STATS: u8 = 50;
pub const TX_START: u8 = 19;
pub const TX_DATA: u8 = 34;
pub const TX_DONE: u8 = 51;
pub const POLL: u8 = 20;
pub const ACK: u8 = 52;
pub const CHALLENGE: u8 = 0x35;
pub const CONFIRM: u8 = 0x15;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PacketClass {
    Control,
    Data,
    Response,
}
pub fn packet_class(kind: u8) -> Option<PacketClass> {
    match kind {
        HELLO | REPORT | TX_START | POLL | CONFIRM => Some(PacketClass::Control),
        DATA | TX_DATA => Some(PacketClass::Data),
        READY | STATS | TX_DONE | ACK | CHALLENGE => Some(PacketClass::Response),
        _ => None,
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame {
    pub kind: u8,
    pub run: u128,
    pub sequence: u32,
    pub len: usize,
    pub payload: [u8; MAX_PAYLOAD],
}
const fn crc_table() -> [u32; 256] {
    let mut table = [0; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut bit = 0;
        while bit < 8 {
            c = (c >> 1) ^ if c & 1 != 0 { 0x82f63b78 } else { 0 };
            bit += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
}
const CRC_TABLE: [u32; 256] = crc_table();
pub fn crc32c(bytes: &[u8]) -> u32 {
    let mut c = 0xffff_ffff;
    for &b in bytes {
        c = (c >> 8) ^ CRC_TABLE[((c ^ b as u32) & 255) as usize];
    }
    !c
}
pub fn encode(kind: u8, run: u128, sequence: u32, payload: &[u8], out: &mut [u8]) -> Option<usize> {
    let n = OVERHEAD + payload.len();
    if payload.len() > MAX_PAYLOAD || out.len() < n || packet_class(kind).is_none() {
        return None;
    }
    out[..2].copy_from_slice(&[0xa5, 0x5a]);
    out[2] = kind;
    out[3..19].copy_from_slice(&run.to_le_bytes());
    out[19..23].copy_from_slice(&sequence.to_le_bytes());
    out[23..25].copy_from_slice(&(payload.len() as u16).to_le_bytes());
    out[25..25 + payload.len()].copy_from_slice(payload);
    let crc = crc32c(&out[..n - 6]);
    out[n - 6..n - 2].copy_from_slice(&crc.to_le_bytes());
    out[n - 2..n].copy_from_slice(&[0x5a, 0xa5]);
    Some(n)
}
pub struct Decoder {
    bytes: [u8; MAX_FRAME],
    len: usize,
    pub errors: u32,
}
impl Default for Decoder {
    fn default() -> Self {
        Self::new()
    }
}
impl Decoder {
    pub const fn new() -> Self {
        Self {
            bytes: [0; MAX_FRAME],
            len: 0,
            errors: 0,
        }
    }
    fn discard(&mut self, n: usize) {
        self.bytes.copy_within(n..self.len, 0);
        self.len -= n;
    }
    /// Called only after the transport's bounded inter-byte idle deadline.
    /// A truncated large frame must not hold a later, shorter POLL indefinitely.
    pub fn expire_partial(&mut self) {
        if self.len != 0 {
            self.len = 0;
            self.errors += 1;
        }
    }
    pub fn feed(&mut self, b: u8) -> Option<Frame> {
        if self.len == MAX_FRAME {
            self.discard(1);
            self.errors += 1;
        }
        self.bytes[self.len] = b;
        self.len += 1;
        loop {
            if self.len < 2 {
                return None;
            }
            if self.bytes[..2] != [0xa5, 0x5a] {
                self.discard(1);
                continue;
            }
            if self.len < 25 {
                return None;
            }
            let size = u16::from_le_bytes([self.bytes[23], self.bytes[24]]) as usize;
            if size > MAX_PAYLOAD || packet_class(self.bytes[2]).is_none() {
                self.errors += 1;
                self.discard(1);
                continue;
            }
            let n = size + OVERHEAD;
            if self.len < n {
                return None;
            }
            let crc = u32::from_le_bytes(self.bytes[n - 6..n - 2].try_into().unwrap());
            if self.bytes[n - 2..n] != [0x5a, 0xa5] || crc32c(&self.bytes[..n - 6]) != crc {
                self.errors += 1;
                self.discard(1);
                continue;
            }
            let mut f = Frame {
                kind: self.bytes[2],
                run: u128::from_le_bytes(self.bytes[3..19].try_into().unwrap()),
                sequence: u32::from_le_bytes(self.bytes[19..23].try_into().unwrap()),
                len: size,
                payload: [0; MAX_PAYLOAD],
            };
            f.payload[..size].copy_from_slice(&self.bytes[25..25 + size]);
            self.discard(n);
            return Some(f);
        }
    }
}
pub fn pattern(sequence: u32, index: usize) -> u8 {
    (sequence.wrapping_mul(31).wrapping_add(index as u32 * 17)) as u8
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn truncated_frame_expires_before_poll_retry() {
        let mut data = [0; MAX_FRAME];
        let n = encode(DATA, 7, 0, &[3; 163], &mut data).unwrap();
        let mut d = Decoder::new();
        for &b in &data[..n / 2] {
            assert!(d.feed(b).is_none());
        }
        d.expire_partial();
        let n = encode(POLL, 7, 1, &1u32.to_le_bytes(), &mut data).unwrap();
        let mut got = None;
        for &b in &data[..n] {
            got = d.feed(b).or(got);
        }
        assert_eq!(got.unwrap().kind, POLL);
        assert_eq!(d.errors, 1);
    }
    #[test]
    fn maximum_frame_back_to_back() {
        let payload = core::array::from_fn::<_, MAX_PAYLOAD, _>(|i| pattern(9, i));
        let mut out = [0; MAX_FRAME];
        let n = encode(DATA, 7, 9, &payload, &mut out).unwrap();
        assert_eq!(n, MAX_FRAME);
        let mut decoder = Decoder::new();
        let mut received = 0;
        for _ in 0..3 {
            for &byte in &out {
                if let Some(frame) = decoder.feed(byte) {
                    assert_eq!(frame.len, MAX_PAYLOAD);
                    assert_eq!(frame.payload, payload);
                    received += 1;
                }
            }
        }
        assert_eq!(received, 3);
        assert_eq!(decoder.errors, 0);
    }
    #[test]
    fn truncated_payload_resynchronizes_to_following_full_packet() {
        for size in [64, 182, 1024] {
            let payload = [0x42; MAX_PAYLOAD];
            let mut old = [0; MAX_FRAME];
            let mut next = [0; MAX_FRAME];
            let n = encode(DATA, 7, 0, &payload[..size - OVERHEAD], &mut old).unwrap();
            encode(DATA, 7, 1, &payload[..size - OVERHEAD], &mut next).unwrap();
            // Complete length header, truncated payload. A header fragment
            // can instead require the separately tested inter-byte idle reset.
            for cut in [25, n / 2, n - 7] {
                let mut decoder = Decoder::new();
                let mut received = None;
                for &byte in old[..cut].iter().chain(next[..n].iter()) {
                    if let Some(frame) = decoder.feed(byte) {
                        assert!(received.is_none());
                        received = Some(frame);
                    }
                }
                assert_eq!(
                    received.map(|frame| frame.sequence),
                    Some(1),
                    "size={size}, cut={cut}"
                );
            }
        }
    }
    #[test]
    fn corruption_recovery_and_binary_payload() {
        assert_eq!(crc32c(b"123456789"), 0xe3069283);
        let mut out = [0; MAX_FRAME];
        let n = encode(DATA, 7, 3, b"AT+++\r\n\xa5\x5a\x5a\xa5", &mut out).unwrap();
        let mut d = Decoder::new();
        let mut bad = out;
        bad[25] ^= 1;
        for &b in &bad[..n] {
            assert!(d.feed(b).is_none());
        }
        let mut got = None;
        for &b in &out[..n] {
            got = d.feed(b).or(got);
        }
        let f = got.unwrap();
        assert_eq!((f.run, f.sequence, f.len), (7, 3, 11));
        assert!(d.errors > 0);
    }
}
