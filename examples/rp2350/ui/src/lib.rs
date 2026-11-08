#![no_std]
mod framebuffer;
mod page;
mod typography;

pub use embedded_graphics::primitives::Rectangle;
pub use framebuffer::{Frame, FrameBytes, FRAME_BYTES, HEIGHT, WIDTH};
pub use page::Dashboard;
