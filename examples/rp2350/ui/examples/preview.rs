//! Desktop preview of the exact retained renderer and two-buffer damage sequence.
use embedded_graphics::{pixelcolor::Rgb565, prelude::*, primitives::PrimitiveStyle};
use monitor_protocol::Metrics;
use rp2350_dashboard::{Dashboard, Frame, FrameBytes, Rectangle, FRAME_BYTES, HEIGHT, WIDTH};
use std::{fs, io::Write};

struct Preview {
    ui: Dashboard,
    buffers: [FrameBytes; 2],
    panel: FrameBytes,
    reference: FrameBytes,
    previous: Option<Rectangle>,
    back: usize,
    commits: usize,
    payload: usize,
}
impl Preview {
    fn new() -> Self {
        Self {
            ui: Dashboard::new(),
            buffers: [[0xa5; FRAME_BYTES], [0x5a; FRAME_BYTES]],
            panel: [0; FRAME_BYTES],
            reference: [0; FRAME_BYTES],
            previous: None,
            back: 0,
            commits: 0,
            payload: 0,
        }
    }
    fn tick(&mut self, m: Metrics, live: bool, page: bool, sample: bool) -> Option<Rectangle> {
        self.ui.update(m, live, page, sample);
        let dirty = self.ui.damage();
        if let Some(dirty) = dirty {
            self.ui
                .render_with_extra_damage(&mut Frame(&mut self.buffers[self.back]), self.previous)
                .unwrap();
            self.ui
                .render_full(&mut Frame(&mut self.reference))
                .unwrap();
            let mismatch = self.buffers[self.back]
                .iter()
                .zip(&self.reference)
                .position(|(a, b)| a != b);
            assert_eq!(
                mismatch, None,
                "stale back buffer on commit {}",
                self.commits
            );
            // Model the ST7789 GRAM update: only the current dirty rectangle is sent.
            for y in
                dirty.top_left.y as usize..dirty.top_left.y as usize + dirty.size.height as usize
            {
                let start = (y * WIDTH + dirty.top_left.x as usize) * 2;
                let end = start + dirty.size.width as usize * 2;
                self.panel[start..end].copy_from_slice(&self.buffers[self.back][start..end]);
            }
            let mismatch = self
                .panel
                .iter()
                .zip(&self.reference)
                .position(|(a, b)| a != b);
            assert_eq!(
                mismatch, None,
                "incorrect LCD damage on commit {}",
                self.commits
            );
            self.payload += dirty.size.width as usize * dirty.size.height as usize * 2;
            self.previous = Some(dirty);
            self.back ^= 1;
            self.commits += 1;
        }
        dirty
    }
    fn save(&self, out: &str, name: &str) {
        save(out, name, &self.panel);
    }
}
fn save(out: &str, name: &str, bytes: &FrameBytes) {
    let mut file = fs::File::create(format!("{out}/{name}.ppm")).unwrap();
    write!(file, "P6\n{WIDTH} {HEIGHT}\n255\n").unwrap();
    for px in bytes.as_chunks::<2>().0 {
        let c = u16::from_be_bytes([px[0], px[1]]);
        file.write_all(&[
            ((c >> 11) * 255 / 31) as u8,
            (((c >> 5) & 63) * 255 / 63) as u8,
            ((c & 31) * 255 / 31) as u8,
        ])
        .unwrap();
    }
}
fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| "output".into());
    fs::create_dir_all(&out).unwrap();
    let mut preview = Preview::new();
    let mut m = Metrics {
        cpu: 427,
        memory: 632,
        rx_kib: 1280,
        tx_kib: 96,
        uptime_s: 14520,
        ..Metrics::default()
    };
    preview.tick(m, false, false, false);
    preview.save(&out, "offline");
    for n in 0..130 {
        m.sequence = n;
        m.cpu = (410.0 + 260.0 * (n as f32 * 0.45).sin() + 90.0 * (n as f32 * 0.17).cos()) as u32;
        m.memory = (620.0 + 45.0 * (n as f32 * 0.08).sin()) as u32;
        preview.tick(m, true, false, true);
    }
    m.cpu = 427;
    m.memory = 632;
    preview.tick(m, true, false, true);
    preview.save(&out, "dashboard");
    preview.tick(m, true, true, false);
    preview.save(&out, "history");
    preview.tick(m, true, false, false);
    for _ in 0..100 {
        assert!(
            preview.tick(m, true, false, false).is_none(),
            "idle frame was invalidated"
        );
    }

    m.rx_kib = 2560;
    let dirty = preview.tick(m, true, false, false).unwrap();
    let mut overlay = preview.panel;
    dirty
        .into_styled(PrimitiveStyle::with_stroke(Rgb565::MAGENTA, 1))
        .draw(&mut Frame(&mut overlay))
        .unwrap();
    save(&out, "damage-rx-only", &overlay);
    println!(
        "RX-only transfer: {}x{} = {} bytes (full frame {})",
        dirty.size.width,
        dirty.size.height,
        dirty.size.width * dirty.size.height * 2,
        FRAME_BYTES
    );
    m.tx_kib = 4096;
    preview.tick(m, true, false, false);
    m.cpu = 0;
    m.memory = 1000;
    preview.tick(m, true, false, true);
    preview.save(&out, "limits");
    m.cpu = 1000;
    m.memory = 0;
    preview.tick(m, true, false, true);
    m.cpu = 427;
    m.memory = 632;
    preview.tick(m, true, true, true);
    m.cpu = 580;
    let dirty = preview.tick(m, true, true, true).unwrap();
    println!(
        "History-only transfer: {}x{} = {} bytes",
        dirty.size.width,
        dirty.size.height,
        dirty.size.width * dirty.size.height * 2
    );
    for _ in 0..6 {
        preview.tick(m, true, false, false);
        preview.tick(m, true, true, false);
    }
    preview.tick(m, false, true, false);
    assert!(preview.tick(m, false, true, false).is_none());
    preview.tick(m, true, false, true);
    m.cpu = 99;
    m.memory = 999;
    m.rx_kib = u32::MAX;
    m.tx_kib = u32::MAX;
    m.uptime_s = u32::MAX;
    preview.tick(m, true, false, true);
    preview.save(&out, "max-values");
    m.cpu = 100;
    m.memory = 1000;
    preview.tick(m, true, false, true);
    preview.tick(m, true, true, true);
    println!("{} alternating-buffer commits match full rendering and simulated LCD; 100 idle ticks send nothing",preview.commits);
    println!(
        "Total transmitted {} bytes vs {} full-frame bytes",
        preview.payload,
        preview.commits * FRAME_BYTES
    );
}
