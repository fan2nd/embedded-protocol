//! Business UI over tiny-ui; custom meters/plots are clipped application overlays.
use crate::{
    typography::{Caption, TextSize},
    HEIGHT, WIDTH,
};
use core::fmt::Write;
use embedded_graphics::{
    pixelcolor::Rgb565,
    prelude::*,
    primitives::{Arc, Circle, Line, PrimitiveStyle, Rectangle, RoundedRectangle},
};
use heapless::String;
use monitor_protocol::Metrics;
use tiny_ui::{Alignment, FontSize, InlineText, Label, Panel, Scene, Style, Widget};

const BG: Rgb565 = Rgb565::new(1, 3, 5);
const CARD: Rgb565 = Rgb565::new(2, 6, 9);
const EDGE: Rgb565 = Rgb565::new(4, 12, 15);
const TRACK: Rgb565 = Rgb565::new(5, 13, 16);
const TEXT: Rgb565 = Rgb565::new(28, 59, 30);
const MUTED: Rgb565 = Rgb565::new(14, 31, 19);
const CYAN: Rgb565 = Rgb565::new(4, 54, 29);
const PURPLE: Rgb565 = Rgb565::new(23, 32, 31);
const GREEN: Rgb565 = Rgb565::new(13, 55, 17);
const AMBER: Rgb565 = Rgb565::new(31, 43, 8);
const HISTORY: usize = 241;
const WINDOW_MS: u64 = 60_000;

const CPU_VALUE: u16 = 20;
const MEM_VALUE: u16 = 21;
const RX_VALUE: u16 = 22;
const TX_VALUE: u16 = 23;
const UPTIME: u16 = 24;

const fn rect(x: i32, y: i32, w: u32, h: u32) -> Rectangle {
    Rectangle::new(Point::new(x, y), Size::new(w, h))
}
const CPU_RING: Rectangle = rect(34, 42, 56, 56);
const MEM_RING: Rectangle = rect(151, 42, 56, 56);
const BIG_PLOT: Rectangle = rect(38, 46, 188, 43);

pub struct Dashboard {
    scene: Scene<24>,
    captions: heapless::Vec<Caption, 24>,
    metrics: Metrics,
    live: bool,
    page: bool,
    cpu: [u16; HISTORY],
    memory: [u16; HISTORY],
    timestamps: [u64; HISTORY],
    now_ms: u64,
    head: usize,
    count: usize,
}
impl Default for Dashboard {
    fn default() -> Self {
        Self::new()
    }
}
impl Dashboard {
    pub fn new() -> Self {
        let mut this = Self {
            scene: Scene::new(rect(0, 0, WIDTH as u32, HEIGHT as u32), BG),
            captions: heapless::Vec::new(),
            metrics: Metrics::default(),
            live: false,
            page: false,
            cpu: [0; HISTORY],
            memory: [0; HISTORY],
            timestamps: [0; HISTORY],
            now_ms: 0,
            head: 0,
            count: 0,
        };
        this.rebuild();
        this
    }
    /// `new_sample` distinguishes a real host update from the UI/key polling tick.
    pub fn update(&mut self, m: Metrics, live: bool, page: bool, new_sample: bool, now_ms: u64) {
        let time_changed = now_ms / 1000 != self.now_ms / 1000;
        self.now_ms = now_ms;
        let old = self.metrics;
        let rebuild = live != self.live || (live && page != self.page);
        if live && !self.live {
            self.head = 0;
            self.count = 0;
        }
        self.metrics = m;
        self.live = live;
        self.page = page;
        if live && new_sample {
            self.timestamps[self.head] = now_ms;
            self.cpu[self.head] = m.cpu.min(1000) as u16;
            self.memory[self.head] = m.memory.min(1000) as u16;
            self.head = (self.head + 1) % HISTORY;
            self.count = (self.count + 1).min(HISTORY);
        }
        if rebuild {
            self.rebuild();
            return;
        }
        if !live {
            return;
        }
        if page {
            if new_sample || time_changed {
                self.scene.invalidate(Some(BIG_PLOT));
            }
            self.set_label(UPTIME, &uptime(m.uptime_s));
        } else {
            if m.cpu != old.cpu {
                self.scene.invalidate(Some(CPU_RING));
                self.set_label(CPU_VALUE, &memory_percent(m.cpu));
            }
            if m.memory != old.memory {
                self.scene.invalidate(Some(MEM_RING));
                self.set_label(MEM_VALUE, &memory_percent(m.memory));
            }
            self.set_label(RX_VALUE, &rate(m.rx_kib));
            self.set_label(TX_VALUE, &rate(m.tx_kib));
        }
    }
    pub fn damage(&self) -> Option<Rectangle> {
        self.scene.damage()
    }

    /// Repair the back buffer's previous-frame damage as well as current changes.
    /// Save damage() BEFORE this call; the returned union is not history state.
    pub fn render_with_extra_damage<D: DrawTarget<Color = Rgb565>>(
        &mut self,
        target: &mut D,
        previous: Option<Rectangle>,
    ) -> Result<Option<Rectangle>, D::Error> {
        let region = self.scene.render_with_extra_damage(target, previous)?;
        if let Some(region) = region {
            if let Err(error) = self.overlays(&mut target.clipped(&region)) {
                self.scene.invalidate(Some(region));
                return Err(error);
            }
        }
        Ok(region)
    }
    /// Full reference rendering for preview/debug; doesn't consume pending damage.
    pub fn render_full<D: DrawTarget<Color = Rgb565>>(
        &self,
        target: &mut D,
    ) -> Result<(), D::Error> {
        self.scene.render_full(target)?;
        self.overlays(target)
    }
    fn panel(&mut self, id: u16, bounds: Rectangle, color: Rgb565, border: Rgb565) {
        let mut panel = Panel::new(id, bounds);
        panel.base.style = Style {
            background: color,
            border,
            radius: 10,
            ..Style::default()
        };
        self.scene.add(panel).unwrap();
    }
    fn label(&mut self, id: u16, bounds: Rectangle, text: &str, color: Rgb565, font: TextSize) {
        if matches!(font, TextSize::Small) {
            assert!(self
                .captions
                .push(Caption {
                    id,
                    bounds,
                    text: InlineText::new(text),
                    color
                })
                .is_ok());
            self.scene.invalidate(Some(bounds));
            return;
        }
        let mut label = Label::new(id, bounds, text);
        label.base.style = Style {
            foreground: color,
            font: FontSize::Normal,
            padding: 0,
            align: if id == MEM_VALUE || id == CPU_VALUE {
                Alignment::Center
            } else {
                Alignment::Left
            },
            ..Style::default()
        };
        self.scene.add(label).unwrap();
    }
    fn set_label(&mut self, id: u16, text: &str) {
        if let Some(caption) = self.captions.iter_mut().find(|c| c.id == id) {
            if caption.text.as_str() != text {
                caption.text = InlineText::new(text);
                self.scene.invalidate(Some(caption.bounds));
            }
            return;
        }
        if matches!(self.scene.get(id), Some(Widget::Label(label)) if label.text.as_str() == text) {
            return;
        }
        self.scene.update(id, |widget| {
            if let Widget::Label(label) = widget {
                label.text = InlineText::new(text);
            }
        });
    }
    fn rebuild(&mut self) {
        self.scene.clear();
        self.captions.clear();
        self.label(
            1,
            rect(8, 4, 156, 13),
            if self.page && self.live {
                "SYSTEM / HISTORY"
            } else {
                "RESOURCE / LIVE"
            },
            TEXT,
            TextSize::Small,
        );
        self.panel(2, rect(175, 3, 59, 15), CARD, EDGE);
        self.label(
            3,
            rect(188, 3, 44, 15),
            if self.live { "LIVE" } else { "OFFLINE" },
            if self.live { GREEN } else { AMBER },
            TextSize::Small,
        );
        if !self.live {
            self.panel(4, rect(6, 24, 228, 105), CARD, EDGE);
            self.label(
                5,
                rect(75, 84, 150, 15),
                "CONNECT A HOST",
                TEXT,
                TextSize::Small,
            );
            self.label(
                6,
                rect(75, 101, 136, 13),
                "USB CDC / PCMON1",
                MUTED,
                TextSize::Small,
            );
            return;
        }
        if self.page {
            self.panel(4, rect(6, 24, 228, 74), CARD, EDGE);
            self.label(5, rect(21, 28, 64, 12), "CPU", CYAN, TextSize::Small);
            self.label(6, rect(72, 28, 64, 12), "MEM", PURPLE, TextSize::Small);
            self.label(
                7,
                rect(150, 28, 76, 12),
                "60s / 100%",
                MUTED,
                TextSize::Small,
            );
            self.label(10, rect(10, 41, 26, 12), "100%", MUTED, TextSize::Small);
            self.label(11, rect(16, 62, 20, 12), "50%", MUTED, TextSize::Small);
            self.label(12, rect(22, 83, 14, 12), "0%", MUTED, TextSize::Small);
            self.panel(8, rect(6, 102, 228, 27), CARD, EDGE);
            self.label(
                UPTIME,
                rect(29, 108, 155, 14),
                &uptime(self.metrics.uptime_s),
                TEXT,
                TextSize::Small,
            );
            self.label(9, rect(193, 108, 30, 14), "2 / 2", MUTED, TextSize::Small);
        } else {
            self.panel(4, rect(6, 24, 111, 78), CARD, EDGE);
            self.panel(5, rect(123, 24, 111, 78), CARD, EDGE);
            self.label(6, rect(35, 29, 64, 13), "CPU LOAD", CYAN, TextSize::Small);
            self.label(7, rect(161, 29, 50, 13), "MEMORY", PURPLE, TextSize::Small);
            self.label(
                CPU_VALUE,
                rect(42, 54, 40, 21),
                &memory_percent(self.metrics.cpu),
                TEXT,
                TextSize::Value,
            );
            self.label(8, rect(58, 77, 8, 12), "%", MUTED, TextSize::Small);
            self.label(
                MEM_VALUE,
                rect(159, 54, 40, 21),
                &memory_percent(self.metrics.memory),
                TEXT,
                TextSize::Value,
            );
            self.label(9, rect(175, 77, 8, 12), "%", MUTED, TextSize::Small);
            self.panel(10, rect(6, 107, 111, 23), CARD, EDGE);
            self.panel(11, rect(123, 107, 111, 23), CARD, EDGE);
            self.label(
                RX_VALUE,
                rect(29, 111, 84, 14),
                &rate(self.metrics.rx_kib),
                TEXT,
                TextSize::Small,
            );
            self.label(
                TX_VALUE,
                rect(146, 111, 80, 14),
                &rate(self.metrics.tx_kib),
                TEXT,
                TextSize::Small,
            );
        }
    }
    fn overlays<D: DrawTarget<Color = Rgb565>>(&self, target: &mut D) -> Result<(), D::Error> {
        self.graphics(target)?;
        // Captions share Scene invalidation but use an application-owned 6x10 font.
        // Render after the clipped scene/graphics so damage repair restores every layer.
        for caption in &self.captions {
            caption.draw(target)?;
        }
        Ok(())
    }

    fn graphics<D: DrawTarget<Color = Rgb565>>(&self, t: &mut D) -> Result<(), D::Error> {
        Circle::new(Point::new(180, 8), 4)
            .into_styled(PrimitiveStyle::with_fill(if self.live {
                GREEN
            } else {
                AMBER
            }))
            .draw(t)?;
        if !self.live {
            RoundedRectangle::with_equal_corners(rect(95, 39, 50, 31), Size::new(6, 6))
                .into_styled(PrimitiveStyle::with_stroke(CYAN, 2))
                .draw(t)?;
            line(t, Point::new(120, 70), Point::new(120, 77), CYAN, 2)?;
            line(t, Point::new(109, 78), Point::new(131, 78), CYAN, 2)?;
            return Ok(());
        }
        if self.page {
            for y in [46, 67, 88] {
                line(t, Point::new(35, y), Point::new(225, y), EDGE, 1)?;
            }
            line(t, Point::new(37, 46), Point::new(37, 88), MUTED, 1)?;
            self.plot(t, BIG_PLOT, &self.memory, PURPLE)?;
            self.plot(t, BIG_PLOT, &self.cpu, CYAN)?;
            Circle::new(Point::new(14, 31), 4)
                .into_styled(PrimitiveStyle::with_fill(CYAN))
                .draw(t)?;
            Circle::new(Point::new(65, 31), 4)
                .into_styled(PrimitiveStyle::with_fill(PURPLE))
                .draw(t)?;
            Circle::new(Point::new(14, 111), 10)
                .into_styled(PrimitiveStyle::with_stroke(MUTED, 1))
                .draw(t)?;
            line(t, Point::new(19, 113), Point::new(19, 116), MUTED, 1)?;
            line(t, Point::new(19, 116), Point::new(22, 117), MUTED, 1)?;
        } else {
            meter(t, MEM_RING, self.metrics.memory, PURPLE)?;
            meter(t, CPU_RING, self.metrics.cpu, CYAN)?;
            arrow(t, Point::new(18, 113), true, CYAN)?;
            arrow(t, Point::new(135, 113), false, PURPLE)?;
        }
        Ok(())
    }
    fn plot<D: DrawTarget<Color = Rgb565>>(
        &self,
        t: &mut D,
        bounds: Rectangle,
        values: &[u16; HISTORY],
        color: Rgb565,
    ) -> Result<(), D::Error> {
        let intersection = bounds.intersection(&t.bounding_box());
        if self.count < 2 || intersection.size.width == 0 || intersection.size.height == 0 {
            return Ok(());
        }
        let mut t = t.clipped(&bounds);
        let start = (self.head + HISTORY - self.count) % HISTORY;
        let bottom = bounds.top_left.y + bounds.size.height as i32 - 1;
        let mut previous = None;
        for i in 0..self.count {
            let index = (start + i) % HISTORY;
            let age = self.now_ms.saturating_sub(self.timestamps[index]);
            if age > WINDOW_MS {
                previous = None;
                continue;
            }
            let point = Point::new(
                bounds.top_left.x
                    + ((WINDOW_MS - age) * (bounds.size.width - 1) as u64 / WINDOW_MS) as i32,
                bottom - values[index] as i32 * (bounds.size.height as i32 - 1) / 1000,
            );
            if let Some(last) = previous {
                line(&mut t, last, point, color, 1)?;
            }
            previous = Some(point);
        }
        Ok(())
    }
}
fn memory_percent(value: u32) -> String<16> {
    let mut s = String::new();
    let _ = write!(s, "{}", (value.min(1000) + 5) / 10);
    s
}

fn rate(kib: u32) -> String<32> {
    let mut s = String::new();
    if kib >= 1024 * 1024 {
        let _ = write!(
            s,
            "{}.{:01} GiB/s",
            kib / (1024 * 1024),
            (kib as u64 * 10 / (1024 * 1024)) % 10
        );
    } else if kib >= 1024 {
        let _ = write!(
            s,
            "{}.{:01} MiB/s",
            kib / 1024,
            (kib as u64 * 10 / 1024) % 10
        );
    } else {
        let _ = write!(s, "{} KiB/s", kib);
    }
    s
}
fn uptime(seconds: u32) -> String<32> {
    let mut s = String::new();
    let _ = write!(s, "UP {}h {:02}m", seconds / 3600, (seconds / 60) % 60);
    s
}
fn line<D: DrawTarget<Color = Rgb565>>(
    t: &mut D,
    a: Point,
    b: Point,
    c: Rgb565,
    width: u32,
) -> Result<(), D::Error> {
    Line::new(a, b)
        .into_styled(PrimitiveStyle::with_stroke(c, width))
        .draw(t)
}
fn meter<D: DrawTarget<Color = Rgb565>>(
    t: &mut D,
    bounds: Rectangle,
    value: u32,
    color: Rgb565,
) -> Result<(), D::Error> {
    let intersection = bounds.intersection(&t.bounding_box());
    if intersection.size.width == 0 || intersection.size.height == 0 {
        return Ok(());
    }
    let mut t = t.clipped(&bounds);
    let origin = bounds.top_left + Point::new(4, 4);
    let diameter = bounds.size.width - 8;
    Arc::new(origin, diameter, 150.0f32.deg(), 240.0f32.deg())
        .into_styled(PrimitiveStyle::with_stroke(TRACK, 4))
        .draw(&mut t)?;
    if value > 0 {
        Arc::new(
            origin,
            diameter,
            150.0f32.deg(),
            (value.min(1000) as f32 * 0.24).deg(),
        )
        .into_styled(PrimitiveStyle::with_stroke(color, 4))
        .draw(&mut t)?;
    }
    Ok(())
}
fn arrow<D: DrawTarget<Color = Rgb565>>(
    t: &mut D,
    p: Point,
    down: bool,
    c: Rgb565,
) -> Result<(), D::Error> {
    let (a, b) = if down {
        (p, p + Point::new(0, 10))
    } else {
        (p + Point::new(0, 10), p)
    };
    line(t, a, b, c, 1)?;
    let dy = if down { -4 } else { 4 };
    line(t, b, b + Point::new(-4, dy), c, 1)?;
    line(t, b, b + Point::new(4, dy), c, 1)
}
