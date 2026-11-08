//! Read-only FAT image in XIP flash; USB never writes or erases firmware flash.
use embassy_futures::select::select;
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
use embassy_usb::{
    control::{InResponse, OutResponse, Recipient, Request, RequestType},
    driver::{Driver, EndpointError, EndpointIn, EndpointOut},
    Builder, Handler,
};

static IMAGE: &[u8] = include_bytes!(env!("PCMON_DISK_IMAGE"));
const BLOCK: usize = 512;
static RESET: Signal<CriticalSectionRawMutex, ()> = Signal::new();

pub struct Control {
    interface: u16,
}
impl Control {
    pub fn new() -> Self {
        Self { interface: 0 }
    }
}
impl Handler for Control {
    fn reset(&mut self) {
        RESET.signal(());
    }
    fn control_in<'a>(&'a mut self, r: Request, buf: &'a mut [u8]) -> Option<InResponse<'a>> {
        if (r.request_type, r.recipient, r.index)
            != (RequestType::Class, Recipient::Interface, self.interface)
        {
            return None;
        }
        Some(if r.request == 0xfe && r.value == 0 && r.length == 1 {
            buf[0] = 0;
            InResponse::Accepted(&buf[..1])
        } else {
            InResponse::Rejected
        })
    }
    fn control_out(&mut self, r: Request, _: &[u8]) -> Option<OutResponse> {
        if (r.request_type, r.recipient, r.index)
            != (RequestType::Class, Recipient::Interface, self.interface)
        {
            return None;
        }
        Some(if r.request == 0xff && r.value == 0 && r.length == 0 {
            RESET.signal(());
            OutResponse::Accepted
        } else {
            OutResponse::Rejected
        })
    }
}

pub struct Disk<I: EndpointIn, O: EndpointOut> {
    input: I,
    output: O,
}
impl<I: EndpointIn, O: EndpointOut> Disk<I, O> {
    pub fn new<'d, D: Driver<'d, EndpointIn = I, EndpointOut = O>>(
        b: &mut Builder<'d, D>,
        control: &'d mut Control,
    ) -> Self {
        let mut f = b.function(8, 6, 0x50);
        let mut i = f.interface();
        control.interface = i.interface_number().0 as u16;
        let mut a = i.alt_setting(8, 6, 0x50, None);
        let output = a.endpoint_bulk_out(None, 64);
        let input = a.endpoint_bulk_in(None, 64);
        drop(f);
        b.handler(control);
        Self { input, output }
    }
    pub async fn run(&mut self) -> ! {
        loop {
            self.output.wait_enabled().await;
            // A BOT/USB reset cancels even a blocked data transfer.
            let _ = select(RESET.wait(), self.connected()).await;
        }
    }
    async fn connected(&mut self) -> Result<(), EndpointError> {
        let mut sense = [0u8; 3];
        let mut ejected = false;
        loop {
            let mut packet = [0u8; 64];
            let n = self.output.read(&mut packet).await?;
            if n != 31
                || &packet[..4] != b"USBC"
                || packet[12] & 0x7f != 0
                || packet[13] > 15
                || !(1..=16).contains(&packet[14])
            {
                // Invalid CBW: wait for class reset, never interpret payload as a command.
                core::future::pending::<()>().await;
            }
            let tag: [u8; 4] = packet[4..8].try_into().unwrap();
            let expected = u32::from_le_bytes(packet[8..12].try_into().unwrap());
            let direction_in = packet[12] == 0x80;
            let c = &packet[15..31];
            let mut status = 0u8;
            let mut sent = 0u32;
            let mut reply = [0u8; 64];
            let mut len = 0usize;
            let mut read = None;
            let min_cdb = match c[0] {
                0x00 | 0x03 | 0x12 | 0x1a | 0x1b | 0x1e => 6,
                0xa0 => 12,
                _ => 10,
            };
            if packet[13] != 0 {
                sense = [5, 0x25, 0];
                status = 1;
            } else if packet[14] < min_cdb {
                sense = [5, 0x24, 0];
                status = 1;
            } else {
                match c[0] {
                    0x00 if ejected => {
                        sense = [2, 0x3a, 0];
                        status = 1;
                    }
                    0x00 | 0x1e | 0x35 => {} // ready, prevent/allow, sync
                    0x1b => {
                        if c[4] & 1 != 0 {
                            ejected = false;
                        } else if c[4] & 2 != 0 {
                            ejected = true;
                        }
                    }
                    0x03 => {
                        // fixed-format REQUEST SENSE
                        reply[0] = 0x70;
                        reply[2] = sense[0];
                        reply[7] = 10;
                        reply[12] = sense[1];
                        reply[13] = sense[2];
                        len = 18.min(c[4] as usize);
                        sense = [0; 3];
                    }
                    0x12 if c[1] & 1 == 0 => {
                        reply[..8].copy_from_slice(&[0, 0x80, 4, 2, 31, 0, 0, 0]);
                        reply[8..16].copy_from_slice(b"PCMON   ");
                        reply[16..32].copy_from_slice(b"Host tools disk ");
                        reply[32..36].copy_from_slice(b"0001");
                        len = 36.min(c[4] as usize);
                    }
                    0x1a => {
                        reply[..4].copy_from_slice(&[3, 0, 0x80, 0]);
                        len = 4.min(c[4] as usize);
                    }
                    0x5a => {
                        reply[1] = 6;
                        reply[3] = 0x80;
                        len = 8.min(u16::from_be_bytes([c[7], c[8]]) as usize);
                    }
                    0x25 => {
                        reply[..4]
                            .copy_from_slice(&((IMAGE.len() / BLOCK - 1) as u32).to_be_bytes());
                        reply[4..8].copy_from_slice(&(BLOCK as u32).to_be_bytes());
                        len = 8;
                    }
                    0x23 => {
                        reply[3] = 8;
                        reply[4..8].copy_from_slice(&((IMAGE.len() / BLOCK) as u32).to_be_bytes());
                        reply[8] = 2;
                        reply[10] = 2;
                        len = 12.min(u16::from_be_bytes([c[7], c[8]]) as usize);
                    }
                    0xa0 => {
                        reply[3] = 8;
                        len = 16.min(u32::from_be_bytes(c[6..10].try_into().unwrap()) as usize);
                    }
                    0x28 if ejected => {
                        sense = [2, 0x3a, 0];
                        status = 1;
                    }
                    0x28 => {
                        let lba = u32::from_be_bytes(c[2..6].try_into().unwrap()) as usize;
                        let count = u16::from_be_bytes([c[7], c[8]]) as usize;
                        if lba > IMAGE.len() / BLOCK || count > IMAGE.len() / BLOCK - lba {
                            sense = [5, 0x21, 0];
                            status = 1;
                        } else {
                            read = Some(&IMAGE[lba * BLOCK..(lba + count) * BLOCK]);
                        }
                    }
                    0x2a | 0x2e | 0xaa | 0x8a => {
                        sense = [7, 0x27, 0];
                        status = 1;
                    }
                    _ => {
                        sense = [5, 0x20, 0];
                        status = 1;
                    }
                }
            }
            let data = read.unwrap_or(&reply[..len]);
            if direction_in {
                if status == 0 {
                    let bytes = data.len().min(expected as usize);
                    for chunk in data[..bytes].chunks(64) {
                        self.input.write(chunk).await?;
                    }
                    sent = bytes as u32;
                    if expected < data.len() as u32 {
                        status = 2;
                    }
                }
                if sent < expected && sent.is_multiple_of(64) {
                    self.input.write(&[]).await?;
                }
            } else if expected > 0 {
                // Consume rejected writes so the next CBW remains packet-aligned.
                let mut remaining = expected;
                while remaining > 0 {
                    let got = self.output.read(&mut packet).await?;
                    remaining = remaining.saturating_sub(got as u32);
                    if got < 64 {
                        break;
                    }
                }
                if status == 0 {
                    status = 2;
                }
            } else if !data.is_empty() {
                status = 2;
            }
            // Tag is saved separately because draining OUT overwrites packet.
            let mut csw = [0u8; 13];
            csw[..4].copy_from_slice(b"USBS");
            csw[4..8].copy_from_slice(&tag);
            csw[8..12].copy_from_slice(&expected.saturating_sub(sent).to_le_bytes());
            csw[12] = status;
            self.input.write(&csw).await?;
        }
    }
}

#[cfg(test)]
#[path = "../../tools/usb-check/storage_tests.rs"]
mod tests;
