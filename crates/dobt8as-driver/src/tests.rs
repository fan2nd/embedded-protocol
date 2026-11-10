extern crate std;
use super::*;
fn feed(d: &mut Driver, bytes: &[u8]) -> std::vec::Vec<Event> {
    bytes.iter().filter_map(|b| d.feed(*b)).collect()
}
#[test]
fn value_and_ok_form_one_transaction_with_interleaved_urc() {
    let mut d = Driver::new();
    let mut tx = [0; 64];
    let n = d.begin(Command::Query(Field::Baud), &mut tx).unwrap();
    assert_eq!(&tx[..n], b"AT+BAUD?\r\n");
    assert!(feed(&mut d, b"+BAUD:115200\r\n").is_empty());
    assert_eq!(d.begin(Command::Reset, &mut tx), Err(Error::Busy));
    assert_eq!(feed(&mut d, b"+BLE:CONNECT\r\n"), [Event::Connected]);
    let events = feed(&mut d, b"OK\r\n");
    assert!(matches!(events.as_slice(),[Event::Complete(r)] if r.number()==Some(115200)));
}
#[test]
fn no_bare_ok_wrong_reply_or_overflow_success() {
    let mut d = Driver::new();
    d.begin(Command::Query(Field::Mtu), &mut [0; 64]).unwrap();
    assert!(matches!(
        feed(&mut d, b"+NAME:abc\r\n").as_slice(),
        [Event::Information(_)]
    ));
    assert_eq!(
        feed(&mut d, b"OK\r\n"),
        [Event::ParseError(Error::MissingValue)]
    );
    d.begin(Command::Query(Field::Mtu), &mut [0; 64]).unwrap();
    for _ in 0..129 {
        d.feed(b'x');
    }
    assert_eq!(d.feed(b'\n'), Some(Event::ParseError(Error::Overflow)));
    d.begin(Command::Query(Field::Mtu), &mut [0; 64]).unwrap();
    assert_eq!(feed(&mut d, b"ERR:4\r\n"), [Event::DeviceError(4)]);
}
#[test]
fn validates_ranges_and_encodes_address_and_uuid_without_guessing_01x_commands() {
    let mut d = Driver::new();
    let mut tx = [0; 64];
    for cmd in [
        Command::SetName("bad\r\nAT+RESET"),
        Command::SetMtu(248),
        Command::SetBaud(1000000),
        Command::SetAdvertisingInterval(19),
        Command::SetAutoDisconnect(4),
        Command::SetConnectionMode(3),
    ] {
        assert_eq!(d.begin(cmd, &mut tx), Err(Error::InvalidParameter));
    }
    let n = d
        .begin(Command::SetUuid([0xa000, 0xa001, 0xa002, 0xa003]), &mut tx)
        .unwrap();
    assert_eq!(&tx[..n], b"AT+UUID=A000,A001,A002,A003\r\n");
    let events = feed(&mut d, b"+UUID:A000,A001,A002,A003\r\nOK\r\n");
    assert!(
        matches!(events.as_slice(),[Event::Complete(r)] if r.uuids()==Some([0xa000,0xa001,0xa002,0xa003]))
    );
    let n = d
        .begin(
            Command::SetMac([0x56, 0x34, 0x12, 0xef, 0xcd, 0xab]),
            &mut tx,
        )
        .unwrap();
    assert_eq!(&tx[..n], b"AT+MAC=563412EFCDAB\r\n");
}
#[test]
fn cmd_off_exception_and_binary_gate_are_distinct() {
    let mut d = Driver::new();
    d.confirm_peer();
    assert_eq!(d.enter_transparent(), Err(Error::NotReady));
    d.begin(Command::SetCommandRecognition(false), &mut [0; 64])
        .unwrap();
    feed(&mut d, b"+CMD:0\r\nOK\r\n");
    assert_eq!(
        d.begin(Command::Reset, &mut [0; 64]),
        Err(Error::CommandsDisabled)
    );
    d.enter_transparent().unwrap();
    assert_eq!(
        d.begin(Command::Query(Field::CommandRecognition), &mut [0; 64]),
        Err(Error::Transparent)
    );
    assert!(feed(&mut d, b"+CMD:1\r\nOK\r\n").is_empty());
    d.resume_at_after_quiesce();
    d.begin(Command::SetCommandRecognition(true), &mut [0; 64])
        .unwrap();
    feed(&mut d, b"+CMD:1\r\nOK\r\n");
    d.begin(Command::Reset, &mut [0; 64]).unwrap();
}
use std::{collections::VecDeque, vec::Vec};
struct Mock {
    script: VecDeque<(&'static [u8], &'static [u8])>,
    rx: VecDeque<u8>,
    bauds: Vec<u32>,
    now: u64,
}
impl Transport for Mock {
    type Error = ();
    fn now_ms(&self) -> u64 {
        self.now
    }
    async fn set_baud(&mut self, b: u32) -> Result<(), ()> {
        self.bauds.push(b);
        Ok(())
    }
    async fn drain(&mut self, _: u32, _: usize) -> Result<(), ()> {
        self.rx.clear();
        Ok(())
    }
    async fn write(&mut self, b: &[u8], _: u32) -> Result<(), ()> {
        let (expected, r) = self.script.pop_front().unwrap();
        assert_eq!(b, expected);
        self.rx.extend(r);
        Ok(())
    }
    async fn read(&mut self, b: &mut [u8], timeout: u32) -> Result<usize, ()> {
        let n = self.rx.len().min(b.len()).min(5);
        for v in &mut b[..n] {
            *v = self.rx.pop_front().unwrap();
        }
        self.now += if n == 0 { timeout as u64 } else { 1 };
        Ok(n)
    }
    async fn delay_ms(&mut self, ms: u32) {
        self.now += ms as u64;
    }
}
fn run<F: core::future::Future>(f: F) -> F::Output {
    let mut f = core::pin::pin!(f);
    match f.as_mut().poll(&mut core::task::Context::from_waker(
        std::task::Waker::noop(),
    )) {
        core::task::Poll::Ready(v) => v,
        _ => panic!(),
    }
}
fn mock(script: &[(&'static [u8], &'static [u8])]) -> Mock {
    Mock {
        script: script.iter().copied().collect(),
        rx: VecDeque::new(),
        bauds: Vec::new(),
        now: 0,
    }
}
#[test]
fn baud_changes_without_old_rate_ok_and_requires_new_rate_readback() {
    let mut io = mock(&[
        (b"AT+BAUD=460800\r\n", b"garbled"),
        (b"AT+BAUD?\r\n", b"+BAUD:460800\r\nOK\r\n"),
    ]);
    let mut d = Driver::new();
    assert_eq!(
        run(d.execute(&mut io, Command::SetBaud(460800))),
        Err(setup::SetupError::Driver(Error::UseChangeBaud))
    );
    run(d.change_baud(&mut io, 460800)).unwrap();
    assert_eq!(io.bauds, [460800]);
    assert!(io.script.is_empty());
    let mut io = mock(&[
        (b"AT+BAUD=460800\r\n", b""),
        (b"AT+BAUD?\r\n", b"+BAUD:115200\r\nOK\r\n"),
    ]);
    assert_eq!(
        run(d.change_baud(&mut io, 460800)),
        Err(setup::SetupError::Readback(Field::Baud))
    );
}
#[test]
fn configure_verifies_name_baud_log_lowpower_and_cmd_without_claiming_connected() {
    let mut io = mock(&[
        (b"AT+BAUD?\r\n", b""),
        (b"AT+BAUD?\r\n", b""),
        (b"AT+BAUD?\r\n", b"+BAUD:115200\r\nOK\r\n"),
        (b"AT+NAME?\r\n", b"+NAME:demo\r\nOK\r\n"),
        (b"AT+LOG=0\r\n", b"+LOG:0\r\nOK\r\n"),
        (b"AT+LOWPOWER=0\r\n", b"+LOWPOWER:0\r\nOK\r\n"),
        (b"AT+RESET\r\n", b"+RESET\r\nOK\r\n"),
        (b"AT+BAUD?\r\n", b"+BAUD:115200\r\nOK\r\n"),
        (b"AT+NAME?\r\n", b"+NAME:demo\r\nOK\r\n"),
        (b"AT+LOG?\r\n", b"+LOG:0\r\nOK\r\n"),
        (b"AT+LOWPOWER=0\r\n", b"+LOWPOWER:0\r\nOK\r\n"),
        (b"AT+LOWPOWER?\r\n", b"+LOWPOWER:0\r\nOK\r\n"),
        (b"AT+CMD=0\r\n", b"+CMD:0\r\nOK\r\n"),
        (b"AT+CMD?\r\n", b"+CMD:0\r\nOK\r\n"),
    ]);
    let mut d = Driver::new();
    let ready = run(d.configure(
        &mut io,
        setup::Options {
            name: "demo",
            baud: 115200,
            reset_wait_ms: 2000,
        },
    ))
    .unwrap();
    assert!(ready.connected_at_commands_disabled && ready.logs_disabled);
    assert!(io.script.is_empty());
    assert_eq!(d.enter_transparent(), Err(Error::NotReady));
}
