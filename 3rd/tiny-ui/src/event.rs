//! Input events and application actions, independent of any hardware driver.
use embedded_graphics::geometry::Point;

pub type WidgetId = u16;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Previous,
    Next,
    Left,
    Right,
    Up,
    Down,
    Activate,
    Escape,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Down(Point),
    Move(Point),
    Up(Point),
    Cancel,
    KeyDown(Key),
    KeyUp(Key),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActionKind {
    Clicked,
    ValueChanged(i32),
    SelectionChanged(usize),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UiAction {
    pub id: WidgetId,
    pub kind: ActionKind,
}
