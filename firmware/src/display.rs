//! Core-0 DMA consumer. Two unique mutable buffers circulate through the queues.
use crate::lcd::Lcd;
use dashboard::{FrameBytes, Rectangle};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};

pub struct FrameJob {
    pub frame: &'static mut FrameBytes,
    /// Current scene damage, excluding the back buffer's repair-only history.
    pub damage: Rectangle,
}
pub static READY: Channel<CriticalSectionRawMutex, FrameJob, 1> = Channel::new();
pub static FREE: Channel<CriticalSectionRawMutex, &'static mut FrameBytes, 1> = Channel::new();

#[embassy_executor::task]
pub async fn transfer(mut lcd: Lcd, initial: &'static mut FrameBytes) {
    lcd.init().await;
    FREE.send(initial).await;
    loop {
        let job = READY.receive().await;
        lcd.flush_region(job.frame, job.damage).await;
        FREE.send(job.frame).await;
    }
}
