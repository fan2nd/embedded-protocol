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
