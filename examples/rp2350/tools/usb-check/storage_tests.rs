use super::*;
use embassy_usb::driver::{Endpoint, EndpointAddress, EndpointInfo, EndpointType};
use std::{
    collections::VecDeque,
    future::Future,
    task::{Context, Poll, Waker},
};

struct Input {
    info: EndpointInfo,
    packets: Vec<Vec<u8>>,
}
struct Output {
    info: EndpointInfo,
    packets: VecDeque<Vec<u8>>,
}
impl Endpoint for Input {
    fn info(&self) -> &EndpointInfo {
        &self.info
    }
    async fn wait_enabled(&mut self) {}
}
impl Endpoint for Output {
    fn info(&self) -> &EndpointInfo {
        &self.info
    }
    async fn wait_enabled(&mut self) {}
}
impl EndpointIn for Input {
    async fn write(&mut self, b: &[u8]) -> Result<(), EndpointError> {
        assert!(b.len() <= 64);
        self.packets.push(b.to_vec());
        Ok(())
    }
}
impl EndpointOut for Output {
    async fn read(&mut self, b: &mut [u8]) -> Result<usize, EndpointError> {
        let p = self.packets.pop_front().ok_or(EndpointError::Disabled)?;
        b[..p.len()].copy_from_slice(&p);
        Ok(p.len())
    }
}
fn cbw(tag: u32, incoming: bool, bytes: u32, command: &[u8]) -> Vec<u8> {
    let mut p = vec![0; 31];
    p[..4].copy_from_slice(b"USBC");
    p[4..8].copy_from_slice(&tag.to_le_bytes());
    p[8..12].copy_from_slice(&bytes.to_le_bytes());
    p[12] = if incoming { 128 } else { 0 };
    p[14] = command.len() as u8;
    p[15..15 + command.len()].copy_from_slice(command);
    p
}
fn disk(packets: Vec<Vec<u8>>) -> Disk<Input, Output> {
    let info = EndpointInfo {
        addr: EndpointAddress::from(1),
        ep_type: EndpointType::Bulk,
        max_packet_size: 64,
        interval_ms: 0,
    };
    Disk {
        input: Input {
            info,
            packets: vec![],
        },
        output: Output {
            info,
            packets: packets.into(),
        },
    }
}
fn run(d: &mut Disk<Input, Output>) {
    let mut f = std::pin::pin!(d.connected());
    assert!(matches!(
        f.as_mut().poll(&mut Context::from_waker(Waker::noop())),
        Poll::Ready(Err(EndpointError::Disabled))
    ));
}
fn csw(p: &[u8], tag: u32, residue: u32, status: u8) {
    assert_eq!(&p[..4], b"USBS");
    assert_eq!(p.len(), 13);
    assert_eq!(&p[4..8], &tag.to_le_bytes());
    assert_eq!(&p[8..12], &residue.to_le_bytes());
    assert_eq!(p[12], status);
}
#[test]
fn capacity_and_sector_read() {
    let mut d = disk(vec![
        cbw(1, true, 8, &[0x25, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
        cbw(2, true, 512, &[0x28, 0, 0, 0, 0, 0, 0, 0, 1, 0]),
    ]);
    run(&mut d);
    assert_eq!(
        &d.input.packets[0][..4],
        &((IMAGE.len() / 512 - 1) as u32).to_be_bytes()
    );
    csw(&d.input.packets[1], 1, 0, 0);
    assert_eq!(d.input.packets[2..10].concat(), IMAGE[..512]);
    csw(&d.input.packets[10], 2, 0, 0);
}
#[test]
fn rejected_write_preserves_tag_and_next_command() {
    let mut packets = vec![cbw(
        0x12345678,
        false,
        512,
        &[0x2a, 0, 0, 0, 0, 0, 0, 0, 1, 0],
    )];
    packets.extend(vec![vec![0xa5; 64]; 8]);
    packets.push(cbw(7, true, 18, &[3, 0, 0, 0, 18, 0]));
    let mut d = disk(packets);
    run(&mut d);
    csw(&d.input.packets[0], 0x12345678, 512, 1);
    assert_eq!(d.input.packets[1][2], 7);
    assert_eq!(d.input.packets[1][12], 0x27);
    csw(&d.input.packets[2], 7, 0, 0);
}
#[test]
fn out_of_range_read_terminates_data_before_csw() {
    let mut d = disk(vec![cbw(
        9,
        true,
        512,
        &[0x28, 0, 255, 255, 255, 255, 0, 0, 1, 0],
    )]);
    run(&mut d);
    assert!(d.input.packets[0].is_empty());
    csw(&d.input.packets[1], 9, 512, 1);
}
#[test]
fn short_inquiry_has_residue() {
    let mut d = disk(vec![cbw(3, true, 64, &[0x12, 0, 0, 0, 36, 0])]);
    run(&mut d);
    assert_eq!(d.input.packets[0].len(), 36);
    csw(&d.input.packets[1], 3, 28, 0);
}
#[test]
fn malformed_cbw_waits_for_reset() {
    let mut d = disk(vec![vec![0; 31]]);
    {
        let mut f = std::pin::pin!(d.connected());
        assert!(f
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
            .is_pending());
    }
    assert!(d.input.packets.is_empty());
}
