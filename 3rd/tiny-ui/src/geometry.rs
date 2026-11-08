//! Overflow-safe rectangle damage union.
use embedded_graphics::{prelude::*, primitives::Rectangle};

pub(crate) fn union(a: Rectangle, b: Rectangle) -> Rectangle {
    let x = a.top_left.x.min(b.top_left.x);
    let y = a.top_left.y.min(b.top_left.y);
    let right =
        (a.top_left.x as i64 + a.size.width as i64).max(b.top_left.x as i64 + b.size.width as i64);
    let bottom = (a.top_left.y as i64 + a.size.height as i64)
        .max(b.top_left.y as i64 + b.size.height as i64);
    Rectangle::new(
        Point::new(x, y),
        Size::new(
            (right - x as i64).min(u32::MAX as i64) as u32,
            (bottom - y as i64).min(u32::MAX as i64) as u32,
        ),
    )
}
