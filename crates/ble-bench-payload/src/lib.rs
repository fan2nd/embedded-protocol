//! Application payloads for the BLE diagnostic example; not link packet types.
#![no_std]
pub const TEST: u8 = 0;
pub const TEST_OVERHEAD: usize = ble_link_protocol::OVERHEAD + 1;
pub const START: u8 = 1;
pub const QUERY: u8 = 2;
pub const STATS: u8 = 3;
pub const DONE: u8 = 4;
pub fn pattern(id: u32, index: usize) -> u8 {
    id.wrapping_mul(31)
        .wrapping_add((index as u32).wrapping_mul(17)) as u8
}
pub fn fill(id: u32, payload: &mut [u8]) {
    payload[0] = TEST;
    for (i, b) in payload[1..].iter_mut().enumerate() {
        *b = pattern(id, i);
    }
}
pub fn valid(id: u32, payload: &[u8]) -> bool {
    payload.first() == Some(&TEST)
        && payload[1..]
            .iter()
            .enumerate()
            .all(|(i, b)| *b == pattern(id, i))
}
pub fn report(tag: u8, values: [u32; 8]) -> [u8; 33] {
    let mut out = [0; 33];
    out[0] = tag;
    for (i, v) in values.iter().enumerate() {
        out[1 + i * 4..5 + i * 4].copy_from_slice(&v.to_le_bytes());
    }
    out
}

/// Deterministic mixed test traffic, expressed as complete wire frame lengths.
/// Upstream uses a half-cycle phase shift; both directions send equal mixes.
pub const MIXED_WIRE: [usize; 6] = [64, 1024, 128, 512, 182, 256];
pub fn mixed_wire(index: u32, upstream: bool) -> usize {
    MIXED_WIRE
        [(index as usize % MIXED_WIRE.len() + if upstream { 3 } else { 0 }) % MIXED_WIRE.len()]
}
pub fn mixed_count(minimum_bytes: usize) -> Option<u32> {
    let cycle_bytes: usize = MIXED_WIRE.iter().map(|n| n - TEST_OVERHEAD).sum();
    let count = minimum_bytes
        .div_ceil(cycle_bytes)
        .checked_mul(MIXED_WIRE.len())?;
    (count > 0 && count <= 8192).then_some(count as u32)
}
/// Complete wire length: protocol overhead plus at least eight payload bytes.
pub const RANDOM_MIN_WIRE: usize = ble_link_protocol::OVERHEAD + 8;
pub const RANDOM_SEEDS: [u32; 2] = [0x6d2b79f5, 0xa341316c];
/// Stateless seeded pseudo-random lengths: retransmission of an index is identical.
/// This mixer is for reproducible test traffic, not cryptography.
pub fn random_wire(index: u32, upstream: bool, cap: usize) -> usize {
    assert!(cap >= RANDOM_MIN_WIRE);
    let mut x = index.wrapping_add(RANDOM_SEEDS[usize::from(upstream)]);
    x = (x ^ (x >> 16)).wrapping_mul(0x7feb352d);
    x = (x ^ (x >> 15)).wrapping_mul(0x846ca68b);
    x ^= x >> 16;
    RANDOM_MIN_WIRE + x as usize % (cap - RANDOM_MIN_WIRE + 1)
}
pub fn random_count(minimum_bytes: usize, cap: usize) -> Option<u32> {
    if minimum_bytes == 0 || !(RANDOM_MIN_WIRE..=1024).contains(&cap) {
        return None;
    }
    let mut totals = [0usize; 2];
    for i in 0..8192 {
        totals[0] += random_wire(i, false, cap) - TEST_OVERHEAD;
        totals[1] += random_wire(i, true, cap) - TEST_OVERHEAD;
        if totals.iter().all(|n| *n >= minimum_bytes) {
            return Some(i + 1);
        }
    }
    None
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn random_lengths_are_reproducible_bounded_and_cover_continuous_sizes() {
        for cap in [128, 182, 256, 512] {
            let count = random_count(16384, cap).unwrap();
            for up in [false, true] {
                let total: usize = (0..count)
                    .map(|i| random_wire(i, up, cap) - TEST_OVERHEAD)
                    .sum();
                assert!(total >= 16384);
                let mut seen = [false; 513];
                for i in 0..20000 {
                    let n = random_wire(i, up, cap);
                    assert!((RANDOM_MIN_WIRE..=cap).contains(&n));
                    seen[n] = true;
                    assert_eq!(n, random_wire(i, up, cap));
                }
                assert!(seen[RANDOM_MIN_WIRE..=cap].iter().all(|v| *v));
            }
            assert!((0..count).any(|i| random_wire(i, false, cap) != random_wire(i, true, cap)));
        }
        assert_eq!(random_count(0, 128), None);
        assert_eq!(random_count(16384, RANDOM_MIN_WIRE - 1), None);
        assert_eq!(random_count(usize::MAX, 128), None);
    }
    #[test]
    fn mixed_cycles_have_equal_bytes_and_different_phases() {
        let count = mixed_count(16384).unwrap();
        assert_eq!(count, 48);
        let total = |up| {
            (0..count)
                .map(|i| mixed_wire(i, up) - TEST_OVERHEAD)
                .sum::<usize>()
        };
        assert_eq!(total(false), 16464);
        assert_eq!(total(false), total(true));
        assert_ne!(mixed_wire(0, false), mixed_wire(0, true));
        assert_eq!(mixed_count(0), None);
        assert_eq!(mixed_count(usize::MAX), None);
    }
}
