//! Caller-provided bounded I/O and time for AT device drivers.
#![no_std]
/// One owner serializes AT and binary traffic. Implementations must honor I/O deadlines.
#[allow(async_fn_in_trait)]
pub trait Transport {
    type Error;
    fn now_ms(&self) -> u64;
    async fn set_baud(&mut self, baud: u32) -> Result<(), Self::Error>;
    /// Drain up to max_bytes, stopping after idle_ms without input.
    async fn drain(&mut self, idle_ms: u32, max_bytes: usize) -> Result<(), Self::Error>;
    /// Write the full command and flush within timeout_ms.
    async fn write(&mut self, bytes: &[u8], timeout_ms: u32) -> Result<(), Self::Error>;
    /// Return zero for timeout; transport errors are distinct from timeout.
    async fn read(&mut self, out: &mut [u8], timeout_ms: u32) -> Result<usize, Self::Error>;
    async fn delay_ms(&mut self, ms: u32);
}
