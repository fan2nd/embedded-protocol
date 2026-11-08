use dashboard::protocol::Metrics;
use serialport::{SerialPort, SerialPortInfo, SerialPortType};
use std::{
    collections::BTreeMap,
    env,
    error::Error,
    io::{self, Read, Write},
    thread,
    time::{Duration, Instant, SystemTime},
};
use sysinfo::{Networks, System};
mod platform;

const VID: u16 = 0xcafe;
const PID: u16 = 0x4001;
type Counters = BTreeMap<String, (u64, u64)>;

#[derive(Default)]
struct Args {
    port: Option<String>,
    interface: Option<String>,
    dry_run: bool,
    demo: bool,
    list: bool,
    list_interfaces: bool,
    count: usize,
    interval: Option<f64>,
}
fn arguments() -> Result<Args, Box<dyn Error>> {
    let mut result = Args::default();
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--port" => result.port = Some(args.next().ok_or("--port needs a value")?),
            "--interface" => {
                result.interface = Some(args.next().ok_or("--interface needs a value")?)
            }
            "--interval" => {
                result.interval = Some(args.next().ok_or("--interval needs seconds")?.parse()?)
            }
            "--count" => result.count = args.next().ok_or("--count needs a value")?.parse()?,
            "--dry-run" => result.dry_run = true,
            "--demo" => result.demo = true,
            "--list-ports" => result.list = true,
            "--list-interfaces" => result.list_interfaces = true,
            "--help" | "-h" => {
                println!(
                    "pc-monitor-host [--port PORT] [--interface NAME]\n\
                          [--interval 0.5] [--dry-run] [--demo] [--count N]\n\
                          [--list-ports] [--list-interfaces]\n\
                          Windows / Linux / macOS: system metrics, auto USB selection, Ctrl-C to stop.\n\
                          --dry-run prints packets; --demo uses synthetic data.\n\
                          Ports: COM3 (Windows), /dev/ttyACM0 (Linux), /dev/cu.usbmodem... (macOS).\n\
                          --list-interfaces shows the default network selection. --count 0 runs forever."
                );
                std::process::exit(0);
            }
            _ => return Err(format!("unknown argument: {arg}").into()),
        }
    }
    let interval = result.interval.unwrap_or(0.5);
    if !interval.is_finite() || !(0.25..=2.0).contains(&interval) {
        return Err("--interval must be between 0.25 and 2 seconds".into());
    }
    Ok(result)
}

fn rates(previous: &Counters, current: &Counters, elapsed: f64) -> (u32, u32) {
    if elapsed <= 0.0 {
        return (0, 0);
    }
    let (mut rx, mut tx) = (0u64, 0u64);
    for (name, &(r, t)) in current {
        if let Some(&(old_r, old_t)) = previous.get(name) {
            rx = rx.saturating_add(r.saturating_sub(old_r));
            tx = tx.saturating_add(t.saturating_sub(old_t));
        }
    }
    (
        (rx as f64 / elapsed / 1024.0).round() as u32,
        (tx as f64 / elapsed / 1024.0).round() as u32,
    )
}

struct Sampler {
    system: System,
    networks: Networks,
    previous: Counters,
    at: Instant,
    interface: Option<String>,
    boot_time: u64,
}
impl Sampler {
    fn new(interface: Option<String>) -> Result<Self, Box<dyn Error>> {
        let boot_time = System::boot_time();
        if boot_time == 0 {
            return Err(
                "Cannot read system boot time; run outside a restricted sandbox/container".into(),
            );
        }
        let mut this = Self {
            system: System::new(),
            networks: Networks::new_with_refreshed_list(),
            previous: BTreeMap::new(),
            at: Instant::now(),
            interface,
            boot_time,
        };
        this.system.refresh_cpu_usage();
        if let Some(name) = &this.interface {
            if !this.networks.iter().any(|(n, _)| n == name) {
                return Err(format!("network interface not found: {name}").into());
            }
        }
        this.previous = this.counters();
        this.at = Instant::now();
        Ok(this)
    }
    fn counters(&self) -> Counters {
        self.networks
            .iter()
            .filter(|(name, data)| {
                platform::selected_interface(name, data, self.interface.as_deref())
            })
            .map(|(n, d)| (n.clone(), (d.total_received(), d.total_transmitted())))
            .collect()
    }
    fn sample(&mut self, sequence: u32) -> Metrics {
        self.system.refresh_cpu_usage();
        self.system.refresh_memory();
        self.networks.refresh(true);
        let now = Instant::now();
        let current = self.counters();
        let (rx_kib, tx_kib) = rates(&self.previous, &current, (now - self.at).as_secs_f64());
        self.previous = current;
        self.at = now;
        Metrics {
            sequence,
            cpu: (self.system.global_cpu_usage() * 10.0)
                .round()
                .clamp(0.0, 1000.0) as u32,
            memory: ((self.system.used_memory() as f64 / self.system.total_memory().max(1) as f64)
                * 1000.0)
                .round()
                .clamp(0.0, 1000.0) as u32,
            rx_kib,
            tx_kib,
            uptime_s: SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
                .saturating_sub(self.boot_time)
                .min(u32::MAX as u64) as u32,
        }
    }
}

fn encode(m: Metrics) -> String {
    format!(
        "PCMON1,{},{},{},{},{},{}\n",
        m.sequence, m.cpu, m.memory, m.rx_kib, m.tx_kib, m.uptime_s
    )
}
fn choose_port(ports: Vec<SerialPortInfo>) -> Result<Option<String>, Box<dyn Error>> {
    // macOS exposes both /dev/tty.* and /dev/cu.*; use the callout device only.
    let mut matches = ports.into_iter().filter(|p| {
        (!cfg!(target_os = "macos") || p.port_name.starts_with("/dev/cu.")) &&
        matches!(&p.port_type, SerialPortType::UsbPort(u) if u.vid == VID && u.pid == PID)
    }).map(|p| p.port_name);
    let first = matches.next();
    if matches.next().is_some() {
        return Err("multiple monitors: select one with --port".into());
    }
    Ok(first)
}
fn handshake<T: Read + Write + ?Sized>(port: &mut T) -> io::Result<()> {
    port.write_all(b"HELLO\n")?;
    let start = Instant::now();
    let mut line = Vec::new();
    let mut dropping = false;
    let mut byte = [0; 1];
    while start.elapsed() < Duration::from_secs(2) {
        match port.read(&mut byte) {
            Ok(0) => return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "USB closed")),
            Ok(_) => {
                if byte[0] == b'\n' {
                    if !dropping && line == b"PCMON1 READY" {
                        return Ok(());
                    }
                    line.clear();
                    dropping = false;
                } else if !dropping {
                    if line.len() == 127 {
                        dropping = true;
                    } else {
                        line.push(byte[0]);
                    }
                }
            }
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::TimedOut
                        | io::ErrorKind::WouldBlock
                        | io::ErrorKind::Interrupted
                ) => {}
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::TimedOut,
        "no PCMON1 reply; use native USB, not CH343",
    ))
}
fn connect(path: &str) -> Result<Box<dyn SerialPort>, Box<dyn Error>> {
    let mut port = serialport::new(path, 115200)
        .timeout(Duration::from_millis(250))
        .dtr_on_open(true)
        .open()?;
    port.clear(serialport::ClearBuffer::Input)?;
    handshake(port.as_mut())?;
    Ok(port)
}
fn main() -> Result<(), Box<dyn Error>> {
    let args = arguments()?;
    if args.list {
        for p in serialport::available_ports()? {
            println!("{} {:?}", p.port_name, p.port_type);
        }
        return Ok(());
    }
    if args.list_interfaces {
        for (name, data) in Networks::new_with_refreshed_list().iter() {
            println!(
                "{} {name}  {:?}",
                if platform::selected_interface(name, data, None) {
                    "*"
                } else {
                    " "
                },
                data.ip_networks()
            );
        }
        println!("* included by default; --interface NAME overrides selection");
        return Ok(());
    }
    let interval = Duration::from_secs_f64(args.interval.unwrap_or(0.5));
    let mut sampler = if args.demo {
        None
    } else {
        Some(Sampler::new(args.interface)?)
    };
    let mut port: Option<Box<dyn SerialPort>> = None;
    let mut sent = 0usize;
    let mut waiting = false;
    thread::sleep(interval); // prime CPU/network deltas
    while args.count == 0 || sent < args.count {
        if !args.dry_run && port.is_none() {
            let path = match &args.port {
                Some(p) => Some(p.clone()),
                None => choose_port(serialport::available_ports()?)?,
            };
            if let Some(path) = path {
                match connect(&path) {
                    Ok(p) => {
                        eprintln!("Connected: {path}");
                        port = Some(p);
                        waiting = false;
                    }
                    Err(e) => {
                        eprintln!("USB: {e}; retrying in 2s");
                        if !waiting {
                            eprintln!("{}", platform::serial_permission_hint());
                            waiting = true;
                        }
                        thread::sleep(Duration::from_secs(2));
                        continue;
                    }
                }
            } else {
                if !waiting {
                    eprintln!("Waiting for RP2350 native USB...");
                    waiting = true;
                }
                thread::sleep(Duration::from_secs(2));
                continue;
            }
        }
        let m = match sampler.as_mut() {
            Some(s) => s.sample(sent as u32),
            None => Metrics {
                sequence: sent as u32,
                cpu: (500.0 + 400.0 * (sent as f64 / 8.0).sin()) as u32,
                memory: 632,
                rx_kib: 1280,
                tx_kib: 96,
                uptime_s: (sent as f64 * interval.as_secs_f64()) as u32,
            },
        };
        let packet = encode(m);
        if args.dry_run {
            print!("{packet}");
            io::stdout().flush()?;
        } else if let Err(e) = port.as_mut().unwrap().write_all(packet.as_bytes()) {
            eprintln!("Disconnected: {e}; reconnecting");
            port = None;
            thread::sleep(Duration::from_secs(2));
            continue;
        }
        sent += 1;
        thread::sleep(interval);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use dashboard::protocol::{Decoder, Message};
    #[test]
    fn host_packet_survives_firmware_stream_decoder() {
        let m = Metrics {
            sequence: u32::MAX,
            cpu: 427,
            memory: 632,
            rx_kib: 1280,
            tx_kib: 96,
            uptime_s: 10,
        };
        let mut decoder = Decoder::new();
        let messages: Vec<_> = encode(m).bytes().filter_map(|b| decoder.feed(b)).collect();
        assert_eq!(messages, vec![Message::Metrics(m)]);
    }
    #[test]
    fn network_resets_and_added_interfaces() {
        let old = BTreeMap::from([("en0".into(), (1024, 2048))]);
        let new = BTreeMap::from([
            ("en0".into(), (3072, 3072)),
            ("en1".into(), (9999999, 9999999)),
        ]);
        assert_eq!(rates(&old, &new, 0.5), (4, 2));
        assert_eq!(rates(&new, &old, 0.5), (0, 0));
        assert_eq!(rates(&old, &new, 0.0), (0, 0));
    }
    #[test]
    fn handshake_with_fragmented_reply() {
        struct Duplex {
            input: io::Cursor<Vec<u8>>,
            output: Vec<u8>,
        }
        impl Read for Duplex {
            fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
                self.input.read(&mut b[..1])
            }
        }
        impl Write for Duplex {
            fn write(&mut self, b: &[u8]) -> io::Result<usize> {
                self.output.extend_from_slice(b);
                Ok(b.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let mut host = Duplex {
            input: io::Cursor::new(b"noise\nPCMON1 READY\n".to_vec()),
            output: Vec::new(),
        };
        handshake(&mut host).unwrap();
        assert_eq!(host.output, b"HELLO\n");
    }
    #[test]
    fn handshake_rejects_closed_or_wrong_device() {
        let mut data = io::Cursor::new(b"wrong reply\n".to_vec());
        assert!(handshake(&mut data).is_err());
    }
    #[test]
    fn automatic_port_selection_excludes_ch343() {
        fn port(name: &str, vid: u16, pid: u16) -> SerialPortInfo {
            SerialPortInfo {
                port_name: name.into(),
                port_type: SerialPortType::UsbPort(serialport::UsbPortInfo {
                    vid,
                    pid,
                    serial_number: None,
                    manufacturer: None,
                    product: None,
                }),
            }
        }
        assert!(choose_port(vec![port("/dev/cu.ch343", 0x1a86, 0x55d3)])
            .unwrap()
            .is_none());
        assert_eq!(
            choose_port(vec![port("/dev/cu.monitor", VID, PID)]).unwrap(),
            Some("/dev/cu.monitor".into())
        );
        assert!(choose_port(vec![
            port("/dev/cu.a", VID, PID),
            port("/dev/cu.b", VID, PID)
        ])
        .is_err());
    }
}
