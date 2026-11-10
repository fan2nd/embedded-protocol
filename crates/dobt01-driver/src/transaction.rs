//! Async command transactions. The caller owns deadlines, UART configuration and input draining.
use embedded_io_async::{Read, Write};
#[derive(Debug, PartialEq, Eq)]
pub enum TransactionError<E> {
    Driver(crate::Error),
    Io(E),
    UnexpectedEof,
}
// Dropping a timed-out future must not leave the parser busy. This does not undo
// bytes already written; the caller must quiesce/drain before another transaction.
struct Transaction<'a>(&'a mut crate::Driver);
impl Drop for Transaction<'_> {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

use crate::{Command, Driver, Response};
impl Driver {
    pub async fn execute<T: Read + Write>(
        &mut self,
        io: &mut T,
        cmd: Command,
    ) -> Result<Response, TransactionError<T::Error>> {
        let mut tx = [0; 32];
        let n = self.begin(cmd, &mut tx).map_err(TransactionError::Driver)?;
        self.exchange(io, &tx[..n]).await
    }
    pub async fn set_name<T: Read + Write>(
        &mut self,
        io: &mut T,
        name: &str,
    ) -> Result<Response, TransactionError<T::Error>> {
        let mut tx = [0; 32];
        let n = self
            .begin_name(name, &mut tx)
            .map_err(TransactionError::Driver)?;
        self.exchange(io, &tx[..n]).await
    }
    async fn exchange<T: Read + Write>(
        &mut self,
        io: &mut T,
        tx: &[u8],
    ) -> Result<Response, TransactionError<T::Error>> {
        let guard = Transaction(self);
        io.write_all(tx).await.map_err(TransactionError::Io)?;
        io.flush().await.map_err(TransactionError::Io)?;
        // Read one byte so the caller retains any bytes after the matched reply.
        let mut byte = [0; 1];
        loop {
            if io.read(&mut byte).await.map_err(TransactionError::Io)? == 0 {
                return Err(TransactionError::UnexpectedEof);
            }
            if let Some(reply) = guard.0.feed(byte[0]) {
                return Ok(reply);
            }
        }
    }
}
