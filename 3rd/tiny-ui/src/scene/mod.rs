//! Fixed-capacity retained scene, lifecycle and damage tracking.
mod input;
mod render;
use crate::{geometry::union, Widget, WidgetId};
use embedded_graphics::{pixelcolor::Rgb565, primitives::Rectangle};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AddError {
    Full,
    DuplicateId,
}
pub struct Scene<const N: usize> {
    widgets: [Option<Widget>; N],
    len: usize,
    viewport: Rectangle,
    background: Rgb565,
    damage: Option<Rectangle>,
    capture: Option<usize>,
    focus: Option<usize>,
    focus_visible: bool,
    key_pressed: Option<usize>,
    dismissing: bool,
}
impl<const N: usize> Scene<N> {
    pub fn new(viewport: Rectangle, background: Rgb565) -> Self {
        Self {
            widgets: [None; N],
            len: 0,
            viewport,
            background,
            damage: Some(viewport),
            capture: None,
            focus: None,
            focus_visible: false,
            key_pressed: None,
            dismissing: false,
        }
    }
    /// Remove all widgets in place while retaining viewport and background.
    /// Replacing a large Scene with Scene::new creates a second inline array on
    /// embedded stacks. Clearing slots avoids that temporary during page changes.
    pub fn clear(&mut self) {
        for widget in &mut self.widgets[..self.len] {
            *widget = None;
        }
        self.len = 0;
        self.capture = None;
        self.focus = None;
        self.focus_visible = false;
        self.key_pressed = None;
        self.dismissing = false;
        self.damage = Some(self.viewport);
    }

    pub fn add(&mut self, widget: impl Into<Widget>) -> Result<(), AddError> {
        let w = widget.into();
        if self.get(w.base().id).is_some() {
            return Err(AddError::DuplicateId);
        }
        if self.len == N {
            return Err(AddError::Full);
        }
        self.invalidate(w.extent());
        self.widgets[self.len] = Some(w);
        self.len += 1;
        Ok(())
    }
    pub fn get(&self, id: WidgetId) -> Option<&Widget> {
        self.widgets[..self.len]
            .iter()
            .flatten()
            .find(|w| w.base().id == id)
    }
    pub fn update(&mut self, id: WidgetId, f: impl FnOnce(&mut Widget)) -> bool {
        if let Some(i) = self.widgets[..self.len]
            .iter()
            .position(|w| w.as_ref().unwrap().base().id == id)
        {
            let old = self.widgets[i].unwrap().extent();
            f(self.widgets[i].as_mut().unwrap());
            self.widgets[i].as_mut().unwrap().base_mut().id = id;
            if !self.widgets[i].unwrap().interactive() {
                self.widgets[i].as_mut().unwrap().cancel();
                if self.capture == Some(i) {
                    self.capture = None;
                }
                if self.focus == Some(i) {
                    self.focus = None;
                }
            }
            let new = self.widgets[i].unwrap().extent();
            self.invalidate(old);
            self.invalidate(new);
            true
        } else {
            false
        }
    }
    pub fn damage(&self) -> Option<Rectangle> {
        self.damage
    }
    /// Retained controls in paint order for an alternate rendering backend.
    pub fn widgets(&self) -> impl Iterator<Item = &Widget> {
        self.widgets[..self.len].iter().flatten()
    }
    /// Acknowledge damage only after an external renderer completed the region.
    pub fn clear_damage(&mut self) {
        self.damage = None;
    }
    pub fn viewport(&self) -> Rectangle {
        self.viewport
    }
    pub fn set_background(&mut self, color: Rgb565) {
        self.background = color;
        self.invalidate(Some(self.viewport));
    }
    pub fn set_viewport(&mut self, viewport: Rectangle) {
        self.invalidate(Some(self.viewport));
        self.viewport = viewport;
        self.invalidate(Some(viewport));
    }
    pub fn invalidate(&mut self, r: Option<Rectangle>) {
        if let Some(r) = r {
            let r = r.intersection(&self.viewport);
            if r.size.width == 0 || r.size.height == 0 {
                return;
            }
            self.damage = Some(self.damage.map_or(r, |old| union(old, r)));
        }
    }
}
