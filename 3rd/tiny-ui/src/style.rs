//! Palette and typography shared by components.
pub use embedded_graphics::text::Alignment;
use embedded_graphics::{pixelcolor::Rgb565, prelude::RgbColor};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FontSize {
    /// Compact labels on small SPI panels.
    Small,
    Normal,
    Large,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Style {
    pub background: Rgb565,
    pub foreground: Rgb565,
    pub accent: Rgb565,
    pub border: Rgb565,
    /// Normal control outline, drawn strictly inside widget bounds.
    pub border_width: u32,
    pub pressed: Rgb565,
    pub disabled: Rgb565,
    pub radius: u32,
    pub padding: u32,
    pub font: FontSize,
    pub align: Alignment,
    pub alternate: Rgb565,
}
impl Default for Style {
    fn default() -> Self {
        Self {
            background: Rgb565::WHITE,
            foreground: Rgb565::new(4, 12, 20),
            accent: Rgb565::new(5, 35, 29),
            border: Rgb565::new(14, 38, 23),
            border_width: 1,
            pressed: Rgb565::new(23, 54, 31),
            disabled: Rgb565::new(22, 44, 22),
            radius: 8,
            padding: 8,
            font: FontSize::Normal,
            align: Alignment::Left,
            alternate: Rgb565::new(29, 61, 31),
        }
    }
}
