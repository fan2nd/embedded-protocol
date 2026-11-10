use core::{
    future::Future,
    task::{Context, Poll},
};
use dobt01_driver::{transaction::TransactionError, Command, Driver, Response};
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
    let mut io = io(b"QT+05\r\ntail");
    let reply = run(d.execute(&mut io, Command::Baud)).unwrap();
    assert_eq!(reply, Response::Baud(115200));
    assert_eq!(io.tx, b"AT+QT\r\n");
    assert_eq!(io.rx.into_iter().collect::<Vec<_>>(), b"tail");
}
#[test]
fn cancellation_during_write_flush_or_read_releases_transaction() {
    // One Pending per partial write, then flush, then read. Cancel at every stage.
    for polls in 1..=b"AT+QT\r\n".len().div_ceil(3) + 2 {
        let mut d = Driver::new();
        let mut io = io(b"");
        {
            let mut f = core::pin::pin!(d.execute(&mut io, Command::Baud));
            let mut cx = Context::from_waker(std::task::Waker::noop());
            for _ in 0..polls {
                assert!(f.as_mut().poll(&mut cx).is_pending());
            }
        }
        assert!(d.begin(Command::Baud, &mut [0; 64]).is_ok());
    }
}
#[test]
fn eof_and_io_errors_are_not_timeouts_and_release_transaction() {
    let mut d = Driver::new();
    let mut io = io(b"");
    io.eof = true;
    assert_eq!(
        run(d.execute(&mut io, Command::Baud)),
        Err(TransactionError::UnexpectedEof)
    );
    io.fail = true;
    assert_eq!(
        run(d.execute(&mut io, Command::Baud)),
        Err(TransactionError::Io(ErrorKind::Other))
    );
    assert!(d.begin(Command::Baud, &mut [0; 64]).is_ok());
}
