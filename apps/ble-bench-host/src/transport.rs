use ble_link_protocol as p;
use bluest::{Adapter, Characteristic, Device};
use futures_lite::StreamExt;
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tokio::sync::mpsc;
pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
#[derive(Default)]
struct ReceiveStats {
    calls: u64,
    bytes: u64,
    empty: u64,
    repeated: u64,
    last: Vec<u8>,
    samples: Vec<String>,
    error: Option<String>,
}
pub struct Ack {
    pub window: p::sliding::Sender,
}
pub struct Link {
    pub adapter: Adapter,
    pub device: Device,
    pub characteristic: Characteristic,
    pub chunk: usize,
    pub expected_baud: u32,
    pub gatt: serde_json::Value,
    receive_stats: Arc<Mutex<ReceiveStats>>,
    rx: mpsc::Receiver<Vec<u8>>,
    task: tokio::task::JoinHandle<()>,
    overflow: Arc<AtomicBool>,
    decoder: p::Decoder,
    queue: VecDeque<(p::Frame, Instant)>,
    pub ack: Option<Ack>,
    pub receive_next: u32,
    pub transmit_next: u32,
    last: Instant,
    pub raw_bytes: u64,
    pub queue_peak: usize,
}
impl Link {
    pub async fn connect(name: &str, expected_baud: u32) -> Result<Self> {
        let adapter = Adapter::default().await.ok_or("No Bluetooth adapter")?;
        tokio::time::timeout(Duration::from_secs(20), adapter.wait_available()).await??;
        let device = {
            let mut scan = adapter.scan(&[]).await?;
            tokio::time::timeout(Duration::from_secs(30), async {
                while let Some(found) = scan.next().await {
                    if found.device.name().unwrap_or_default() == name {
                        return Ok(found.device);
                    }
                }
                Err::<Device, Box<dyn std::error::Error + Send + Sync>>("Device not found".into())
            })
            .await??
        };
        tokio::time::timeout(Duration::from_secs(20), adapter.connect_device(&device)).await??;
        let mut selected = None;
        let mut notifiable = None;
        let mut gatt = Vec::new();
        for service in device.discover_services().await? {
            for c in service.discover_characteristics().await? {
                let id = c.uuid().to_string();
                let props = c.properties().await?;
                gatt.push(serde_json::json!({"uuid":id,"read":props.read,"notify":props.notify,"write_without_response":props.write_without_response}));
                if id == "0000a003-0000-1000-8000-00805f9b34fb" && props.write_without_response {
                    selected = Some(c.clone());
                }
                if id == "0000a002-0000-1000-8000-00805f9b34fb" && props.notify {
                    notifiable = Some(c);
                }
            }
        }
        let characteristic = selected.ok_or("A003 write-without-response missing")?;
        let receiver = notifiable.ok_or("A002 NOTIFY characteristic missing")?;
        let chunk = characteristic.max_write_len()?.min(182);
        if chunk == 0 {
            return Err("zero ATT capacity".into());
        }
        let (tx, rx) = mpsc::channel(512);
        let overflow = Arc::new(AtomicBool::new(false));
        let failed = overflow.clone();
        let receive_stats = Arc::new(Mutex::new(ReceiveStats::default()));
        let counters = receive_stats.clone();
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let mut stream = match receiver.notify().await {
                Ok(stream) => {
                    let _ = ready_tx.send(Ok::<(), String>(()));
                    stream
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e.to_string()));
                    return;
                }
            };
            while let Some(value) = stream.next().await {
                match value {
                    Ok(bytes) => {
                        {
                            let mut s = counters.lock().unwrap();
                            s.calls += 1;
                            s.bytes += bytes.len() as u64;
                            if bytes.is_empty() {
                                s.empty += 1;
                            }
                            if s.calls > 1 && bytes == s.last {
                                s.repeated += 1;
                            }
                            if s.samples.len() < 16 {
                                s.samples
                                    .push(bytes.iter().map(|b| format!("{b:02x}")).collect());
                            }
                            s.last = bytes.clone();
                        }
                        if !bytes.is_empty() && tx.try_send(bytes).is_err() {
                            counters.lock().unwrap().error =
                                Some("notification receive queue overflow".into());
                            failed.store(true, Ordering::Relaxed);
                            return;
                        }
                    }
                    Err(e) => {
                        counters.lock().unwrap().error = Some(e.to_string());
                        failed.store(true, Ordering::Relaxed);
                        return;
                    }
                }
            }
            counters.lock().unwrap().error = Some("notification stream ended".into());
            failed.store(true, Ordering::Relaxed);
        });
        ready_rx
            .await?
            .map_err(|e| format!("A002 notify subscription failed: {e}"))?;
        Ok(Self {
            adapter,
            device,
            characteristic,
            chunk,
            expected_baud,
            gatt: serde_json::json!(gatt),
            receive_stats,
            rx,
            task,
            overflow,
            decoder: p::Decoder::new(),
            queue: VecDeque::new(),
            ack: None,
            receive_next: 0,
            transmit_next: 0,
            last: Instant::now(),
            raw_bytes: 0,
            queue_peak: 0,
        })
    }
    fn ingest(&mut self, bytes: &[u8]) -> Result<()> {
        let now = Instant::now();
        if now.duration_since(self.last) >= Duration::from_secs(1) {
            self.decoder.expire_partial()
        }
        self.last = now;
        self.raw_bytes += bytes.len() as u64;
        for &b in bytes {
            if let Some(f) = self.decoder.feed(b) {
                if let Some(a) = self.ack.as_mut() {
                    if matches!(f.kind, p::ACK | p::DATA) {
                        a.window.acknowledge(f.ack_next);
                    }
                    if f.kind == p::ACK && f.len == 0 {
                        continue;
                    }
                }
                if self.queue.len() == 1024 {
                    return Err("decoded queue overflow".into());
                }
                self.queue.push_back((f, now));
                self.queue_peak = self.queue_peak.max(self.queue.len());
            }
        }
        Ok(())
    }
    pub fn drain(&mut self) -> Result<()> {
        if self.overflow.load(Ordering::Relaxed) {
            return Err(format!(
                "notification receive failed: {:?}",
                self.receive_stats.lock().unwrap().error
            )
            .into());
        }
        while let Ok(bytes) = self.rx.try_recv() {
            self.ingest(&bytes)?
        }
        Ok(())
    }
    pub fn reset_stats(&mut self) {
        self.decoder.errors = 0;
        self.raw_bytes = 0;
        self.queue_peak = 0;
    }
    pub fn receive_stats(&self) -> serde_json::Value {
        let s = self.receive_stats.lock().unwrap();
        serde_json::json!({"notifications":s.calls,"bytes":s.bytes,"empty_notifications":s.empty,"same_as_previous":s.repeated,"first_values_hex":s.samples,"error":s.error})
    }
    pub fn errors(&self) -> u32 {
        self.decoder.errors
    }
    pub async fn send(
        &mut self,
        kind: u8,
        id: u32,
        payload: &[u8],
        cancel_data: bool,
    ) -> Result<usize> {
        let mut raw = [0; p::MAX_FRAME];
        let n = p::encode(
            kind,
            if kind == p::DATA { id } else { 0 },
            self.receive_next,
            payload,
            &mut raw,
        )
        .ok_or("invalid frame")?;
        self.send_raw(&raw[..n], cancel_data.then_some(id)).await
    }
    pub async fn send_raw(&mut self, raw: &[u8], cancel_id: Option<u32>) -> Result<usize> {
        let mut sent = 0;
        for data in raw.chunks(self.chunk) {
            let c = self.characteristic.clone();
            let write = c.write_without_response(data);
            tokio::pin!(write);
            let deadline = tokio::time::sleep(Duration::from_secs(120));
            tokio::pin!(deadline);
            loop {
                self.drain()?;
                if cancel_id.is_some_and(|id| self.ack.as_ref().is_some_and(|a| id < a.window.base))
                {
                    return Ok(sent);
                }
                tokio::select! {biased; bytes=self.rx.recv()=>{self.ingest(&bytes.ok_or("notification task ended")?)?;}, result=&mut write=>{result?;sent+=data.len();break}, _=&mut deadline=>return Err("native BLE write timeout".into())}
            }
        }
        Ok(sent)
    }
    pub async fn next(&mut self, limit: Duration) -> Result<(p::Frame, Instant)> {
        let until = tokio::time::Instant::now() + limit;
        loop {
            self.drain()?;
            if let Some(f) = self.queue.pop_front() {
                return Ok(f);
            }
            let bytes = tokio::time::timeout_at(until, self.rx.recv())
                .await?
                .ok_or("notification task ended")?;
            self.ingest(&bytes)?
        }
    }
    pub fn try_next(&mut self) -> Result<Option<(p::Frame, Instant)>> {
        self.drain()?;
        Ok(self.queue.pop_front())
    }
    pub async fn pump(&mut self, limit: Duration) -> Result<()> {
        self.drain()?;
        if !self.queue.is_empty() {
            return Ok(());
        }
        match tokio::time::timeout(limit, self.rx.recv()).await {
            Ok(Some(bytes)) => self.ingest(&bytes)?,
            Ok(None) => return Err("notification task ended".into()),
            Err(_) => {}
        }
        Ok(())
    }
    pub async fn wait_ack(&mut self, before: u32, limit: Duration) -> Result<bool> {
        let until = tokio::time::Instant::now() + limit;
        loop {
            self.drain()?;
            if self.ack.as_ref().unwrap().window.base > before {
                return Ok(true);
            }
            match tokio::time::timeout_at(until, self.rx.recv()).await {
                Ok(Some(bytes)) => self.ingest(&bytes)?,
                Ok(None) => return Err("notification task ended".into()),
                Err(_) => return Ok(false),
            }
        }
    }
    pub async fn close(&mut self) {
        self.task.abort();
        let _ = self.adapter.disconnect_device(&self.device).await;
    }
}
impl Drop for Link {
    fn drop(&mut self) {
        self.task.abort();
    }
}
