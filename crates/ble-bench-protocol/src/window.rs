//! Fixed Go-Back-N window; data IDs never count ACK/control packets.
//! Caller retains immutable payloads and validates session + ACK correlation.
pub const MAX_ATTEMPTS: usize = 8;
pub const fn retry_timeout_ms(attempt: usize) -> u64 {
    match attempt {
        0 => 100,
        1 => 200,
        _ => 400,
    }
}
#[derive(Debug)]
pub struct Window {
    pub base: u32,
    pub end: u32,
    pub width: u32,
}
impl Window {
    pub fn new(frame_bytes: u32, byte_limit: u32, frame_limit: u32) -> Option<Self> {
        if frame_bytes == 0 || frame_limit == 0 {
            return None;
        }
        let width = (byte_limit / frame_bytes).min(frame_limit).min(8);
        if width == 0 {
            return None;
        }
        Some(Self {
            base: 0,
            end: 0,
            width,
        })
    }
    pub fn begin(&mut self, total: u32) -> core::ops::Range<u32> {
        if self.base == self.end {
            self.end = self.base.saturating_add(self.width).min(total);
        }
        self.base..self.end
    }
    pub fn acknowledge(&mut self, next: u32) -> Result<(), &'static str> {
        if next < self.base || next > self.end {
            return Err("invalid ACK");
        }
        self.base = next;
        Ok(())
    }
    /// Re-evaluate the cumulative ACK at each packet boundary, including retries.
    pub fn next_pending(&self, cursor: u32) -> Option<u32> {
        let next = cursor.max(self.base);
        (next < self.end).then_some(next)
    }
    /// Consume the whole read, including coalesced ACKs. Duplicate/stale ACKs
    /// neither move the window backwards nor trigger an immediate retry.
    pub fn receive_acks(
        &mut self,
        decoder: &mut crate::Decoder,
        bytes: &[u8],
        run: u128,
        turn: u32,
        sent_end: u32,
    ) -> bool {
        let before = self.base;
        for &byte in bytes {
            if let Some(f) = decoder.feed(byte) {
                if f.kind == crate::ACK && f.run == run && f.sequence == turn && f.len == 4 {
                    let next = u32::from_le_bytes(f.payload[..4].try_into().unwrap());
                    if next > self.base && next <= sent_end {
                        let _ = self.acknowledge(next);
                    }
                }
            }
        }
        self.base > before
    }
}
#[derive(Default)]
pub struct Receiver {
    pub next: u32,
}
impl Receiver {
    pub fn accept(&mut self, sequence: u32) -> bool {
        if sequence != self.next || self.next == u32::MAX {
            return false;
        }
        self.next += 1;
        true
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fixed_window_and_retries_keep_data_ids() {
        let mut tx = Window::new(128, 512, 4).unwrap();
        let mut rx = Receiver::default();
        assert_eq!(tx.begin(10), 0..4);
        assert!(rx.accept(0));
        assert!(!rx.accept(0));
        assert!(!rx.accept(2));
        tx.acknowledge(rx.next).unwrap();
        assert_eq!(tx.begin(10), 1..4);
        assert_eq!(tx.width, 4);
        for i in 1..4 {
            assert!(rx.accept(i));
        }
        tx.acknowledge(4).unwrap();
        assert_eq!(tx.begin(10), 4..8);
        assert!(tx.acknowledge(3).is_err());
        assert!(tx.acknowledge(9).is_err());
        assert_eq!(tx.width, 4);
    }
    #[test]
    fn byte_boundaries_and_exact_retry_schedule() {
        assert!(Window::new(1024, 1023, 2).is_none());
        assert_eq!(Window::new(1024, 2048, 8).unwrap().width, 2);
        assert_eq!(
            [
                retry_timeout_ms(0),
                retry_timeout_ms(1),
                retry_timeout_ms(2),
                retry_timeout_ms(7)
            ],
            [100, 200, 400, 400]
        );
    }
    #[test]
    fn late_ack_skips_retry_tail_and_coalesced_acks_are_all_consumed() {
        let mut tx = Window::new(128, 1024, 8).unwrap();
        tx.begin(16);
        let mut decoder = crate::Decoder::new();
        let mut bytes = [0; crate::MAX_FRAME];
        // Retry of packet 0 has already started when ACK(3) arrives.
        assert_eq!(tx.next_pending(0), Some(0));
        let n = crate::encode(crate::ACK, 7, 1, &3u32.to_le_bytes(), &mut bytes).unwrap();
        assert!(!tx.receive_acks(&mut decoder, &bytes[..10], 7, 1, 8));
        assert!(tx.receive_acks(&mut decoder, &bytes[10..n], 7, 1, 8));
        assert_eq!(tx.next_pending(1), Some(3));
        let n = crate::encode(crate::ACK, 7, 1, &5u32.to_le_bytes(), &mut bytes).unwrap();
        let m = crate::encode(crate::ACK, 7, 1, &8u32.to_le_bytes(), &mut bytes[n..]).unwrap();
        assert!(tx.receive_acks(&mut decoder, &bytes[..n + m], 7, 1, 8));
        assert_eq!(tx.next_pending(4), None);
        assert_eq!(tx.width, 8);
        assert_eq!(tx.begin(16), 8..16);
        // Old turn/session, unsent range and duplicate ACK must not advance.
        for (run, turn, next) in [(6, 2, 10u32), (7, 1, 10), (7, 2, 16), (7, 2, 8)] {
            let n = crate::encode(crate::ACK, run, turn, &next.to_le_bytes(), &mut bytes).unwrap();
            assert!(!tx.receive_acks(&mut decoder, &bytes[..n], 7, 2, 10));
        }
        assert_eq!(tx.base, 8);
    }
}
