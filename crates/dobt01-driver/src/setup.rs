//! Portable AT transactions and setup. Hardware and deadlines are supplied by the caller.
use crate::{Command, Driver, Error, Response};
pub const SCAN_BAUDS: [u32; 13] = [
    115200, 9600, 230400, 256000, 460800, 512000, 1000000, 57600, 38400, 19200, 31250, 2400, 4800,
];
pub use at_transport::Transport;
#[derive(Debug, PartialEq, Eq)]
pub enum SetupError<E> {
    Driver(Error),
    Transport(E),
    Timeout,
    NotFound,
    Unexpected(Response),
    BaudReadback,
    NameReadback,
}
pub struct Options<'a> {
    pub name: &'a str,
    pub baud: u32,
    pub ensure_slave: bool,
}
#[derive(Debug, PartialEq, Eq)]
pub struct Ready {
    pub detected_baud: u32,
    pub baud: u32,
    /// None means ER+2: unsupported, not evidence of slave/connected state.
    pub role: Option<u8>,
    pub connection: Option<u8>,
}
impl Driver {
    pub async fn execute<T: Transport>(
        &mut self,
        io: &mut T,
        cmd: Command,
    ) -> Result<Response, SetupError<T::Error>> {
        let mut tx = [0; 32];
        let n = self.begin(cmd, &mut tx).map_err(SetupError::Driver)?;
        self.exchange(io, &tx[..n]).await
    }
    pub async fn set_name<T: Transport>(
        &mut self,
        io: &mut T,
        name: &str,
    ) -> Result<Response, SetupError<T::Error>> {
        let mut tx = [0; 32];
        let n = self.begin_name(name, &mut tx).map_err(SetupError::Driver)?;
        self.exchange(io, &tx[..n]).await
    }
    async fn exchange<T: Transport>(
        &mut self,
        io: &mut T,
        tx: &[u8],
    ) -> Result<Response, SetupError<T::Error>> {
        let result = async {
            io.drain(20, 4096).await.map_err(SetupError::Transport)?;
            let start = io.now_ms();
            io.write(tx, 800).await.map_err(SetupError::Transport)?;
            let mut rx = [0; 128];
            loop {
                let elapsed = io.now_ms().saturating_sub(start);
                if elapsed >= 800 {
                    return Err(SetupError::Timeout);
                }
                let n = io
                    .read(&mut rx, (800 - elapsed) as u32)
                    .await
                    .map_err(SetupError::Transport)?;
                if n == 0 {
                    return Err(SetupError::Timeout);
                }
                for &b in &rx[..n] {
                    if let Some(reply) = self.feed(b) {
                        return Ok(reply);
                    }
                }
            }
        }
        .await;
        if result.is_err() {
            self.cancel();
        }
        result
    }
    /// Scan, configure, reset and verify; does not claim the BLE peer is connected.
    /// Call only while the application exclusively owns a quiescent AT connection.
    pub async fn configure<T: Transport>(
        &mut self,
        io: &mut T,
        options: Options<'_>,
    ) -> Result<Ready, SetupError<T::Error>> {
        // Validate configuration before any I/O; preserve the mode/busy gate.
        let mut validation = Driver::new();
        validation
            .begin(Command::SetBaud(options.baud), &mut [0; 32])
            .map_err(SetupError::Driver)?;
        validation.cancel();
        validation
            .begin_name(options.name, &mut [0; 32])
            .map_err(SetupError::Driver)?;
        self.begin(Command::Baud, &mut [0; 32])
            .map_err(SetupError::Driver)?;
        self.cancel();
        io.delay_ms(1000).await;
        let mut found = None;
        for baud in SCAN_BAUDS {
            io.set_baud(baud).await.map_err(SetupError::Transport)?;
            match self.execute(io, Command::Baud).await {
                Ok(Response::Baud(reported)) if reported == baud => {
                    found = Some(baud);
                    break;
                }
                Ok(_) | Err(SetupError::Timeout) => {}
                Err(e) => return Err(e),
            }
        }
        let detected_baud = found.ok_or(SetupError::NotFound)?;
        let mut role = match self.execute(io, Command::Role).await? {
            Response::Role(v) => Some(v),
            Response::ErrorCode(2) => None,
            r => return Err(SetupError::Unexpected(r)),
        };
        if options.ensure_slave && role == Some(1) {
            expect_ok(self.execute(io, Command::Slave).await?)?;
            io.delay_ms(2000).await;
            role = match self.execute(io, Command::Role).await? {
                Response::Role(0) => Some(0),
                r => return Err(SetupError::Unexpected(r)),
            };
        }
        let named = self.execute(io, Command::BleName).await? == Response::BleName
            && self.name_matches(options.name);
        if !named {
            expect_ok(self.set_name(io, options.name).await?)?;
        }
        if options.baud != detected_baud {
            expect_ok(self.execute(io, Command::SetBaud(options.baud)).await?)?;
        }
        io.delay_ms(1000).await;
        match self.execute(io, Command::Reset).await {
            Ok(Response::Ok) | Err(SetupError::Timeout) => {}
            Ok(r) => return Err(SetupError::Unexpected(r)),
            Err(e) => return Err(e),
        }
        io.set_baud(options.baud)
            .await
            .map_err(SetupError::Transport)?;
        io.delay_ms(2000).await;
        self.disconnected(); // Module reset was requested; readback below is mandatory.
        if self.execute(io, Command::Baud).await? != Response::Baud(options.baud) {
            return Err(SetupError::BaudReadback);
        }
        if self.execute(io, Command::BleName).await? != Response::BleName
            || !self.name_matches(options.name)
        {
            return Err(SetupError::NameReadback);
        }
        let connection = match self.execute(io, Command::Connection).await? {
            Response::Connection(v) => Some(v),
            Response::ErrorCode(2) => None,
            r => return Err(SetupError::Unexpected(r)),
        };
        Ok(Ready {
            detected_baud,
            baud: options.baud,
            role,
            connection,
        })
    }
    fn name_matches(&self, name: &str) -> bool {
        self.line().strip_prefix(b"TM+") == Some(name.as_bytes())
    }
}
fn expect_ok<E>(r: Response) -> Result<(), SetupError<E>> {
    if r == Response::Ok {
        Ok(())
    } else {
        Err(SetupError::Unexpected(r))
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::{collections::VecDeque, vec::Vec};
    struct Mock {
        script: VecDeque<(&'static [u8], &'static [u8])>,
        rx: VecDeque<u8>,
        bauds: Vec<u32>,
        waits: Vec<u32>,
        now: u64,
    }
    impl Transport for Mock {
        type Error = ();
        fn now_ms(&self) -> u64 {
            self.now
        }
        async fn set_baud(&mut self, baud: u32) -> Result<(), ()> {
            self.bauds.push(baud);
            Ok(())
        }
        async fn drain(&mut self, _: u32, _: usize) -> Result<(), ()> {
            self.rx.clear();
            Ok(())
        }
        async fn write(&mut self, bytes: &[u8], _: u32) -> Result<(), ()> {
            let (expected, response) = self.script.pop_front().unwrap();
            assert_eq!(bytes, expected);
            self.rx.extend(response);
            Ok(())
        }
        async fn read(&mut self, out: &mut [u8], timeout: u32) -> Result<usize, ()> {
            let n = self.rx.len().min(3).min(out.len());
            for b in &mut out[..n] {
                *b = self.rx.pop_front().unwrap();
            }
            self.now += if n == 0 { timeout as u64 } else { 1 };
            Ok(n)
        }
        async fn delay_ms(&mut self, ms: u32) {
            self.waits.push(ms);
            self.now += ms as u64;
        }
    }
    fn mock(script: &[(&'static [u8], &'static [u8])]) -> Mock {
        Mock {
            script: script.iter().copied().collect(),
            rx: VecDeque::new(),
            bauds: Vec::new(),
            waits: Vec::new(),
            now: 0,
        }
    }
    fn run<F: core::future::Future>(f: F) -> F::Output {
        let mut f = core::pin::pin!(f);
        let w = std::task::Waker::noop();
        match f.as_mut().poll(&mut core::task::Context::from_waker(w)) {
            core::task::Poll::Ready(v) => v,
            _ => panic!("mock must not suspend"),
        }
    }
    #[test]
    fn scans_configures_resets_and_checks_readback_without_inventing_connection() {
        let mut io = mock(&[
            (b"AT+QT\r\n", b""),
            (b"AT+QT\r\n", b"QT+01\r\n"),
            (b"AT+QS00\r\n", b"ER+2\r\n"),
            (b"AT+TM\r\n", b"TM+old\r\n"),
            (b"AT+BMexample\r\n", b"OK\r\n"),
            (b"AT+CT05\r\n", b"OK\r\n"),
            (b"AT+CZ\r\n", b"boot\r\n"),
            (b"AT+QT\r\n", b"QT+05\r\n"),
            (b"AT+TM\r\n", b"TM+example\r\n"),
            (b"AT+QC00\r\n", b"ER+2\r\n"),
        ]);
        let mut d = Driver::new();
        let r = run(d.configure(
            &mut io,
            Options {
                name: "example",
                baud: 115200,
                ensure_slave: true,
            },
        ))
        .unwrap();
        assert_eq!(
            r,
            Ready {
                detected_baud: 9600,
                baud: 115200,
                role: None,
                connection: None
            }
        );
        assert_eq!(io.bauds, [115200, 9600, 115200]);
        assert_eq!(io.waits, [1000, 1000, 2000]);
        assert!(io.script.is_empty());
        assert_eq!(d.enter_transparent(), Err(Error::NotConnected));
    }
    #[test]
    fn wrong_post_reset_baud_is_not_success() {
        let mut io = mock(&[
            (b"AT+QT\r\n", b"QT+05\r\n"),
            (b"AT+QS00\r\n", b"ER+2\r\n"),
            (b"AT+TM\r\n", b"TM+example\r\n"),
            (b"AT+CZ\r\n", b"OK\r\n"),
            (b"AT+QT\r\n", b"QT+09\r\n"),
        ]);
        assert_eq!(
            run(Driver::new().configure(
                &mut io,
                Options {
                    name: "example",
                    baud: 115200,
                    ensure_slave: true
                }
            )),
            Err(SetupError::BaudReadback)
        );
    }
    #[test]
    fn timeout_releases_transaction_and_transparent_gate_prevents_io() {
        let mut io = mock(&[
            (b"AT+QT\r\n", b""),
            (b"AT+QT\r\n", b"noise\r\nOK\r\nQT+05\r\n"),
        ]);
        let mut d = Driver::new();
        assert_eq!(
            run(d.execute(&mut io, Command::Baud)),
            Err(SetupError::Timeout)
        );
        assert_eq!(
            run(d.execute(&mut io, Command::Baud)),
            Ok(Response::Baud(115200))
        );
        d.confirm_peer();
        assert_eq!(
            run(d.execute(&mut io, Command::Reset)),
            Err(SetupError::Driver(Error::Transparent))
        );
    }
}
