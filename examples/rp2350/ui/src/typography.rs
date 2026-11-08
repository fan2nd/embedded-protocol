//! Small-screen captions belong to this example, not a fork of upstream tinyui.
use embedded_graphics::{
    mono_font::{ascii::FONT_6X10, MonoTextStyle},
    pixelcolor::Rgb565,
    prelude::*,
    primitives::Rectangle,
    text::{Baseline, Text},
};
use tiny_ui::InlineText;

pub enum TextSize {
    Small,
    Value,
}

pub struct Caption {
    pub id: u16,
    pub bounds: Rectangle,
    pub text: InlineText,
    pub color: Rgb565,
}
impl Caption {
    pub fn draw<D: DrawTarget<Color = Rgb565>>(&self, target: &mut D) -> Result<(), D::Error> {
        let mut clipped = target.clipped(&self.bounds);
        let origin =
            self.bounds.top_left + Point::new(0, (self.bounds.size.height as i32 - 10).max(0) / 2);
        Text::with_baseline(
            self.text.as_str(),
            origin,
            MonoTextStyle::new(&FONT_6X10, self.color),
            Baseline::Top,
        )
        .draw(&mut clipped)
        .map(|_| ())
    }
}
