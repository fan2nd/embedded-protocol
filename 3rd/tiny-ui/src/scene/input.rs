//! Focus visibility, pointer capture and event routing.
use super::*;
use crate::{ActionKind, Event, Key, UiAction};

impl<const N: usize> Scene<N> {
    pub fn focused(&self) -> Option<WidgetId> {
        self.focus.map(|i| self.widgets[i].unwrap().base().id)
    }
    /// Whether the logical focus is highlighted for keyboard navigation.
    /// Pointer input keeps logical focus but hides its navigation ring.
    pub fn focus_visible(&self) -> bool {
        self.focus_visible && self.focus.is_some()
    }
    fn set_focus_visible(&mut self, visible: bool) {
        if self.focus_visible != visible {
            self.focus_visible = visible;
            if let Some(i) = self.focus {
                self.invalidate(self.widgets[i].unwrap().extent());
            }
        }
    }
    /// Explicit focus requests show the keyboard-navigation ring.
    pub fn focus(&mut self, id: WidgetId) -> bool {
        if let Some(i) = self.widgets[..self.len]
            .iter()
            .position(|w| w.as_ref().unwrap().base().id == id && w.as_ref().unwrap().interactive())
        {
            if let Some(captured) = self.capture.take() {
                let old = self.widgets[captured].unwrap().extent();
                self.widgets[captured].as_mut().unwrap().cancel();
                self.invalidate(old);
            }
            self.dismissing = false;
            if let Some(pressed) = self.key_pressed.take() {
                let old = self.widgets[pressed].unwrap().extent();
                self.widgets[pressed].as_mut().unwrap().cancel();
                self.invalidate(old);
            }
            self.change_focus(Some(i));
            self.set_focus_visible(true);
            true
        } else {
            false
        }
    }
    fn change_focus(&mut self, next: Option<usize>) {
        if self.focus == next {
            return;
        }
        if let Some(i) = self.focus {
            let old = self.widgets[i].unwrap().extent();
            self.widgets[i].as_mut().unwrap().cancel();
            self.invalidate(old);
        }
        self.key_pressed = None;
        self.focus = next;
        if let Some(i) = next {
            self.invalidate(self.widgets[i].unwrap().extent());
        }
    }
    pub fn handle(&mut self, event: Event) -> Option<UiAction> {
        // A pointer gesture owns input until release/cancel. Focus navigation is
        // an explicit handoff; other keyboard input cannot activate it twice.
        if self.capture.is_some()
            && matches!(event,Event::KeyDown(k) if !matches!(k,Key::Next|Key::Previous|Key::Escape))
        {
            return None;
        }
        if matches!(event, Event::KeyDown(_)) {
            self.set_focus_visible(true);
        }
        if matches!(event, Event::KeyDown(Key::Escape)) {
            return self.handle(Event::Cancel);
        }
        if matches!(event, Event::Down(_)) {
            self.set_focus_visible(false);
            if let Some(i) = self.key_pressed.take() {
                if let Some(Widget::Button(v)) = self.widgets[i].as_mut() {
                    v.pressed = false;
                }
                self.invalidate(self.widgets[i].unwrap().extent());
            }
        }
        if matches!(event, Event::KeyDown(Key::Next | Key::Previous)) {
            if let Some(i) = self.capture.take() {
                let old = self.widgets[i].unwrap().extent();
                self.widgets[i].as_mut().unwrap().cancel();
                self.invalidate(old);
            }
            if let Some(i) = self.focus {
                let old = self.widgets[i].unwrap().extent();
                self.widgets[i].as_mut().unwrap().cancel();
                self.invalidate(old);
            }
            self.key_pressed = None;
            self.dismissing = false;
            let backwards = matches!(event, Event::KeyDown(Key::Previous));
            for offset in 1..=self.len {
                let i = if backwards {
                    (self.focus.unwrap_or(0) + self.len - offset) % self.len
                } else {
                    (self.focus.unwrap_or(self.len.saturating_sub(1)) + offset) % self.len
                };
                if self.widgets[i].unwrap().interactive() {
                    self.change_focus(Some(i));
                    break;
                }
            }
            return None;
        }
        if matches!(event, Event::KeyDown(Key::Activate)) {
            if self.key_pressed.is_some() {
                return None;
            }
            self.key_pressed = self.focus;
        }
        if matches!(event, Event::KeyUp(Key::Activate)) && self.key_pressed.take() != self.focus {
            return None;
        }
        if matches!(event, Event::Cancel | Event::KeyDown(Key::Escape)) {
            self.key_pressed = None;
        }
        if self.dismissing {
            if matches!(event, Event::Up(_) | Event::Cancel) {
                self.dismissing = false;
                self.capture = None;
            }
            return None;
        }
        let popup=(0..self.len).rev().find(|&i|matches!(self.widgets[i],Some(Widget::ComboBox(v)) if v.open&&v.base.visible&&v.base.enabled));
        if let Event::Down(p) = event {
            if let Some(old) = self.capture.take() {
                let w = self.widgets[old].as_mut().unwrap();
                let r = w.extent();
                w.cancel();
                self.invalidate(r);
            }
            let hit = popup.or_else(|| {
                (0..self.len).rev().find(|&i| {
                    let w = self.widgets[i].unwrap();
                    w.interactive() && w.base().bounds.contains(p)
                })
            });
            self.capture = hit;
            if let Some(i) = hit {
                self.change_focus(Some(i));
            }
            if let Some(i) = popup {
                if let Some(Widget::ComboBox(v)) = self.widgets[i] {
                    if !v.base.bounds.contains(p) && !v.popup_bounds().contains(p) {
                        self.dismissing = true;
                    }
                }
            }
        }
        let index = match event {
            Event::Down(_) | Event::Move(_) | Event::Up(_) => self.capture,
            Event::Cancel => self.capture.or(popup).or(self.focus),
            Event::KeyDown(_) | Event::KeyUp(_) => popup.or(self.focus),
        };
        let i = index?;
        let w = self.widgets[i].as_mut().unwrap();
        let original = *w;
        let old = w.extent();
        let id = w.base().id;
        let mut action = None;
        match w {
            Widget::Button(v) => match event {
                Event::Down(_) | Event::KeyDown(Key::Activate) => v.pressed = true,
                Event::Move(p) => v.pressed = v.base.bounds.contains(p),
                Event::Up(p) => {
                    if v.pressed && v.base.bounds.contains(p) {
                        action = Some(ActionKind::Clicked);
                    }
                    v.pressed = false;
                }
                Event::KeyUp(Key::Activate) => {
                    if v.pressed {
                        action = Some(ActionKind::Clicked);
                    }
                    v.pressed = false;
                }
                Event::Cancel | Event::KeyDown(Key::Escape) => v.pressed = false,
                _ => {}
            },
            Widget::Slider(v) => {
                let prev = v.value;
                match event {
                    Event::Down(p) | Event::Move(p) | Event::Up(p) => v.at(p),
                    Event::KeyDown(Key::Left | Key::Down) => {
                        v.value = v.quantize(v.value as i64 - (v.step as i64).abs().max(1))
                    }
                    Event::KeyDown(Key::Right | Key::Up) => {
                        v.value = v.quantize(v.value as i64 + (v.step as i64).abs().max(1))
                    }
                    _ => {}
                }
                if v.value != prev {
                    action = Some(ActionKind::ValueChanged(v.value));
                }
            }
            Widget::ComboBox(v) => match event {
                Event::Up(p) => {
                    if v.open {
                        let popup = v.popup_bounds();
                        if popup.contains(p) && v.row_height > 0 {
                            let row = ((p.y as i64 - popup.top_left.y as i64) / v.row_height as i64)
                                as usize;
                            if row < v.items.len() {
                                v.selected = row;
                                action = Some(ActionKind::SelectionChanged(row));
                            }
                        }
                        v.open = false;
                    } else if v.base.bounds.contains(p) {
                        v.open = true;
                    }
                }
                Event::Down(p)
                    if v.open && !v.popup_bounds().contains(p) && !v.base.bounds.contains(p) =>
                {
                    v.open = false
                }
                Event::Cancel | Event::KeyDown(Key::Escape) => v.open = false,
                Event::KeyDown(Key::Activate) => v.open = !v.open,
                Event::KeyDown(Key::Up | Key::Down) if !v.items.is_empty() => {
                    let old = v.selected;
                    v.selected = if old >= v.items.len() {
                        0
                    } else if matches!(event, Event::KeyDown(Key::Up)) {
                        old.saturating_sub(1)
                    } else {
                        (old + 1).min(v.items.len() - 1)
                    };
                    if old != v.selected {
                        action = Some(ActionKind::SelectionChanged(v.selected));
                    }
                }
                _ => {}
            },
            Widget::ListView(v) => match event {
                Event::Down(p) => {
                    v.origin = p;
                    v.scroll = v.scroll.min(v.max_scroll());
                    v.initial_scroll = v.scroll;
                    v.dragging = false;
                }
                Event::Move(p) => {
                    let dy = p.y as i64 - v.origin.y as i64;
                    let dx = p.x as i64 - v.origin.x as i64;
                    if dy.abs().max(dx.abs()) > v.drag_threshold as i64 {
                        v.dragging = true;
                    }
                    if v.dragging {
                        v.scroll =
                            (v.initial_scroll as i64 - dy).clamp(0, v.max_scroll() as i64) as u32;
                    }
                }
                Event::Up(p) => {
                    let moved = (p.y as i64 - v.origin.y as i64)
                        .abs()
                        .max((p.x as i64 - v.origin.x as i64).abs())
                        > v.drag_threshold as i64;
                    if !v.dragging && !moved && v.base.bounds.contains(p) {
                        let row = ((p.y as i64 - v.base.bounds.top_left.y as i64
                            + v.scroll.min(v.max_scroll()) as i64)
                            / v.row_height.max(1) as i64)
                            as usize;
                        if row < v.items.len() {
                            v.selected = row;
                            action = Some(ActionKind::SelectionChanged(row));
                        }
                    }
                    v.dragging = false;
                }
                Event::KeyDown(Key::Up | Key::Down) => {
                    if !v.items.is_empty() {
                        let old = v.selected;
                        v.selected = if old >= v.items.len() {
                            0
                        } else if matches!(event, Event::KeyDown(Key::Up)) {
                            old.saturating_sub(1)
                        } else {
                            (old + 1).min(v.items.len() - 1)
                        };
                        v.reveal();
                        if old != v.selected {
                            action = Some(ActionKind::SelectionChanged(v.selected));
                        }
                    }
                }
                Event::Cancel => v.dragging = false,
                _ => {}
            },
            _ => {}
        }
        let new = w.extent();
        if *w != original {
            self.invalidate(old);
            self.invalidate(new);
        }
        if matches!(event, Event::Up(_) | Event::Cancel) {
            self.capture = None;
        }
        action.map(|kind| UiAction { id, kind })
    }
}
