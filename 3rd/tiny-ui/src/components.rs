//! Configurable widget data and local interaction state.
use crate::{geometry::union, InlineText, Style, WidgetId};
use embedded_graphics::{prelude::*, primitives::Rectangle};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Base {
    pub id: WidgetId,
    pub bounds: Rectangle,
    pub visible: bool,
    pub enabled: bool,
    pub style: Style,
}
impl Base {
    pub fn new(id: WidgetId, bounds: Rectangle) -> Self {
        Self {
            id,
            bounds,
            visible: true,
            enabled: true,
            style: Style::default(),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Button {
    pub base: Base,
    pub text: &'static str,
    pub(crate) pressed: bool,
}
impl Button {
    pub fn is_pressed(&self) -> bool {
        self.pressed
    }
    pub fn new(id: WidgetId, bounds: Rectangle, text: &'static str) -> Self {
        Self {
            base: Base::new(id, bounds),
            text,
            pressed: false,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Label {
    pub base: Base,
    pub text: InlineText,
}
impl Label {
    pub fn new(id: WidgetId, bounds: Rectangle, text: &str) -> Self {
        Self {
            base: Base::new(id, bounds),
            text: InlineText::new(text),
        }
    }
    pub fn set_text(&mut self, text: &str) {
        self.text = InlineText::new(text);
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Panel {
    pub base: Base,
}
impl Panel {
    pub fn new(id: WidgetId, bounds: Rectangle) -> Self {
        Self {
            base: Base::new(id, bounds),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Slider {
    pub base: Base,
    pub min: i32,
    pub max: i32,
    pub step: i32,
    pub value: i32,
}
impl Slider {
    pub fn new(id: WidgetId, bounds: Rectangle, min: i32, max: i32, step: i32, value: i32) -> Self {
        let mut s = Self {
            base: Base::new(id, bounds),
            min,
            max,
            step,
            value,
        };
        s.value = s.quantize(value as i64);
        s
    }
    pub(crate) fn limits(&self) -> (i64, i64) {
        (
            (self.min.min(self.max)) as i64,
            (self.min.max(self.max)) as i64,
        )
    }
    pub(crate) fn quantize(&self, v: i64) -> i32 {
        let (lo, hi) = self.limits();
        let step = (self.step as i64).abs().max(1);
        let v = v.clamp(lo, hi);
        if v == hi {
            return hi as i32;
        }
        (lo + ((v - lo + step / 2) / step) * step).clamp(lo, hi) as i32
    }
    /// Thumb centers stay inside the bounds at both ends of the range.
    pub(crate) fn track_geometry(&self) -> (u32, u32) {
        let width = self.base.bounds.size.width;
        let inset = 6.min(width.saturating_sub(1) / 2);
        (inset, width.saturating_sub(1).saturating_sub(inset * 2))
    }
    pub(crate) fn at(&mut self, p: Point) {
        let (lo, hi) = self.limits();
        let (inset, travel) = self.track_geometry();
        let width = travel.max(1) as i64;
        let x = (p.x as i64 - self.base.bounds.top_left.x as i64 - inset as i64).clamp(0, width);
        self.value = self.quantize(lo + ((hi - lo) as u64 * x as u64 / width as u64) as i64);
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ComboBox {
    pub base: Base,
    pub items: &'static [&'static str],
    pub selected: usize,
    pub row_height: u32,
    pub open: bool,
}
impl ComboBox {
    pub fn new(id: WidgetId, bounds: Rectangle, items: &'static [&'static str]) -> Self {
        Self {
            base: Base::new(id, bounds),
            items,
            selected: 0,
            row_height: 30,
            open: false,
        }
    }
    pub fn popup_bounds(&self) -> Rectangle {
        Rectangle::new(
            Point::new(
                self.base.bounds.top_left.x,
                self.base
                    .bounds
                    .top_left
                    .y
                    .saturating_add(self.base.bounds.size.height.min(i32::MAX as u32) as i32),
            ),
            Size::new(
                self.base.bounds.size.width,
                self.row_height
                    .saturating_mul(self.items.len().min(u32::MAX as usize) as u32),
            ),
        )
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ListView {
    pub base: Base,
    pub items: &'static [&'static str],
    pub selected: usize,
    pub row_height: u32,
    pub scroll: u32,
    pub drag_threshold: u32,
    pub(crate) origin: Point,
    pub(crate) initial_scroll: u32,
    pub(crate) dragging: bool,
}
impl ListView {
    pub fn new(id: WidgetId, bounds: Rectangle, items: &'static [&'static str]) -> Self {
        Self {
            base: Base::new(id, bounds),
            items,
            selected: 0,
            row_height: 30,
            scroll: 0,
            drag_threshold: 6,
            origin: Point::zero(),
            initial_scroll: 0,
            dragging: false,
        }
    }
    pub fn max_scroll(&self) -> u32 {
        self.row_height
            .max(1)
            .saturating_mul(self.items.len().min(u32::MAX as usize) as u32)
            .saturating_sub(self.base.bounds.size.height)
    }
    /// Bounds for custom row overlays, excluding the rounded perimeter and its scroll indicator.
    /// Row positions still use `base.bounds.top_left` and the clamped scroll offset.
    pub fn content_bounds(&self) -> Rectangle {
        let b = self.base.bounds;
        // Keep row fills/overlays inside the curved focus perimeter.
        let inset = self
            .base
            .style
            .radius
            .div_ceil(2)
            .max(self.base.style.border_width)
            .max(4)
            .min(b.size.width / 2)
            .min(b.size.height / 2);
        Rectangle::new(
            Point::new(
                b.top_left.x.saturating_add(inset as i32),
                b.top_left.y.saturating_add(inset as i32),
            ),
            Size::new(
                b.size.width.saturating_sub(2 * inset),
                b.size.height.saturating_sub(2 * inset),
            ),
        )
    }
    pub(crate) fn reveal(&mut self) {
        let top = (self.selected as u32).saturating_mul(self.row_height.max(1));
        if top < self.scroll {
            self.scroll = top;
        }
        let bottom = top.saturating_add(self.row_height.max(1));
        if bottom > self.scroll.saturating_add(self.base.bounds.size.height) {
            self.scroll = bottom.saturating_sub(self.base.bounds.size.height);
        }
        self.scroll = self.scroll.min(self.max_scroll());
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Widget {
    Button(Button),
    Slider(Slider),
    ComboBox(ComboBox),
    ListView(ListView),
    Label(Label),
    Panel(Panel),
}
macro_rules! conversions { ($($t:ident),*) => {$ (impl From<$t> for Widget {fn from(v:$t)->Self {Self::$t(v)}})*}; }
conversions!(Button, Slider, ComboBox, ListView, Label, Panel);
impl Widget {
    pub fn base(&self) -> &Base {
        match self {
            Self::Button(v) => &v.base,
            Self::Slider(v) => &v.base,
            Self::ComboBox(v) => &v.base,
            Self::ListView(v) => &v.base,
            Self::Label(v) => &v.base,
            Self::Panel(v) => &v.base,
        }
    }
    pub fn base_mut(&mut self) -> &mut Base {
        match self {
            Self::Button(v) => &mut v.base,
            Self::Slider(v) => &mut v.base,
            Self::ComboBox(v) => &mut v.base,
            Self::ListView(v) => &mut v.base,
            Self::Label(v) => &mut v.base,
            Self::Panel(v) => &mut v.base,
        }
    }
    pub(crate) fn interactive(&self) -> bool {
        self.base().visible
            && self.base().enabled
            && !matches!(self, Self::Label(_) | Self::Panel(_))
    }
    pub(crate) fn extent(&self) -> Option<Rectangle> {
        if !self.base().visible {
            return None;
        }
        let b = self.base().bounds;
        Some(match self {
            Self::ComboBox(v) if v.open => union(b, v.popup_bounds()),
            _ => b,
        })
    }
    pub(crate) fn cancel(&mut self) {
        match self {
            Self::Button(v) => v.pressed = false,
            Self::ListView(v) => v.dragging = false,
            Self::ComboBox(v) => v.open = false,
            _ => {}
        }
    }
}
