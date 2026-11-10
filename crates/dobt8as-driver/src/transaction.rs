//! Async command transactions. The caller owns deadlines, UART configuration and input draining.
use embedded_io_async::{Read, Write};
#[derive(Debug, PartialEq, Eq)]
pub enum TransactionError<E> {
    Driver(crate::Error),
    Io(E),
    UnexpectedEof,
    Device(u16),
}
// Dropping a timed-out future must not leave the parser busy. This does not undo
// bytes already written; the caller must quiesce/drain before another transaction.
struct Transaction<'a>(&'a mut crate::Driver);
impl Drop for Transaction<'_> {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

use crate::{Command, Driver, Error, Event, Reply};
impl Driver {
    /// Send a command and await its matching reply. SetBaud requires send_command
    /// because the caller must reconfigure UART before receiving at the new rate.
    pub async fn execute<T: Read + Write>(
        &mut self,
        io: &mut T,
        cmd: Command<'_>,
    ) -> Result<Reply, TransactionError<T::Error>> {
        if matches!(cmd, Command::SetBaud(_)) {
            return Err(TransactionError::Driver(Error::UseSendCommand));
        }
        let mut tx = [0; 64];
        let n = self.begin(cmd, &mut tx).map_err(TransactionError::Driver)?;
        let guard = Transaction(self);
        io.write_all(&tx[..n]).await.map_err(TransactionError::Io)?;
        io.flush().await.map_err(TransactionError::Io)?;
        let mut byte = [0; 1];
        loop {
            if io.read(&mut byte).await.map_err(TransactionError::Io)? == 0 {
                return Err(TransactionError::UnexpectedEof);
            }
            match guard.0.feed(byte[0]) {
                Some(Event::Complete(r)) => return Ok(r),
                Some(Event::DeviceError(e)) => return Err(TransactionError::Device(e)),
                Some(Event::ParseError(e)) => return Err(TransactionError::Driver(e)),
                _ => {}
            }
        }
    }
    /// Write and flush without waiting for a reply (e.g. immediate BAUD change).
    /// Success means only local transmission; caller must verify the module later.
    pub async fn send_command<T: Write>(
        &mut self,
        io: &mut T,
        cmd: Command<'_>,
    ) -> Result<(), TransactionError<T::Error>> {
        let mut tx = [0; 64];
        let n = self.begin(cmd, &mut tx).map_err(TransactionError::Driver)?;
        let _guard = Transaction(self);
        io.write_all(&tx[..n]).await.map_err(TransactionError::Io)?;
        io.flush().await.map_err(TransactionError::Io)
    }
}
