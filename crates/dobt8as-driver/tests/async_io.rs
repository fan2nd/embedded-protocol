use core::{
    future::Future,
    task::{Context, Poll},
};
use dobt8as_driver::{transaction::TransactionError, Command, Driver, Field};
use embedded_io_async::{ErrorKind, ErrorType, Read, Write};
use std::{collections::VecDeque, vec::Vec};
struct Io {
    tx: Vec<u8>,
    rx: VecDeque<u8>,
    flushed: bool,
    eof: bool,
    fail: bool,
}
impl ErrorType for Io {
    type Error = ErrorKind;
}
async fn yield_once() {
    let mut pending = true;
    core::future::poll_fn(|cx| {
        if pending {
            pending = false;
            cx.waker().wake_by_ref();
            Poll::Pending
        } else {
            Poll::Ready(())
        }
    })
    .await
}
impl Write for Io {
    async fn write(&mut self, bytes: &[u8]) -> Result<usize, ErrorKind> {
        yield_once().await;
        if self.fail {
            return Err(ErrorKind::Other);
        }
        let n = bytes.len().min(3);
        self.tx.extend_from_slice(&bytes[..n]);
        Ok(n)
    }
    async fn flush(&mut self) -> Result<(), ErrorKind> {
        yield_once().await;
        self.flushed = true;
        Ok(())
    }
}
impl Read for Io {
    async fn read(&mut self, out: &mut [u8]) -> Result<usize, ErrorKind> {
        assert!(self.flushed);
        yield_once().await;
        if let Some(b) = self.rx.pop_front() {
            out[0] = b;
            Ok(1)
        } else if self.eof {
            Ok(0)
        } else {
            core::future::pending().await
        }
    }
}
fn io(reply: &[u8]) -> Io {
    Io {
        tx: Vec::new(),
        rx: reply.iter().copied().collect(),
        flushed: false,
        eof: false,
        fail: false,
    }
}
fn run<F: Future>(f: F) -> F::Output {
    let mut f = core::pin::pin!(f);
    let mut cx = Context::from_waker(std::task::Waker::noop());
    for _ in 0..1000 {
        if let Poll::Ready(v) = f.as_mut().poll(&mut cx) {
            return v;
        }
    }
    panic!("future did not complete")
}
#[test]
fn pending_partial_io_and_reply_boundary() {
    let mut d = Driver::new();
    let mut io = io(b"+BAUD:115200\r\nOK\r\ntail");
    let reply = run(d.execute(&mut io, Command::Query(Field::Baud))).unwrap();
    assert_eq!(reply.number(), Some(115200));
    assert_eq!(io.tx, b"AT+BAUD?\r\n");
    assert_eq!(io.rx.into_iter().collect::<Vec<_>>(), b"tail");
}
#[test]
fn cancellation_during_write_flush_or_read_releases_transaction() {
    // One Pending per partial write, then flush, then read. Cancel at every stage.
    for polls in 1..=b"AT+BAUD?\r\n".len().div_ceil(3) + 2 {
        let mut d = Driver::new();
        let mut io = io(b"");
        {
            let mut f = core::pin::pin!(d.execute(&mut io, Command::Query(Field::Baud)));
            let mut cx = Context::from_waker(std::task::Waker::noop());
            for _ in 0..polls {
                assert!(f.as_mut().poll(&mut cx).is_pending());
            }
        }
        assert!(d.begin(Command::Query(Field::Baud), &mut [0; 64]).is_ok());
    }
}
#[test]
fn eof_and_io_errors_are_not_timeouts_and_release_transaction() {
    let mut d = Driver::new();
    let mut io = io(b"");
    io.eof = true;
    assert_eq!(
        run(d.execute(&mut io, Command::Query(Field::Baud))),
        Err(TransactionError::UnexpectedEof)
    );
    io.fail = true;
    assert_eq!(
        run(d.execute(&mut io, Command::Query(Field::Baud))),
        Err(TransactionError::Io(ErrorKind::Other))
    );
    assert!(d.begin(Command::Query(Field::Baud), &mut [0; 64]).is_ok());
}

#[test]
fn baud_submission_does_not_wait_or_reconfigure_uart() {
    let mut d = Driver::new();
    let mut io = io(b"");
    assert!(run(d.execute(&mut io, Command::SetBaud(460800))).is_err());
    assert!(io.tx.is_empty());
    run(d.send_command(&mut io, Command::SetBaud(460800))).unwrap();
    assert_eq!(io.tx, b"AT+BAUD=460800\r\n");
    assert!(io.flushed);
    // Caller changes UART, drains old-rate response, then verifies explicitly.
    io.rx.extend(b"+BAUD:460800\r\nOK\r\n");
    assert_eq!(
        run(d.execute(&mut io, Command::Query(Field::Baud)))
            .unwrap()
            .number(),
        Some(460800)
    );
}
