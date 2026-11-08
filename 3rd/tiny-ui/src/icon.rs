//! Allocation-free, pre-rasterized icon masks. SVG decoding is an offline concern.
use embedded_graphics::{pixelcolor::Rgb565, prelude::*};

/// An 8-bit coverage mask in row-major order, one byte per pixel.
/// Short masks safely leave the missing pixels unchanged.
pub struct AlphaIcon<'a> {
    alpha: &'a [u8],
    size: Size,
}
impl<'a> AlphaIcon<'a> {
    pub const fn new(alpha: &'a [u8], size: Size) -> Self {
        Self { alpha, size }
    }
    /// Draw against a known solid background, preserving antialiased edges.
    pub fn draw<D: DrawTarget<Color = Rgb565>>(
        &self,
        target: &mut D,
        position: Point,
        foreground: Rgb565,
        background: Rgb565,
    ) -> Result<(), D::Error> {
        if self.size.width == 0 {
            return Ok(());
        }
        let count = (self.size.width as u64 * self.size.height as u64).min(self.alpha.len() as u64)
            as usize;
        let blend = |f: u8, b: u8, a: u8| {
            ((f as u32 * a as u32 + b as u32 * (255 - a as u32) + 127) / 255) as u8
        };
        target.draw_iter(
            self.alpha[..count]
                .iter()
                .enumerate()
                .filter_map(|(i, &a)| {
                    if a == 0 {
                        return None;
                    }
                    let x = position.x as i64 + (i as u64 % self.size.width as u64) as i64;
                    let y = position.y as i64 + (i as u64 / self.size.width as u64) as i64;
                    let point = Point::new(i32::try_from(x).ok()?, i32::try_from(y).ok()?);
                    Some(Pixel(
                        point,
                        Rgb565::new(
                            blend(foreground.r(), background.r(), a),
                            blend(foreground.g(), background.g(), a),
                            blend(foreground.b(), background.b(), a),
                        ),
                    ))
                }),
        )
    }
}
