#![no_std]
#![no_main]

use ble_bench_payload as app;
use ble_link_protocol as bench;
use core::sync::atomic::{AtomicU32, Ordering};
use defmt_rtt as _;
mod at_setup;
use dobt01_driver::Driver;
use embassy_executor::Spawner;
use embassy_stm32::{
    bind_interrupts, peripherals,
    usart::{self, BufferedUart, Config},
};
use embassy_time::{with_timeout, Duration, Instant, Timer};
use embedded_io_async::{Read, ReadReady, Write};
use panic_halt as _;

const ACK_TRACE: bool = option_env!("G431_ACK_TRACE").is_some();
static UART_IDLE_US: AtomicU32 = AtomicU32::new(0);
static ACK_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static UART_RX_BYTES: AtomicU32 = AtomicU32::new(0);
static UART_ERROR_EVENTS: AtomicU32 = AtomicU32::new(0);
struct UartErrorCounter;
impl embassy_stm32::interrupt::typelevel::Handler<embassy_stm32::interrupt::typelevel::USART1>
    for UartErrorCounter
{
    unsafe fn on_interrupt() {
        let flags = embassy_stm32::pac::USART1.isr().read();
        if ACK_TRACE && flags.idle() {
            UART_IDLE_US.store(Instant::now().as_micros() as u32, Ordering::Relaxed);
        }
        if flags.rxne() {
            UART_RX_BYTES.fetch_add(1, Ordering::Relaxed);
        }
        if flags.pe() || flags.fe() || flags.ne() || flags.ore() {
            UART_ERROR_EVENTS.fetch_add(1, Ordering::Relaxed);
        }
    }
}
bind_interrupts!(struct Irqs { USART1 => UartErrorCounter, usart::BufferedInterruptHandler<peripherals::USART1>; RNG => embassy_stm32::rng::InterruptHandler<peripherals::RNG>; });
const MAX_PACKETS: usize = 8192;

/// Submit one complete protocol frame; never insert pauses within its bytes.
async fn send(
    uart: &mut BufferedUart<'_>,
    kind: u8,
    seq: u32,
    ack_next: u32,
    payload: &[u8],
) -> bool {
    let mut encoded = [0; bench::MAX_FRAME];
    let Some(n) = bench::encode(kind, seq, ack_next, payload, &mut encoded) else {
        return false;
    };
    with_timeout(Duration::from_secs(1), async {
        uart.write_all(&encoded[..n]).await?;
        uart.flush().await
    })
    .await
    .ok()
    .and_then(Result::ok)
    .is_some()
}

#[derive(Default)]
struct ReceiveStats {
    packets: u32,
    bytes: u32,
    duplicates: u32,
    invalid: u32,
    gaps: u32,
}
fn accept_data(
    f: &bench::Frame,
    receiver: &mut bench::sliding::Receiver,
    stats: &mut ReceiveStats,
) -> bool {
    if f.kind != bench::DATA || !app::valid(f.sequence, &f.payload[..f.len]) {
        return false;
    }
    if f.sequence < receiver.next {
        stats.duplicates += 1;
    } else if !receiver.accept(f.sequence) {
        stats.gaps += 1;
    } else {
        stats.packets += 1;
        stats.bytes += (f.len - 1) as u32;
    }
    true
}
async fn receive_acks(
    uart: &mut BufferedUart<'_>,
    window: &mut bench::sliding::Sender,
    decoder: &mut bench::Decoder,
    receiver: &mut bench::sliding::Receiver,
    stats: &mut ReceiveStats,
    pending: &mut bool,
    wait_ms: u64,
) {
    let mut input = [0; 128];
    let mut wait = wait_ms;
    loop {
        let result = if uart.read_ready().unwrap_or(false) {
            uart.read(&mut input).await.ok()
        } else if wait > 0 {
            with_timeout(Duration::from_millis(wait), uart.read(&mut input))
                .await
                .ok()
                .and_then(Result::ok)
        } else {
            None
        };
        wait = 0;
        let Some(n) = result else { break };
        for &b in &input[..n] {
            if let Some(f) = decoder.feed(b) {
                if matches!(f.kind, bench::ACK | bench::DATA) {
                    if ACK_TRACE && ACK_TRACE_COUNT.fetch_add(1, Ordering::Relaxed) < 8 {
                        defmt::info!(
                            "ACK_TRACE next={} uart_idle_us={} parsed_us={}",
                            f.ack_next,
                            UART_IDLE_US.load(Ordering::Relaxed),
                            Instant::now().as_micros() as u32
                        );
                    }
                    window.acknowledge(f.ack_next);
                }
                if accept_data(&f, receiver, stats) {
                    *pending = true;
                }
            }
        }
    }
}
async fn transmit_windowed(
    uart: &mut BufferedUart<'_>,
    next_tx: &mut u32,
    receiver: &mut bench::sliding::Receiver,
    stats: &mut ReceiveStats,
    count: u32,
    size: usize,
    frames: u32,
    byte_limit: u32,
    mixed: u8,
    decoder: &mut bench::Decoder,
) -> [u8; 33] {
    let max_wire = size + bench::OVERHEAD;
    let first = *next_tx;
    let end = first + count;
    let mut window = bench::sliding::Sender::new_at(frames, byte_limit, first).unwrap();
    let start = Instant::now();
    let mut payload = [0; bench::MAX_PAYLOAD];
    let (mut attempts, mut retries, mut status, mut piggybacks) = (0, 0, 0, 0);
    let mut pending = true; // START is acknowledged by the first DATA.
    loop {
        receive_acks(uart, &mut window, decoder, receiver, stats, &mut pending, 0).await;
        if window.base == end {
            break;
        }
        let size = if mixed == 2 {
            app::random_wire(window.next - first, true, max_wire) - bench::OVERHEAD
        } else if mixed == 1 {
            app::mixed_wire(window.next - first, true) - bench::OVERHEAD
        } else {
            size
        };
        let now = (Instant::now() - start).as_millis();
        let (id, retry) = match window.due(now) {
            bench::sliding::Due::Exhausted(_) => {
                status = 2;
                break;
            }
            bench::sliding::Due::Retry(id) => {
                window.retry(id, now);
                (id, true)
            }
            bench::sliding::Due::None => {
                if window.next < end && window.can_send(size + bench::OVERHEAD) {
                    (window.start(size + bench::OVERHEAD, now).unwrap(), false)
                } else {
                    if pending {
                        send(uart, bench::ACK, 0, receiver.next, &[]).await;
                        pending = false;
                    }
                    let delay = window.wait_ms(now).max(1);
                    receive_acks(
                        uart,
                        &mut window,
                        decoder,
                        receiver,
                        stats,
                        &mut pending,
                        delay,
                    )
                    .await;
                    continue;
                }
            }
        };
        receive_acks(uart, &mut window, decoder, receiver, stats, &mut pending, 0).await;
        if !window.pending(id) {
            continue;
        }
        let size = if mixed == 2 {
            app::random_wire(id - first, true, max_wire) - bench::OVERHEAD
        } else if mixed == 1 {
            app::mixed_wire(id - first, true) - bench::OVERHEAD
        } else {
            size
        };
        app::fill(id, &mut payload[..size]);
        attempts += 1;
        retries += u32::from(retry);
        if ACK_TRACE && attempts <= 8 {
            defmt::info!(
                "DATA_TRACE id={} attempt={} begin_us={}",
                id,
                attempts,
                Instant::now().as_micros() as u32
            );
        }
        if !send(uart, bench::DATA, id, receiver.next, &payload[..size]).await {
            status = 1;
            break;
        }
        if ACK_TRACE && attempts <= 8 {
            defmt::info!(
                "DATA_TRACE id={} flushed_us={}",
                id,
                Instant::now().as_micros() as u32
            );
        }
        piggybacks += u32::from(pending);
        pending = false;
        window.submitted(id, (Instant::now() - start).as_millis());
    }
    *next_tx = window.next;
    app::report(
        app::DONE,
        [
            window.base - first,
            attempts,
            retries,
            piggybacks,
            (Instant::now() - start).as_micros().min(u32::MAX as u64) as u32,
            status,
            window.next,
            retries,
        ],
    )
}

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let mut config = embassy_stm32::Config::default();
    {
        use embassy_stm32::rcc::*;
        config.rcc.hsi = true;
        config.rcc.pll = Some(Pll {
            source: PllSource::HSI,
            prediv: PllPreDiv::DIV4,
            mul: PllMul::MUL32,
            divp: None,
            divq: None,
            divr: Some(PllRDiv::DIV2),
        });
        config.rcc.sys = Sysclk::PLL1_R;
    }
    let p = embassy_stm32::init(config);
    let mut cfg = Config::default();
    cfg.baudrate = 115200;
    cfg.eager_reads = Some(64);
    let mut tx = [0; 2048];
    let mut rx = [0; 8192];
    let mut uart = BufferedUart::new(p.USART1, p.PA10, p.PA9, &mut tx, &mut rx, Irqs, cfg).unwrap();
    let mut driver = Driver::new();
    defmt::info!("STM32G431 doBT-01X: USART1 TX=PA9 RX=PA10; HSI PLL 64MHz, RX=8192 TX=2048");
    let target_baud = option_env!("G431_BENCH_BAUD")
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(115200);
    let ready = match at_setup::configure(
        &mut driver,
        &mut uart,
        at_setup::Options {
            name: "G431-BLE",
            baud: target_baud,
            ensure_slave: true,
        },
    )
    .await
    {
        Ok(ready) => ready,
        Err(e) => {
            defmt::error!("AT setup failed: {}", defmt::Debug2Format(&e));
            loop {
                Timer::after_secs(1).await;
            }
        }
    };
    let baud = ready.baud;
    defmt::info!(
        "AT_READY detected={} verified={} role_supported={} connection_supported={}",
        ready.detected_baud,
        ready.baud,
        ready.role.is_some(),
        ready.connection.is_some()
    );
    defmt::info!(
        "WAIT_BLE: connect Mac, subscribe notify, send binary HELLO. No AT polling in data mode."
    );

    let mut decoder = bench::Decoder::new();
    let mut readbuf = [0; 512];
    let mut receiver = bench::sliding::Receiver::default();
    let mut next_tx = 0u32;
    let mut byte_limit = 8192u32;
    let mut active = false;
    let mut completed: Option<[u8; 33]> = None;
    let mut stats = ReceiveStats::default();
    loop {
        let n = match with_timeout(Duration::from_secs(1), uart.read(&mut readbuf)).await {
            Ok(Ok(n)) => n,
            _ => {
                decoder.expire_partial();
                continue;
            }
        };
        for &b in &readbuf[..n] {
            let Some(f) = decoder.feed(b) else { continue };
            if f.kind == bench::HELLO
                && f.sequence == 0
                && f.len == 12
                && &f.payload[..4] == bench::HELLO_MAGIC
            {
                let budget = u32::from_le_bytes(f.payload[4..8].try_into().unwrap());
                let floor = u32::from_le_bytes(f.payload[8..12].try_into().unwrap());
                if !(64..=8192).contains(&budget) {
                    continue;
                }
                driver.confirm_peer();
                // Explicit test boundary retires any previously reported failed tail.
                if floor >= receiver.next {
                    if ACK_TRACE {
                        ACK_TRACE_COUNT.store(0, Ordering::Relaxed);
                    }
                    receiver.next = floor;
                    byte_limit = budget;
                    active = true;
                    completed = None;
                    stats = ReceiveStats::default();
                    decoder.errors = 0;
                    UART_ERROR_EVENTS.store(0, Ordering::Relaxed);
                }
                let mut ready = [0; 20];
                ready[..4].copy_from_slice(&baud.to_le_bytes());
                ready[4..6].copy_from_slice(&(bench::MAX_PAYLOAD as u16).to_le_bytes());
                ready[6..8].copy_from_slice(&(MAX_PACKETS as u16).to_le_bytes());
                ready[8..10].copy_from_slice(&bench::VERSION.to_le_bytes());
                ready[10..12].copy_from_slice(&8u16.to_le_bytes());
                ready[12..16].copy_from_slice(&byte_limit.to_le_bytes());
                ready[16..20].copy_from_slice(&next_tx.to_le_bytes());
                send(&mut uart, bench::HELLO, 0, receiver.next, &ready).await;
                continue;
            }
            if !active {
                continue;
            }
            if f.kind == bench::ACK && (f.len == 1 || f.len == 2) && f.payload[0] == app::QUERY {
                let cached = if f.len == 2 && f.payload[1] == app::STATS {
                    None
                } else {
                    completed
                };
                let report = cached.unwrap_or_else(|| {
                    app::report(
                        app::STATS,
                        [
                            stats.packets,
                            stats.bytes,
                            stats.duplicates,
                            stats.invalid,
                            decoder.errors,
                            UART_ERROR_EVENTS.load(Ordering::Relaxed),
                            0,
                            stats.gaps,
                        ],
                    )
                });
                send(&mut uart, bench::ACK, 0, receiver.next, &report).await;
            } else if f.kind == bench::DATA && f.len > 0 {
                if f.payload[0] == app::TEST {
                    if !accept_data(&f, &mut receiver, &mut stats) {
                        stats.invalid += 1;
                        continue;
                    }
                    send(&mut uart, bench::ACK, 0, receiver.next, &[]).await;
                } else if f.payload[0] == app::START
                    && (f.len == 9 || (f.len == 10 && matches!(f.payload[9], 1 | 2)))
                {
                    if let Some(done) = completed {
                        send(&mut uart, bench::ACK, 0, receiver.next, &done).await;
                        continue;
                    }
                    let count = u32::from_le_bytes(f.payload[1..5].try_into().unwrap());
                    let size = u16::from_le_bytes(f.payload[5..7].try_into().unwrap()) as usize;
                    let width = u16::from_le_bytes(f.payload[7..9].try_into().unwrap()) as u32;
                    if count == 0
                        || count > MAX_PACKETS as u32
                        || !(2..=bench::MAX_PAYLOAD).contains(&size)
                        || (f.len == 10 && f.payload[9] == 1 && size + bench::OVERHEAD != 1024)
                        || (f.len == 10
                            && f.payload[9] == 2
                            && size + bench::OVERHEAD < app::RANDOM_MIN_WIRE)
                        || !(1..=8).contains(&width)
                        || size + bench::OVERHEAD > byte_limit as usize
                        || next_tx.checked_add(count).is_none()
                    {
                        continue;
                    }
                    if receiver.accept(f.sequence) {
                        let done = transmit_windowed(
                            &mut uart,
                            &mut next_tx,
                            &mut receiver,
                            &mut stats,
                            count,
                            size,
                            width,
                            byte_limit,
                            if f.len == 10 { f.payload[9] } else { 0 },
                            &mut decoder,
                        )
                        .await;
                        completed = Some(done);
                        send(&mut uart, bench::ACK, 0, receiver.next, &done).await;
                    } else {
                        send(&mut uart, bench::ACK, 0, receiver.next, &[]).await;
                    }
                }
            }
        }
    }
}
