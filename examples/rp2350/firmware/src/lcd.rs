//! ST7789 240x135 landscape, RGB565, software reset (RESET is board-wide).
use embassy_rp::{
    gpio::Output,
    peripherals::SPI1,
    spi::{Async, Spi},
};
use embassy_time::Timer;
use rp2350_dashboard::{FrameBytes, Rectangle, HEIGHT, WIDTH};

// Panel-specific tuning lives here. Opposite landscape may need offsets 40/52.
const X_OFFSET: u16 = 40;
const Y_OFFSET: u16 = 53;
const MADCTL: u8 = 0x60; // MX | MV; RGB (set bit 3 for BGR panels)
const INVERT: bool = true;

pub struct Lcd {
    spi: Spi<'static, SPI1, Async>,
    cs: Output<'static>,
    dc: Output<'static>,
    backlight: Output<'static>,
}
impl Lcd {
    pub fn new(
        spi: Spi<'static, SPI1, Async>,
        cs: Output<'static>,
        dc: Output<'static>,
        backlight: Output<'static>,
    ) -> Self {
        Self {
            spi,
            cs,
            dc,
            backlight,
        }
    }
    fn command(&mut self, cmd: u8, data: &[u8]) {
        self.cs.set_low();
        self.dc.set_low();
        self.spi.blocking_write(&[cmd]).unwrap();
        self.dc.set_high();
        if !data.is_empty() {
            self.spi.blocking_write(data).unwrap();
        }
        self.cs.set_high();
    }
    pub async fn init(&mut self) {
        Timer::after_millis(150).await;
        self.command(0x01, &[]); // SWRESET
        Timer::after_millis(150).await;
        self.command(0x11, &[]); // SLPOUT
        Timer::after_millis(120).await;
        self.command(0x3a, &[0x55]);
        self.command(0x36, &[MADCTL]);
        self.command(0xb2, &[0x0c, 0x0c, 0x00, 0x33, 0x33]);
        self.command(0xb7, &[0x35]);
        self.command(0xbb, &[0x19]);
        self.command(0xc0, &[0x2c]);
        self.command(0xc2, &[0x01]);
        self.command(0xc3, &[0x12]);
        self.command(0xc4, &[0x20]);
        self.command(0xc6, &[0x0f]);
        self.command(0xd0, &[0xa4, 0xa1]);
        self.command(if INVERT { 0x21 } else { 0x20 }, &[]);
        self.command(0x13, &[]);
        self.command(0x29, &[]);
        Timer::after_millis(20).await;
    }
    pub async fn flush_region(&mut self, frame: &FrameBytes, region: Rectangle) {
        let x = region.top_left.x;
        let y = region.top_left.y;
        let width = region.size.width as usize;
        let height = region.size.height as usize;
        assert!(x >= 0 && y >= 0 && width > 0 && height > 0);
        let (x, y) = (x as usize, y as usize);
        assert!(x + width <= WIDTH && y + height <= HEIGHT);
        let x1 = (X_OFFSET + x as u16).to_be_bytes();
        let x2 = (X_OFFSET + (x + width - 1) as u16).to_be_bytes();
        let y1 = (Y_OFFSET + y as u16).to_be_bytes();
        let y2 = (Y_OFFSET + (y + height - 1) as u16).to_be_bytes();
        self.command(0x2a, &[x1[0], x1[1], x2[0], x2[1]]);
        self.command(0x2b, &[y1[0], y1[1], y2[0], y2[1]]);
        self.cs.set_low();
        self.dc.set_low();
        self.spi.blocking_write(&[0x2c]).unwrap();
        self.dc.set_high();
        // A subrectangle has full-frame row stride. Do not send gaps between rows.
        if x == 0 && width == WIDTH {
            self.spi
                .write(&frame[y * WIDTH * 2..(y + height) * WIDTH * 2])
                .await
                .unwrap();
        } else {
            for row in y..y + height {
                let start = (row * WIDTH + x) * 2;
                self.spi
                    .write(&frame[start..start + width * 2])
                    .await
                    .unwrap();
            }
        }
        // Embassy's DMA write also waits for SPI BSY to clear before returning.
        self.cs.set_high();
        self.backlight.set_low();
    }
}
