//! doBT-8AS slave V1.1.5 AT driver. Not the 8AM or 8AMS command set.
#![no_std]
use core::fmt::Write;
pub const BAUDS: [u32; 15] = [
    115200, 1200, 2400, 4800, 9600, 14400, 19200, 38400, 56000, 57600, 128000, 230400, 256000,
    460800, 921600,
];
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Version,
    Name,
    Mac,
    Baud,
    CommandRecognition,
    Log,
    Mtu,
    Uuid,
    Advertising,
    AdvertisingInterval,
    ConnectionMode,
    LowPower,
    AutoDisconnect,
    Reset,
    PowerOff,
    Disconnect,
}
impl Field {
    fn key(self) -> &'static str {
        match self {
            Self::Version => "VERSION",
            Self::Name => "NAME",
            Self::Mac => "MAC",
            Self::Baud => "BAUD",
            Self::CommandRecognition => "CMD",
            Self::Log => "LOG",
            Self::Mtu => "MTU",
            Self::Uuid => "UUID",
            Self::Advertising => "ADV",
            Self::AdvertisingInterval => "ADVINTERVAL",
            Self::ConnectionMode => "CONNMODE",
            Self::LowPower => "LOWPOWER",
            Self::AutoDisconnect => "AUTODISCON",
            Self::Reset => "RESET",
            Self::PowerOff => "POWEROFF",
            Self::Disconnect => "DISC",
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub enum Command<'a> {
    Query(Field),
    SetName(&'a str),
    SetMac([u8; 6]),
    SetBaud(u32),
    SetCommandRecognition(bool),
    SetLog(bool),
    SetMtu(u16),
    SetUuid([u16; 4]),
    SetAdvertising(bool),
    SetAdvertisingInterval(u16),
    SetConnectionMode(u8),
    SetLowPower(bool),
    SetAutoDisconnect(u16),
    Reset,
    PowerOff,
    Disconnect,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Busy,
    Transparent,
    CommandsDisabled,
    InvalidParameter,
    BufferTooSmall,
    Overflow,
    MissingValue,
    NotReady,
    UseSendCommand,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reply {
    pub field: Field,
    bytes: [u8; 96],
    len: usize,
}
impl Reply {
    pub fn value(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
    pub fn number(&self) -> Option<u32> {
        decimal(self.value())
    }
    /// MAC bytes in the wire's little-endian order, not human display order.
    pub fn mac_le(&self) -> Option<[u8; 6]> {
        if self.field != Field::Mac {
            return None;
        }
        let mut out = [0; 6];
        for (b, p) in out.iter_mut().zip(self.value().chunks(2)) {
            *b = hex(p)? as u8;
        }
        Some(out)
    }
    pub fn uuids(&self) -> Option<[u16; 4]> {
        if self.field != Field::Uuid {
            return None;
        }
        let mut out = [0; 4];
        for (b, p) in out.iter_mut().zip(self.value().split(|b| *b == b',')) {
            *b = hex(p)?;
        }
        Some(out)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Complete(Reply),
    DeviceError(u16),
    ParseError(Error),
    Connected,
    Disconnected,
    Information(Reply),
}
pub struct Driver {
    pending: Option<Field>,
    reboot_on_complete: bool,
    candidate: Option<Reply>,
    line: [u8; 128],
    len: usize,
    dropping: bool,
    connected: bool,
    transparent: bool,
    at_enabled: Option<bool>,
}
impl Default for Driver {
    fn default() -> Self {
        Self::new()
    }
}
impl Driver {
    pub const fn new() -> Self {
        Self {
            pending: None,
            reboot_on_complete: false,
            candidate: None,
            line: [0; 128],
            len: 0,
            dropping: false,
            connected: false,
            transparent: false,
            at_enabled: None,
        }
    }
    pub fn cancel(&mut self) {
        self.pending = None;
        self.reboot_on_complete = false;
        self.candidate = None;
        self.len = 0;
        self.dropping = false;
    }
    pub fn disconnected(&mut self) {
        self.cancel();
        self.connected = false;
        self.transparent = false;
    }
    /// Owner has established and verified a binary peer connection.
    pub fn confirm_peer(&mut self) {
        self.connected = true;
    }
    pub fn enter_transparent(&mut self) -> Result<(), Error> {
        if self.pending.is_some() {
            return Err(Error::Busy);
        }
        if !self.connected || self.at_enabled != Some(false) {
            return Err(Error::NotReady);
        }
        self.transparent = true;
        Ok(())
    }
    /// Caller must first stop binary traffic and drain both directions.
    /// This changes only the local gate; it does not send an escape command.
    pub fn resume_at_after_quiesce(&mut self) {
        self.transparent = false;
        self.cancel();
    }
    pub fn begin(&mut self, cmd: Command<'_>, out: &mut [u8]) -> Result<usize, Error> {
        if self.transparent {
            return Err(Error::Transparent);
        }
        if self.pending.is_some() {
            return Err(Error::Busy);
        }
        let field = command_field(cmd)?;
        if self.connected && self.at_enabled == Some(false) && field != Field::CommandRecognition {
            return Err(Error::CommandsDisabled);
        }
        let mut w = Buffer { out, len: 0 };
        write!(w, "AT+{}", field.key()).map_err(|_| Error::BufferTooSmall)?;
        let result = match cmd {
            Command::Query(_) => w.write_str("?"),
            Command::SetName(n) => write!(w, "={n}"),
            Command::SetMac(mac) => {
                w.write_str("=").map_err(|_| Error::BufferTooSmall)?;
                for b in mac {
                    write!(w, "{b:02X}").map_err(|_| Error::BufferTooSmall)?;
                }
                Ok(())
            }
            Command::SetBaud(v) => write!(w, "={v}"),
            Command::SetCommandRecognition(v)
            | Command::SetLog(v)
            | Command::SetAdvertising(v)
            | Command::SetLowPower(v) => write!(w, "={}", u8::from(v)),
            Command::SetMtu(v)
            | Command::SetAdvertisingInterval(v)
            | Command::SetAutoDisconnect(v) => write!(w, "={v}"),
            Command::SetConnectionMode(v) => write!(w, "={v}"),
            Command::SetUuid(v) => {
                write!(w, "={:04X},{:04X},{:04X},{:04X}", v[0], v[1], v[2], v[3])
            }
            Command::Disconnect => w.write_str("=8"),
            Command::Reset | Command::PowerOff => Ok(()),
        };
        result.map_err(|_| Error::BufferTooSmall)?;
        w.write_str("\r\n").map_err(|_| Error::BufferTooSmall)?;
        self.cancel();
        self.pending = Some(field);
        self.reboot_on_complete = matches!(
            cmd,
            Command::Reset | Command::PowerOff | Command::SetConnectionMode(_)
        );
        Ok(w.len)
    }
    /// Feed only while owning an AT transaction channel; never feed binary payloads here.
    /// A parameter line is provisional until the matching trailing OK arrives.
    pub fn feed(&mut self, byte: u8) -> Option<Event> {
        if self.transparent {
            return None;
        }
        if byte != b'\n' {
            if self.len == self.line.len() {
                self.dropping = true;
            }
            if !self.dropping {
                self.line[self.len] = byte;
                self.len += 1;
            }
            return None;
        }
        if self.dropping {
            self.cancel();
            return Some(Event::ParseError(Error::Overflow));
        }
        let n = self.len;
        self.len = 0;
        let line = self.line[..n].strip_suffix(b"\r")?;
        if line == b"+BLE:CONNECT" {
            self.connected = true;
            return Some(Event::Connected);
        }
        if line == b"+BLE:DISCONNECT" {
            self.connected = false;
            if self.pending == Some(Field::Disconnect) {
                self.pending = None;
                return Some(Event::Complete(reply(Field::Disconnect, b"")?));
            }
            return Some(Event::Disconnected);
        }
        if let Some(v) = line
            .strip_prefix(b"ERR:")
            .and_then(decimal)
            .and_then(|v| u16::try_from(v).ok())
        {
            self.cancel();
            return Some(Event::DeviceError(v));
        }
        if line == b"OK" {
            self.pending?;
            let Some(r) = self.candidate.take() else {
                self.cancel();
                return Some(Event::ParseError(Error::MissingValue));
            };
            self.pending = None;
            if r.field == Field::CommandRecognition {
                self.at_enabled = Some(r.value() == b"1");
            }
            if self.reboot_on_complete {
                self.connected = false;
                self.at_enabled = None;
            }
            self.reboot_on_complete = false;
            return Some(Event::Complete(r));
        }
        let parsed = parse_line(line)?;
        if self.pending == Some(parsed.field) {
            self.candidate = Some(parsed);
            None
        } else {
            Some(Event::Information(parsed))
        }
    }
}
struct Buffer<'a> {
    out: &'a mut [u8],
    len: usize,
}
impl Write for Buffer<'_> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let end = self.len + s.len();
        if end > self.out.len() {
            return Err(core::fmt::Error);
        }
        self.out[self.len..end].copy_from_slice(s.as_bytes());
        self.len = end;
        Ok(())
    }
}
fn command_field(c: Command<'_>) -> Result<Field, Error> {
    Ok(match c {
        Command::Query(f) => {
            if matches!(f, Field::Reset | Field::PowerOff | Field::Disconnect) {
                return Err(Error::InvalidParameter);
            }
            f
        }
        Command::SetName(n) => {
            if n.is_empty() || n.len() > 20 || n.bytes().any(|b| b < 0x20 || b == 0x7f) {
                return Err(Error::InvalidParameter);
            }
            Field::Name
        }
        Command::SetMac(_) => Field::Mac,
        Command::SetBaud(v) => {
            if !BAUDS.contains(&v) {
                return Err(Error::InvalidParameter);
            }
            Field::Baud
        }
        Command::SetCommandRecognition(_) => Field::CommandRecognition,
        Command::SetLog(_) => Field::Log,
        Command::SetMtu(v) => {
            if !(23..=247).contains(&v) {
                return Err(Error::InvalidParameter);
            }
            Field::Mtu
        }
        Command::SetUuid(_) => Field::Uuid,
        Command::SetAdvertising(_) => Field::Advertising,
        Command::SetAdvertisingInterval(v) => {
            if !(20..=10240).contains(&v) {
                return Err(Error::InvalidParameter);
            }
            Field::AdvertisingInterval
        }
        Command::SetConnectionMode(v) => {
            if v > 2 {
                return Err(Error::InvalidParameter);
            }
            Field::ConnectionMode
        }
        Command::SetLowPower(_) => Field::LowPower,
        Command::SetAutoDisconnect(v) => {
            if v != 0 && !(5..=3600).contains(&v) {
                return Err(Error::InvalidParameter);
            }
            Field::AutoDisconnect
        }
        Command::Reset => Field::Reset,
        Command::PowerOff => Field::PowerOff,
        Command::Disconnect => Field::Disconnect,
    })
}
fn decimal(b: &[u8]) -> Option<u32> {
    if b.is_empty() {
        return None;
    }
    b.iter().try_fold(0u32, |v, b| {
        if b.is_ascii_digit() {
            v.checked_mul(10)?.checked_add((b - b'0') as u32)
        } else {
            None
        }
    })
}
fn hex(b: &[u8]) -> Option<u16> {
    if b.is_empty() || b.len() > 4 {
        return None;
    }
    b.iter().try_fold(0u16, |v, b| {
        let n = match b {
            b'0'..=b'9' => b - b'0',
            b'a'..=b'f' => b - b'a' + 10,
            b'A'..=b'F' => b - b'A' + 10,
            _ => return None,
        };
        v.checked_mul(16)?.checked_add(n as u16)
    })
}
fn reply(field: Field, value: &[u8]) -> Option<Reply> {
    if value.len() > 96 {
        return None;
    }
    let mut r = Reply {
        field,
        bytes: [0; 96],
        len: value.len(),
    };
    r.bytes[..value.len()].copy_from_slice(value);
    Some(r)
}
fn parse_line(line: &[u8]) -> Option<Reply> {
    if line == b"+RESET" {
        return reply(Field::Reset, b"");
    }
    if line == b"+POWEROFF" {
        return reply(Field::PowerOff, b"");
    }
    let line = line.strip_prefix(b"+")?;
    let at = line.iter().position(|b| *b == b':')?;
    let key = &line[..at];
    let v = &line[at + 1..];
    let fields = [
        Field::Version,
        Field::Name,
        Field::Mac,
        Field::Baud,
        Field::CommandRecognition,
        Field::Log,
        Field::Mtu,
        Field::Uuid,
        Field::Advertising,
        Field::AdvertisingInterval,
        Field::ConnectionMode,
        Field::LowPower,
        Field::AutoDisconnect,
    ];
    let field = fields.into_iter().find(|f| f.key().as_bytes() == key)?;
    let valid = match field {
        Field::Version => !v.is_empty() && v.len() <= 96,
        Field::Name => !v.is_empty() && v.len() <= 20 && core::str::from_utf8(v).is_ok(),
        Field::Mac => v.len() == 12 && v.chunks(2).all(|b| hex(b).is_some()),
        Field::Uuid => {
            v.len() == 19
                && v.split(|b| *b == b',').count() == 4
                && v.split(|b| *b == b',')
                    .all(|b| b.len() == 4 && hex(b).is_some())
        }
        Field::Baud => decimal(v).is_some_and(|v| BAUDS.contains(&v)),
        Field::CommandRecognition | Field::Log | Field::Advertising | Field::LowPower => {
            matches!(v, b"0" | b"1")
        }
        Field::Mtu => decimal(v).is_some_and(|v| (23..=247).contains(&v)),
        Field::AdvertisingInterval => decimal(v).is_some_and(|v| (20..=10240).contains(&v)),
        Field::ConnectionMode => decimal(v).is_some_and(|v| v <= 2),
        Field::AutoDisconnect => decimal(v).is_some_and(|v| v == 0 || (5..=3600).contains(&v)),
        _ => false,
    };
    valid.then(|| reply(field, v)).flatten()
}
pub mod transaction;

#[cfg(test)]
mod tests;
