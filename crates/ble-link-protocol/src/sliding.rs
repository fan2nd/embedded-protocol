//! Fixed-capacity sliding window; each packet owns its retry clock.
//! Callers retain immutable payloads until cumulatively acknowledged.
pub const MAX_WINDOW: usize = 8;
pub const MAX_ATTEMPTS: u8 = 8;
pub const fn retry_timeout_ms(attempts: u8, wire_bytes: u16) -> u64 {
    let units = (wire_bytes as u64).div_ceil(128);
    let base = match attempts {
        0 | 1 => 100,
        2 => 200,
        _ => 400,
    };
    base * if units == 0 { 1 } else { units }
}
#[derive(Clone, Copy, Default, Debug)]
struct Slot {
    id: u32,
    bytes: u16,
    sent_at: u64,
    attempts: u8,
}
#[derive(Debug, PartialEq, Eq)]
pub enum Due {
    None,
    Retry(u32),
    Exhausted(u32),
}
#[derive(Debug)]
pub struct Sender {
    pub base: u32,
    pub next: u32,
    pub width: u32,
    byte_limit: u32,
    used: u32,
    slots: [Slot; MAX_WINDOW],
}
impl Sender {
    pub fn new(width: u32, byte_limit: u32) -> Option<Self> {
        if !(1..=MAX_WINDOW as u32).contains(&width) || !(64..=8192).contains(&byte_limit) {
            return None;
        }
        Some(Self {
            base: 0,
            next: 0,
            width,
            byte_limit,
            used: 0,
            slots: [Slot::default(); MAX_WINDOW],
        })
    }
    pub fn new_at(width: u32, byte_limit: u32, first: u32) -> Option<Self> {
        let mut s = Self::new(width, byte_limit)?;
        s.base = first;
        s.next = first;
        Some(s)
    }
    pub fn can_send(&self, bytes: usize) -> bool {
        self.next < u32::MAX
            && self.next - self.base < self.width
            && bytes <= u16::MAX as usize
            && self.used + bytes as u32 <= self.byte_limit
    }
    pub fn start(&mut self, bytes: usize, now: u64) -> Option<u32> {
        if !self.can_send(bytes) {
            return None;
        }
        let id = self.next;
        self.next += 1;
        self.used += bytes as u32;
        self.slots[id as usize % MAX_WINDOW] = Slot {
            id,
            bytes: bytes as u16,
            sent_at: now,
            attempts: 1,
        };
        Some(id)
    }
    pub fn pending(&self, id: u32) -> bool {
        id >= self.base && id < self.next
    }
    pub fn acknowledge(&mut self, next: u32) -> bool {
        if next <= self.base || next > self.next {
            return false;
        }
        for id in self.base..next {
            self.used -= u32::from(self.slots[id as usize % MAX_WINDOW].bytes)
        }
        self.base = next;
        true
    }
    pub fn submitted(&mut self, id: u32, now: u64) {
        if self.pending(id) {
            self.slots[id as usize % MAX_WINDOW].sent_at = now;
        }
    }
    pub fn retry(&mut self, id: u32, now: u64) -> bool {
        if !self.pending(id) {
            return false;
        }
        let slot = &mut self.slots[id as usize % MAX_WINDOW];
        if slot.attempts >= MAX_ATTEMPTS {
            return false;
        }
        slot.attempts += 1;
        slot.sent_at = now;
        true
    }
    pub fn due(&self, now: u64) -> Due {
        for id in self.base..self.next {
            let slot = &self.slots[id as usize % MAX_WINDOW];
            debug_assert_eq!(slot.id, id);
            if now.saturating_sub(slot.sent_at) >= retry_timeout_ms(slot.attempts, slot.bytes) {
                return if slot.attempts >= MAX_ATTEMPTS {
                    Due::Exhausted(id)
                } else {
                    Due::Retry(id)
                };
            }
        }
        Due::None
    }
    pub fn wait_ms(&self, now: u64) -> u64 {
        (self.base..self.next)
            .map(|id| {
                let s = &self.slots[id as usize % MAX_WINDOW];
                s.sent_at
                    .saturating_add(retry_timeout_ms(s.attempts, s.bytes))
                    .saturating_sub(now)
            })
            .min()
            .unwrap_or(1000)
    }
    pub fn empty(&self) -> bool {
        self.base == self.next
    }
}
#[derive(Default)]
pub struct Receiver {
    pub next: u32,
}
impl Receiver {
    pub fn accept(&mut self, id: u32) -> bool {
        if id != self.next || self.next == u32::MAX {
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
    fn timeout_rounds_up_full_wire_length_in_128_byte_units() {
        for (bytes, units) in [
            (19, 1),
            (64, 1),
            (128, 1),
            (129, 2),
            (182, 2),
            (256, 2),
            (257, 3),
            (512, 4),
            (1024, 8),
            (1043, 9),
        ] {
            assert_eq!(retry_timeout_ms(1, bytes), units * 100);
            assert_eq!(retry_timeout_ms(2, bytes), units * 200);
            assert_eq!(retry_timeout_ms(3, bytes), units * 400);
            assert_eq!(retry_timeout_ms(8, bytes), units * 400);
        }
    }
    #[test]
    fn mixed_packet_sizes_keep_independent_scaled_deadlines() {
        let mut s = Sender::new(4, 4096).unwrap();
        s.start(128, 0);
        s.start(129, 0);
        s.start(512, 0);
        assert_eq!(s.wait_ms(99), 1);
        assert_eq!(s.due(100), Due::Retry(0));
        s.acknowledge(1);
        assert_eq!(s.wait_ms(100), 100);
        assert_eq!(s.due(199), Due::None);
        assert_eq!(s.due(200), Due::Retry(1));
        s.retry(1, 200);
        assert_eq!(s.due(399), Due::None);
        assert_eq!(s.due(400), Due::Retry(2));
        s.acknowledge(2);
        assert_eq!(s.wait_ms(400), 0);
        assert_eq!(s.width, 4);
    }
    #[test]
    fn lost_ack_retries_only_pending_data_without_duplicate_delivery() {
        let mut s = Sender::new(2, 256).unwrap();
        let mut r = Receiver::default();
        assert_eq!(s.start(128, 0), Some(0));
        assert!(r.accept(0)); // Its ACK is lost.
        assert_eq!(s.due(100), Due::Retry(0));
        assert!(s.retry(0, 100));
        assert!(!r.accept(0)); // Duplicate still elicits ACK(next=1).
        assert!(s.acknowledge(r.next));
        assert!(!s.retry(0, 300));
        assert_eq!(s.start(128, 101), Some(1));
    }
    #[test]
    fn lost_data_recovers_gap_and_refills_before_later_packet_is_acked() {
        let mut s = Sender::new(2, 256).unwrap();
        let mut r = Receiver::default();
        s.start(128, 0); // Packet 0 is lost.
        s.start(128, 1);
        assert!(!r.accept(1));
        assert!(!s.acknowledge(r.next));
        assert_eq!(s.due(100), Due::Retry(0));
        assert!(s.retry(0, 100));
        assert!(r.accept(0));
        assert!(s.acknowledge(r.next));
        assert_eq!(s.start(128, 100), Some(2)); // 1 remains outstanding.
        assert_eq!(s.due(101), Due::Retry(1));
        assert!(s.retry(1, 101));
        assert!(r.accept(1));
        assert!(s.acknowledge(r.next));
        assert!(r.accept(2));
        assert!(s.acknowledge(r.next));
        assert!(s.empty());
    }
    #[test]
    fn duplicate_and_invalid_acks_do_not_postpone_retry() {
        let mut s = Sender::new(2, 256).unwrap();
        s.start(128, 0);
        s.start(128, 10);
        assert!(s.acknowledge(1));
        for ack in [0, 1, 3, u32::MAX] {
            assert!(!s.acknowledge(ack));
        }
        assert_eq!(s.wait_ms(109), 1);
        assert_eq!(s.due(110), Due::Retry(1));
        assert_eq!(s.width, 2);
    }
    #[test]
    fn partial_ack_immediately_refills_without_waiting_for_whole_window() {
        let mut s = Sender::new(4, 512).unwrap();
        for id in 0..4 {
            assert_eq!(s.start(128, 0), Some(id));
        }
        assert!(!s.can_send(128));
        assert!(s.acknowledge(1));
        assert_eq!(s.start(128, 1), Some(4));
        assert_eq!(s.base, 1);
        assert_eq!(s.width, 4);
        assert!(!s.pending(0));
        assert!(!s.retry(0, 100));
        assert!(!s.acknowledge(0));
        assert!(!s.acknowledge(6));
        assert!(s.acknowledge(5));
        assert!(s.empty());
    }
    #[test]
    fn independent_deadlines_and_no_shrink() {
        let mut s = Sender::new(2, 256).unwrap();
        s.start(128, 0);
        s.start(128, 50);
        assert_eq!(s.due(99), Due::None);
        assert_eq!(s.due(100), Due::Retry(0));
        assert!(s.retry(0, 100));
        assert_eq!(s.due(150), Due::Retry(1));
        assert!(s.acknowledge(1));
        assert_eq!(s.start(128, 151), Some(2));
        assert_eq!(s.due(151), Due::Retry(1));
        assert!(s.retry(1, 151));
        assert_eq!(s.width, 2);
    }
    #[test]
    fn exact_backoff_and_retry_exhaustion() {
        let mut s = Sender::new(1, 64).unwrap();
        s.start(64, 0);
        let mut now = 0;
        for attempt in 1..8 {
            now += retry_timeout_ms(attempt, 64);
            assert_eq!(s.due(now), Due::Retry(0));
            assert!(s.retry(0, now));
        }
        now += 400;
        assert_eq!(s.due(now), Due::Exhausted(0));
        assert_eq!(s.width, 1);
    }
    #[test]
    fn byte_credit_and_receiver_dedup() {
        let mut s = Sender::new(8, 128).unwrap();
        assert_eq!(s.start(100, 0), Some(0));
        assert!(!s.can_send(64));
        assert!(s.acknowledge(1));
        assert!(s.can_send(128));
        let mut r = Receiver::default();
        assert!(!r.accept(1));
        assert!(r.accept(0));
        assert!(!r.accept(0));
        assert!(r.accept(1));
    }
}
