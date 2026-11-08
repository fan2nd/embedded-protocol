//! Big-endian RGB565 storage shared by the renderer and ST7789 DMA adapter.
use core::convert::Infallible;
use embedded_graphics::{pixelcolor::Rgb565, prelude::*, primitives::Rectangle};

pub const WIDTH: usize = 240;
pub const HEIGHT: usize = 135;
pub const FRAME_BYTES: usize = WIDTH * HEIGHT * 2;
pub type FrameBytes = [u8; FRAME_BYTES];

pub struct Frame<'a>(pub &'a mut FrameBytes);
impl OriginDimensions for Frame<'_> {
    fn size(&self) -> Size {
        Size::new(WIDTH as u32, HEIGHT as u32)
    }
}
impl DrawTarget for Frame<'_> {
    type Color = Rgb565;
    type Error = Infallible;
    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Rgb565>>,
    {
        for Pixel(p, color) in pixels {
            if self.bounding_box().contains(p) {
                let i = (p.y as usize * WIDTH + p.x as usize) * 2;
                self.0[i..i + 2].copy_from_slice(&color.into_storage().to_be_bytes());
            }
        }
        Ok(())
    }
    fn fill_solid(&mut self, area: &Rectangle, color: Rgb565) -> Result<(), Self::Error> {
        let clip = area.intersection(&self.bounding_box());
        if clip.size.width == 0 || clip.size.height == 0 {
            return Ok(());
        }
        let bytes = color.into_storage().to_be_bytes();
        for y in clip.top_left.y..clip.top_left.y + clip.size.height as i32 {
            let start = (y as usize * WIDTH + clip.top_left.x as usize) * 2;
            let end = start + clip.size.width as usize * 2;
            for px in self.0[start..end].as_chunks_mut::<2>().0 {
                px.copy_from_slice(&bytes);
            }
        }
        Ok(())
    }
}
