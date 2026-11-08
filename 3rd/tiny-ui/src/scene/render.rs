//! Clipped repaint with explicit history damage for alternating buffers.
use super::*;
use crate::drawing::{draw_items, draw_widget};
use embedded_graphics::{
    prelude::*,
    primitives::{PrimitiveStyle, RoundedRectangle},
};

impl<const N: usize> Scene<N> {
    pub fn render<D: DrawTarget<Color = Rgb565>>(
        &mut self,
        target: &mut D,
    ) -> Result<Option<Rectangle>, D::Error> {
        self.render_with_extra_damage(target, None)
    }
    /// Draw a complete reference frame without consuming pending damage.
    pub fn render_full<D: DrawTarget<Color = Rgb565>>(
        &self,
        target: &mut D,
    ) -> Result<(), D::Error> {
        self.draw_region(target, self.viewport)
    }
    pub fn render_with_extra_damage<D: DrawTarget<Color = Rgb565>>(
        &mut self,
        target: &mut D,
        extra: Option<Rectangle>,
    ) -> Result<Option<Rectangle>, D::Error> {
        self.invalidate(extra);
        let rendered = self.damage;
        if let Some(damage) = rendered {
            self.draw_region(target, damage)?;
            self.damage = None;
        }
        Ok(rendered)
    }
    fn draw_region<D: DrawTarget<Color = Rgb565>>(
        &self,
        target: &mut D,
        damage: Rectangle,
    ) -> Result<(), D::Error> {
        let clip = damage.intersection(&self.viewport);
        let mut target = target.clipped(&clip);
        target.fill_solid(&clip, self.background)?;
        for (i, w) in self.widgets[..self.len].iter().enumerate() {
            let w = w.as_ref().unwrap();
            if w.base().visible {
                draw_widget(w, &mut target)?;
                if self.focus_visible && self.focus == Some(i) {
                    RoundedRectangle::with_equal_corners(
                        w.base().bounds,
                        Size::new(w.base().style.radius, w.base().style.radius),
                    )
                    .into_styled(PrimitiveStyle::with_stroke(w.base().style.accent, 1))
                    .draw(&mut target)?;
                }
            }
        }
        for w in self.widgets[..self.len].iter().flatten() {
            if let Widget::ComboBox(v) = w {
                if v.base.visible && v.open {
                    draw_items(
                        &mut target,
                        v.popup_bounds(),
                        v.items,
                        v.selected,
                        v.row_height,
                        0,
                        v.base.style,
                    )?;
                }
            }
        }
        Ok(())
    }
}
