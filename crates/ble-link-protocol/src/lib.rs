#![no_std]
pub mod sliding;
pub const MAX_PAYLOAD: usize = 1024;
pub const OVERHEAD: usize = 17;
pub const VERSION: u16 = 6;
pub const HELLO_MAGIC: &[u8; 4] = b"RLB6";
pub const MAX_FRAME: usize = MAX_PAYLOAD + OVERHEAD;
pub const HELLO: u8 = 1;
pub const DATA: u8 = 2;
pub const ACK: u8 = 3;
fn packet_class(kind: u8) -> Option<()> {
    matches!(kind, HELLO | DATA | ACK).then_some(())
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame {
    pub kind: u8,
    pub ack_next: u32,
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
pub fn encode(
    kind: u8,
    sequence: u32,
    ack_next: u32,
    payload: &[u8],
    out: &mut [u8],
) -> Option<usize> {
    let n = OVERHEAD + payload.len();
    if payload.len() > MAX_PAYLOAD || out.len() < n || packet_class(kind).is_none() {
        return None;
    }
    out[..2].copy_from_slice(&[0xa5, 0x5a]);
    out[2] = kind;
    out[3..7].copy_from_slice(&sequence.to_le_bytes());
    out[7..11].copy_from_slice(&ack_next.to_le_bytes());
    out[11..13].copy_from_slice(&(payload.len() as u16).to_le_bytes());
    out[13..13 + payload.len()].copy_from_slice(payload);
    let crc = crc32c(&out[..n - 4]);
    out[n - 4..n].copy_from_slice(&crc.to_le_bytes());
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
    /// A truncated large frame must not hold a later, shorter ACK indefinitely.
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
            if self.len < 13 {
                return None;
            }
            let size = u16::from_le_bytes([self.bytes[11], self.bytes[12]]) as usize;
            if size > MAX_PAYLOAD || packet_class(self.bytes[2]).is_none() {
                self.errors += 1;
                self.discard(1);
                continue;
            }
            let n = size + OVERHEAD;
            if self.len < n {
                return None;
            }
            let crc = u32::from_le_bytes(self.bytes[n - 4..n].try_into().unwrap());
            if crc32c(&self.bytes[..n - 4]) != crc {
                self.errors += 1;
                // A complete candidate is consumed once, even when corrupt.
                // Resume at its end; never reinterpret payload bytes as frames.
                self.discard(n);
                continue;
            }
            let mut f = Frame {
                kind: self.bytes[2],
                ack_next: u32::from_le_bytes(self.bytes[7..11].try_into().unwrap()),
                sequence: u32::from_le_bytes(self.bytes[3..7].try_into().unwrap()),
                len: size,
                payload: [0; MAX_PAYLOAD],
            };
            f.payload[..size].copy_from_slice(&self.bytes[13..13 + size]);
            self.discard(n);
            return Some(f);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn roundtrip(kind: u8, seq: u32, ack: u32, payload: &[u8]) -> Frame {
        let mut wire = [0; MAX_FRAME];
        let n = encode(kind, seq, ack, payload, &mut wire).unwrap();
        assert_eq!(n, 17 + payload.len());
        let mut decoder = Decoder::new();
        wire[..n]
            .iter()
            .filter_map(|b| decoder.feed(*b))
            .last()
            .unwrap()
    }
    #[test]
    fn all_types_use_the_same_layout() {
        for kind in [HELLO, DATA, ACK] {
            let f = roundtrip(kind, 123, 456, b"AT\r\n+++\xa5\x5a");
            assert_eq!((f.kind, f.sequence, f.ack_next), (kind, 123, 456));
            assert_eq!(&f.payload[..f.len], b"AT\r\n+++\xa5\x5a");
        }
    }
    #[test]
    fn bidirectional_data_ack_releases_slots_and_retry_can_refresh_ack() {
        let mut a = sliding::Sender::new_at(1, 128, 20).unwrap();
        let mut b = sliding::Sender::new_at(1, 128, 80).unwrap();
        let mut ar = sliding::Receiver { next: 80 };
        let mut br = sliding::Receiver { next: 20 };
        let id = a.start(128, 0).unwrap();
        let f = roundtrip(DATA, id, ar.next, b"a");
        assert!(br.accept(f.sequence));
        let id = b.start(128, 1).unwrap();
        let f = roundtrip(DATA, id, br.next, b"b");
        assert!(a.acknowledge(f.ack_next));
        assert!(ar.accept(f.sequence));
        let id = a.start(128, 2).unwrap();
        let f = roundtrip(DATA, id, ar.next, b"c");
        assert!(b.acknowledge(f.ack_next));
        assert!(br.accept(f.sequence));
        // Lost standalone ACK: repeat DATA ID/payload but update reverse confirmation.
        assert!(a.retry(id, 102));
        let f = roundtrip(DATA, id, 81, b"c");
        assert!(!br.accept(f.sequence));
        let ack = roundtrip(ACK, 0, br.next, b"");
        assert!(a.acknowledge(ack.ack_next));
        assert!(!a.retry(id, 302));
    }
    #[test]
    fn corrupt_candidate_skips_embedded_valid_frames_and_resumes_at_end() {
        let mut inner = [0; MAX_FRAME];
        let inner_len = encode(ACK, 0, 999, &[], &mut inner).unwrap();
        let mut bad = [0; MAX_FRAME];
        let n = encode(DATA, 0, 0, &inner[..inner_len], &mut bad).unwrap();
        bad[n - 4] ^= 1;
        let mut good = [0; MAX_FRAME];
        let m = encode(ACK, 0, 3, &[], &mut good).unwrap();
        let mut decoder = Decoder::new();
        for &b in &bad[..n] {
            assert!(decoder.feed(b).is_none());
        }
        assert_eq!(decoder.errors, 1);
        let mut frames = 0;
        for &b in &good[..m] {
            if let Some(f) = decoder.feed(b) {
                frames += 1;
                assert_eq!(f.ack_next, 3);
            }
        }
        assert_eq!(frames, 1);
        assert_eq!(decoder.errors, 1);
    }
    #[test]
    fn crc_failure_resynchronizes() {
        let mut bad = [0; MAX_FRAME];
        let n = encode(DATA, 0, 0, b"damaged", &mut bad).unwrap();
        bad[14] ^= 1;
        let mut good = [0; MAX_FRAME];
        let m = encode(ACK, 0, 3, &[], &mut good).unwrap();
        let mut decoder = Decoder::new();
        let mut found = None;
        for &v in bad[..n].iter().chain(&good[..m]) {
            if let Some(f) = decoder.feed(v) {
                found = Some(f);
            }
        }
        assert_eq!(found.unwrap().ack_next, 3);
        assert!(decoder.errors > 0);
    }
}
