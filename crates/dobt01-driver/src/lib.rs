//! doBT-01X V1.1 manual: CRLF AT commands; default 115200 8N1, no flow control.
//! A nonblocking command engine. The caller supplies UART I/O and deadlines.
#![no_std]
pub mod setup;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Baud,
    BleName,
    SetBleName,
    Role,
    Connection,
    Slave,
    Reset,
    SetBaud(u32),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Response {
    Baud(u32),
    BleName,
    Role(u8),
    Connection(u8),
    Ok,
    Error,
    ErrorCode(u16),
    Overflow,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Busy,
    Transparent,
    UnsupportedBaud,
    BufferTooSmall,
    NotConnected,
    InvalidName,
}

pub struct Driver {
    pending: Option<Command>,
    transparent: bool,
    connected: bool,
    line: [u8; 128],
    len: usize,
    last_len: usize,
    dropping: bool,
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
            transparent: false,
            connected: false,
            line: [0; 128],
            len: 0,
            last_len: 0,
            dropping: false,
        }
    }
    pub fn begin(&mut self, command: Command, out: &mut [u8]) -> Result<usize, Error> {
        if self.transparent {
            return Err(Error::Transparent);
        }
        if self.pending.is_some() {
            return Err(Error::Busy);
        }
        let bytes: &[u8] = match command {
            Command::Baud => b"AT+QT\r\n",
            Command::BleName => b"AT+TM\r\n",
            Command::Role => b"AT+QS00\r\n",
            Command::Connection => b"AT+QC00\r\n",
            Command::Slave => b"AT+MS00\r\n",
            Command::Reset => b"AT+CZ\r\n",
            Command::SetBleName => return Err(Error::InvalidName),
            Command::SetBaud(9600) => b"AT+CT01\r\n",
            Command::SetBaud(19200) => b"AT+CT02\r\n",
            Command::SetBaud(38400) => b"AT+CT03\r\n",
            Command::SetBaud(57600) => b"AT+CT04\r\n",
            Command::SetBaud(115200) => b"AT+CT05\r\n",
            Command::SetBaud(256000) => b"AT+CT06\r\n",
            Command::SetBaud(512000) => b"AT+CT07\r\n",
            Command::SetBaud(230400) => b"AT+CT08\r\n",
            Command::SetBaud(460800) => b"AT+CT09\r\n",
            Command::SetBaud(1000000) => b"AT+CT10\r\n",
            Command::SetBaud(31250) => b"AT+CT11\r\n",
            Command::SetBaud(2400) => b"AT+CT12\r\n",
            Command::SetBaud(4800) => b"AT+CT13\r\n",
            Command::SetBaud(_) => return Err(Error::UnsupportedBaud),
        };
        if out.len() < bytes.len() {
            return Err(Error::BufferTooSmall);
        }
        out[..bytes.len()].copy_from_slice(bytes);
        self.pending = Some(command);
        self.len = 0;
        self.last_len = 0;
        self.dropping = false;
        Ok(bytes.len())
    }
    /// Call after a timeout/I/O failure, then drain UART before the next command.
    pub fn cancel(&mut self) {
        self.pending = None;
        self.len = 0;
        self.dropping = false;
    }
    /// ASCII name policy is deliberately bounded; does not inject CR/LF into AT.
    pub fn begin_name(&mut self, name: &str, out: &mut [u8]) -> Result<usize, Error> {
        if self.transparent {
            return Err(Error::Transparent);
        }
        if self.pending.is_some() {
            return Err(Error::Busy);
        }
        if name.is_empty()
            || name.len() > 20
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(Error::InvalidName);
        }
        let n = 5 + name.len() + 2;
        if out.len() < n {
            return Err(Error::BufferTooSmall);
        }
        out[..5].copy_from_slice(b"AT+BM");
        out[5..n - 2].copy_from_slice(name.as_bytes());
        out[n - 2..n].copy_from_slice(b"\r\n");
        self.pending = Some(Command::SetBleName);
        self.len = 0;
        self.last_len = 0;
        self.dropping = false;
        Ok(n)
    }
    pub fn line(&self) -> &[u8] {
        &self.line[..self.last_len]
    }
    pub fn enter_transparent(&mut self) -> Result<(), Error> {
        if self.pending.is_some() {
            return Err(Error::Busy);
        }
        if !self.connected {
            return Err(Error::NotConnected);
        }
        self.transparent = true;
        Ok(())
    }
    /// A verified binary peer handshake is also proof of a transparent connection.
    pub fn confirm_peer(&mut self) {
        self.cancel();
        self.connected = true;
        self.transparent = true;
    }
    /// Only call after the external owner has disconnected and drained the link.
    pub fn disconnected(&mut self) {
        *self = Self::new();
    }
    pub fn feed(&mut self, byte: u8) -> Option<Response> {
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
            return Some(Response::Overflow);
        }
        let len = self.len;
        self.len = 0;
        let line = self.line[..len].strip_suffix(b"\r")?;
        self.last_len = line.len();
        let response = parse_response(line)?;
        let matches = match (self.pending, response) {
            (_, Response::Error | Response::ErrorCode(_)) => self.pending.is_some(),
            (Some(Command::Baud), Response::Baud(_))
            | (Some(Command::BleName), Response::BleName)
            | (Some(Command::Role), Response::Role(_))
            | (Some(Command::Connection), Response::Connection(_)) => true,
            (
                Some(Command::Slave | Command::Reset | Command::SetBaud(_) | Command::SetBleName),
                Response::Ok,
            ) => true,
            _ => false,
        };
        if !matches {
            return None;
        }
        self.pending = None;
        if let Response::Connection(state) = response {
            self.connected = state == 2 || state == 3;
        }
        Some(response)
    }
}
pub fn parse_response(line: &[u8]) -> Option<Response> {
    match line {
        b"OK" => Some(Response::Ok),
        b"ERROR" => Some(Response::Error),
        b"QS00" => Some(Response::Role(0)),
        b"QS01" => Some(Response::Role(1)),
        b"QC+00" => Some(Response::Connection(0)),
        b"QC+02" => Some(Response::Connection(2)),
        b"QC+03" => Some(Response::Connection(3)),
        _ if line.starts_with(b"TM+") && line.len() > 3 => Some(Response::BleName),
        _ if line.starts_with(b"ER+") => {
            let digits = &line[3..];
            if digits.is_empty() {
                return None;
            }
            let mut code = 0u16;
            for &byte in digits {
                if !byte.is_ascii_digit() {
                    return None;
                }
                code = code.checked_mul(10)?.checked_add((byte - b'0') as u16)?;
            }
            Some(Response::ErrorCode(code))
        }
        _ => {
            let digits = line.strip_prefix(b"QT+")?;
            if digits.is_empty() {
                return None;
            }
            let mut baud = 0u32;
            for &b in digits {
                if !b.is_ascii_digit() {
                    return None;
                }
                baud = baud.checked_mul(10)?.checked_add((b - b'0') as u32)?;
            }
            // Hardware returns QT+05, unlike the manual's QT+<baud> wording.
            if digits.len() == 2 {
                baud = match baud {
                    1 => 9600,
                    2 => 19200,
                    3 => 38400,
                    4 => 57600,
                    5 => 115200,
                    6 => 256000,
                    7 => 512000,
                    8 => 230400,
                    9 => 460800,
                    10 => 1000000,
                    11 => 31250,
                    12 => 2400,
                    13 => 4800,
                    _ => return None,
                };
            }
            Some(Response::Baud(baud))
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn matched_reply_and_mode_gate() {
        let mut d = Driver::new();
        let mut tx = [0; 32];
        assert_eq!(d.enter_transparent(), Err(Error::NotConnected));
        d.begin(Command::Connection, &mut tx).unwrap();
        for &b in b"OK\r\nQT+115200\r\n" {
            assert_eq!(d.feed(b), None);
        }
        let mut answer = None;
        for &b in b"QC+02\r\n" {
            answer = d.feed(b).or(answer);
        }
        assert_eq!(answer, Some(Response::Connection(2)));
        d.enter_transparent().unwrap();
        assert_eq!(d.begin(Command::Reset, &mut tx), Err(Error::Transparent));
    }
    #[test]
    fn overflow_and_baud_validation() {
        let mut d = Driver::new();
        d.begin(Command::Baud, &mut [0; 32]).unwrap();
        for _ in 0..129 {
            d.feed(b'X');
        }
        assert_eq!(d.feed(b'\n'), Some(Response::Overflow));
        assert_eq!(parse_response(b"QT+4294967296"), None);
        assert_eq!(parse_response(b"QT+115200"), Some(Response::Baud(115200)));
        assert_eq!(parse_response(b"QT+05"), Some(Response::Baud(115200)));
        assert_eq!(parse_response(b"QT+10"), Some(Response::Baud(1000000)));
        assert_eq!(parse_response(b"QT+99"), None);
        assert_eq!(parse_response(b"ER+2"), Some(Response::ErrorCode(2)));
        assert_eq!(parse_response(b"ER+65536"), None);
        assert_eq!(
            d.begin(Command::SetBaud(12345), &mut [0; 32]),
            Err(Error::UnsupportedBaud)
        );
    }
    #[test]
    fn name_encoding_and_readback() {
        let mut d = Driver::new();
        let mut tx = [0; 32];
        assert_eq!(
            d.begin_name("bad\r\nAT+CZ", &mut tx),
            Err(Error::InvalidName)
        );
        let n = d.begin_name("G431-BLE", &mut tx).unwrap();
        assert_eq!(&tx[..n], b"AT+BMG431-BLE\r\n");
        for &b in b"OK\r\n" {
            d.feed(b);
        }
        d.begin(Command::BleName, &mut tx).unwrap();
        for &b in b"TM+G431-BLE\r\n" {
            d.feed(b);
        }
        assert_eq!(d.line(), b"TM+G431-BLE");
    }
}
