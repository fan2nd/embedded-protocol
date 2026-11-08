#![no_std]
#![no_main]

mod display;
mod lcd;
mod storage;

use core::{cell::RefCell, ptr::addr_of_mut};
use embassy_executor::{Executor, Spawner};
use embassy_futures::join::join;
use embassy_rp::{
    bind_interrupts,
    gpio::{Input, Level, Output, Pull},
    multicore::{spawn_core1, Stack},
    peripherals::USB,
    spi::{Config as SpiConfig, Spi},
    usb::{Driver, InterruptHandler},
    Peri,
};
use embassy_sync::blocking_mutex::{raw::CriticalSectionRawMutex, Mutex};
use embassy_time::{with_timeout, Duration, Instant, Timer};
use embassy_usb::{
    class::cdc_acm::{CdcAcmClass, State},
    Builder, Config,
};
use monitor_protocol::{Decoder, Message, Metrics, READY};
use panic_halt as _;
use rp2350_dashboard::{Dashboard, Frame, FrameBytes, FRAME_BYTES};
use static_cell::StaticCell;

bind_interrupts!(struct UsbIrqs { USBCTRL_IRQ => InterruptHandler<USB>; });

#[derive(Clone, Copy, Default)]
struct Snapshot {
    metrics: Metrics,
    received_ms: u64,
    generation: u32,
    valid: bool,
}
// embassy-rp's critical-section-impl uses hardware spinlocks across both cores.
// Only copy a small snapshot under the lock; never render or await while locked.
static SHARED: Mutex<CriticalSectionRawMutex, RefCell<Snapshot>> =
    Mutex::new(RefCell::new(Snapshot {
        metrics: Metrics {
            sequence: 0,
            cpu: 0,
            memory: 0,
            rx_kib: 0,
            tx_kib: 0,
            uptime_s: 0,
        },
        received_ms: 0,
        generation: 0,
        valid: false,
    }));
static mut CORE1_STACK: Stack<16384> = Stack::new();
static CORE1_EXECUTOR: StaticCell<Executor> = StaticCell::new();
static FRAME_A: StaticCell<FrameBytes> = StaticCell::new();
static FRAME_B: StaticCell<FrameBytes> = StaticCell::new();

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let mut config = embassy_rp::config::Config::default();
    config.clocks = embassy_rp::clocks::ClockConfig::system_freq(150_000_000).unwrap();
    let p = embassy_rp::init(config);
    // Move only USB to core 1. The driver (and its IRQ) is initialized there.
    spawn_core1(
        p.CORE1,
        unsafe { &mut *addr_of_mut!(CORE1_STACK) },
        move || {
            CORE1_EXECUTOR.init(Executor::new()).run(|s| {
                s.spawn(usb_core(p.USB)).unwrap();
            })
        },
    );

    // TF DATA3 is SPI CS. LCD shares SCK/MOSI with the card socket.
    let _tf_cs = Output::new(p.PIN_15, Level::High);
    let mut led = Output::new(p.PIN_3, Level::High); // active low
    let key = Input::new(p.PIN_2, Pull::Up);
    let mut spi_config = SpiConfig::default();
    spi_config.frequency = 32_000_000;
    let spi = Spi::new_txonly(p.SPI1, p.PIN_10, p.PIN_11, p.DMA_CH0, spi_config);
    let lcd = lcd::Lcd::new(
        spi,
        Output::new(p.PIN_9, Level::High),
        Output::new(p.PIN_8, Level::Low),
        Output::new(p.PIN_25, Level::High),
    );
    let initial = FRAME_A.init_with(|| [0; FRAME_BYTES]);
    let mut frame = FRAME_B.init_with(|| [0; FRAME_BYTES]);
    spawner.spawn(display::transfer(lcd, initial)).unwrap();
    let mut ui = Dashboard::new();
    let mut previous_damage = None;
    let mut last_generation = 0;
    let mut page = false;
    let mut stable_down = false;
    let mut candidate_down = false;
    let mut candidate_since = 0;
    loop {
        let now = Instant::now().as_millis();
        let down = key.is_low();
        if down != candidate_down {
            candidate_down = down;
            candidate_since = now;
        }
        if candidate_down != stable_down && now - candidate_since >= 40 {
            stable_down = candidate_down;
            if stable_down {
                page = !page;
            }
        }
        let snapshot = SHARED.lock(|s| *s.borrow());
        let live = snapshot.valid && now.saturating_sub(snapshot.received_ms) < 3000;
        let new_sample = live && snapshot.generation != last_generation;
        if new_sample {
            last_generation = snapshot.generation;
        }
        if live {
            led.set_low();
        } else {
            led.set_high();
        }
        ui.update(snapshot.metrics, live, page, new_sample, now);
        if let Some(damage) = ui.damage() {
            // Alternating buffers are one update apart. Repair the previous damage
            // in RAM, but send only this update's damage to the serial LCD.
            ui.render_with_extra_damage(&mut Frame(frame), previous_damage)
                .unwrap();
            previous_damage = Some(damage);
            display::READY
                .send(display::FrameJob { frame, damage })
                .await;
            // Ownership comes back only after DMA AND SPI FIFO draining complete.
            frame = display::FREE.receive().await;
        }
        // Poll keys/offline state with at most 1 ms added display latency.
        Timer::after_millis(1).await;
    }
}

#[embassy_executor::task]
async fn usb_core(usb_peripheral: Peri<'static, USB>) {
    let driver = Driver::new(usb_peripheral, UsbIrqs);
    // Demo-only identity, not an allocated production VID/PID.
    let mut config = Config::new(0xcafe, 0x4001);
    config.manufacturer = Some("ATK Demo");
    config.product = Some("RP2350 PC Monitor");
    config.serial_number = Some("PCMON-DEMO-001");
    config.max_power = 250;
    config.max_packet_size_0 = 64;
    let mut config_descriptor = [0; 384];
    let mut bos_descriptor = [0; 256];
    let mut control_buffer = [0; 64];
    let mut state = State::new();
    let mut disk_control = storage::Control::new();
    let mut builder = Builder::new(
        driver,
        config,
        &mut config_descriptor,
        &mut bos_descriptor,
        &mut [],
        &mut control_buffer,
    );
    let mut cdc = CdcAcmClass::new(&mut builder, &mut state, 64);
    let mut disk = storage::Disk::new(&mut builder, &mut disk_control);
    let mut device = builder.build();
    let receiver = async {
        loop {
            cdc.wait_connection().await;
            let mut decoder = Decoder::new();
            let mut packet = [0; 64];
            loop {
                let count = match cdc.read_packet(&mut packet).await {
                    Ok(n) => n,
                    Err(_) => break,
                };
                for &b in &packet[..count] {
                    match decoder.feed(b) {
                        Some(Message::Hello) => {
                            // A host that never reads replies cannot starve receive forever.
                            let _ =
                                with_timeout(Duration::from_millis(250), cdc.write_packet(READY))
                                    .await;
                        }
                        Some(Message::Metrics(metrics)) => {
                            SHARED.lock(|s| {
                                let mut s = s.borrow_mut();
                                s.metrics = metrics;
                                s.received_ms = Instant::now().as_millis();
                                s.generation = s.generation.wrapping_add(1);
                                s.valid = true;
                            });
                        }
                        None => {}
                    }
                }
            }
            SHARED.lock(|s| s.borrow_mut().valid = false);
        }
    };
    join(device.run(), join(receiver, disk.run())).await;
}
