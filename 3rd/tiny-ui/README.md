# tiny-ui

A reusable `#![no_std]`, allocation-free retained UI over `embedded-graphics`
`DrawTarget<Color = Rgb565>`. It has no board, display-resolution or demo dependency.

- `Scene<N>` owns up to N independently configured `Widget`s; duplicate IDs and
  capacity exhaustion return errors.
- `Button`, `Slider`, `ComboBox`, `ListView`, `Label`, `Panel` have public `Base`
  geometry, visibility, enablement and palette. Styles include radius, padding,
  18/20 pixel fonts and alternating row color. Item strings are static borrowed
  slices; labels use 96-byte UTF-8 inline storage and support `core::fmt::Write`.
- Pointer capture lasts until release/cancel. Buttons click on release; slider
  ranges/steps use widened arithmetic; list dragging cancels taps and clamps
  scrolling. Overflowing lists show a proportional indicator integrated into the
  right outline, with an 18-pixel minimum thumb and a muted outline/accent blend.
  It disappears when all rows fit. Custom row overlays
  should clip to `ListView::content_bounds()` to preserve its edge indicator
  and rounded focus perimeter. Row positions/input coordinates are unchanged.
  Popups capture input, dismiss outside, and render above all widgets.
- Keyboard previous/next moves focus; arrows adjust/select; activate operates
  buttons/combos and Escape/Cancel clears activation. Each scene owns its state.
  Logical focus is separate from its visible keyboard-navigation ring: pointer
  down hides the ring (including a touch on blank space), while an accepted
  key-down or explicit `scene.focus(id)` shows it. Touch still produces pressed
  feedback and selection, but release/cancel does not leave a focus outline.
  `focused()` returns the logical target; `focus_visible()` reports the ring.
- Change properties through `scene.update(id, |widget| ...)` to invalidate both
  old/new extents. Identity is preserved by update. `usize::MAX` means no list
  selection. Public values can be assigned directly; rendering clamps scroll and
  slider positions. Use constructors to normalize slider values to their step.
- `render` returns actual clipped damage and clears it only after success.
  `render_full(&self, ...)` draws a full reference frame without consuming damage.
  `render_with_extra_damage` accepts previous damage to repair alternating frame
  buffers. Save *current scene damage* before rendering for next frame; passing
  the already merged return damage repeatedly will unnecessarily accumulate it.

## Module layout

- `components.rs`: configurable widget data, bounds and local interaction state.
- `event.rs`: input events, widget IDs and application actions.
- `style.rs` / `text.rs`: palettes, typography and bounded UTF-8 text.
- `scene/mod.rs`: fixed-capacity storage, lifecycle and dirty regions.
- `scene/input.rs`: logical focus, keyboard-visible focus, pointer capture and dispatch.
- `scene/render.rs`: clipped scene repaint and alternating-buffer history repair.
- `drawing.rs` / `geometry.rs`: component rasterization and rectangle helpers.

Public types remain re-exported at the crate root, so existing applications do not
need import changes. The framework remains `no_std` and allocator-free.

## Validation

This project does not contain or add unit tests (see the root `AGENTS.md`).
Use `cargo check --target riscv32imafc-unknown-none-elf`, `cargo fmt --check`, and
`cargo clippy --lib --target x86_64-unknown-linux-gnu -- -D warnings` from this
crate. Firmware builds, source/visual review and on-board operation are distinct
checks; physical input/display timing still requires hardware validation.

## Minimal composition (no page-specific API)

```rust
use embedded_graphics::{pixelcolor::Rgb565, prelude::*, primitives::Rectangle};
use tiny_ui::{ActionKind, Button, Event, Scene, Slider};

let mut scene = Scene::<4>::new(
    Rectangle::new(Point::zero(), Size::new(240, 160)), Rgb565::BLACK,
);
scene.add(Slider::new(7, Rectangle::new(Point::new(10, 20), Size::new(200, 30)),
                      -20, 80, 5, 10)).unwrap();
scene.add(Button::new(9, Rectangle::new(Point::new(60, 90), Size::new(120, 40)),
                      "Apply")).unwrap();
// The input driver supplies events. It does not know about the application.
scene.handle(Event::Down(Point::new(100, 110)));
if let Some(action) = scene.handle(Event::Up(Point::new(100, 110))) {
    if action.id == 9 && action.kind == ActionKind::Clicked {
        // Enqueue an application command here; never wait for Wi-Fi in a widget.
    }
}
// scene.render(&mut display)?; // Caller provides an RGB565 DrawTarget.
```

An unmatched Up never clicks: a matching Down/capture (or keyboard activation)
is required. Forward the entire input sequence in the real driver.
Use `scene.update(id, |widget| ...)` when changing components already in a scene.
A scene can contain multiple sliders/buttons with independent geometry/state,
not one hardcoded instance of each type. Each scene owns its interaction state.

This first version uses a flat retained scene, explicit geometry, static item
arrays, ASCII bitmap fonts, and built-in widget variants. It is intentionally
not a Slint parser, automatic layout system, general custom-widget trait, or
animated/inertial scroller. List clipping is rectangular; popup size/placement
is configured by the caller and clipped to the scene viewport.

`AlphaIcon` draws a pre-rasterized 8-bit coverage mask against a known solid
background without allocation. Applications own icon artwork and attribution.
`Style::border_width` controls normal rounded control outlines; the default is
one logical pixel and the stroke stays inside the widget bounds. This persistent
control boundary is separate from keyboard-only focus feedback.
