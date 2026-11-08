#![no_std]
//! Allocation-free retained UI using embedded-graphics.
//! Components are independent of display hardware and application pages.
//! Scene handles input/focus and tracks damage; rendering clips all affected
//! layers and repairs previous-frame damage when the caller alternates buffers.

mod components;
mod drawing;
mod event;
mod geometry;
mod icon;
mod scene;
mod style;
mod text;

pub use components::{Base, Button, ComboBox, Label, ListView, Panel, Slider, Widget};
pub use event::{ActionKind, Event, Key, UiAction, WidgetId};
pub use icon::AlphaIcon;
pub use scene::{AddError, Scene};
pub use style::{Alignment, FontSize, Style};
pub use text::InlineText;
