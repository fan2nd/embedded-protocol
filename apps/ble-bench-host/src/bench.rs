use crate::transport::{Ack, Link, Result};
use ble_link_protocol as p;
use serde_json::{json, Value};
use std::time::{Duration, Instant};
fn words(bytes: &[u8]) -> Vec<u32> {
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| u32::from_le_bytes(*b))
        .collect()
}
pub(crate) async fn handshake(link: &mut Link, size: usize, window: u32) -> Result<(u32, u32)> {
    if let Some(a) = link.ack.take() {
        link.transmit_next = link.transmit_next.max(a.window.next);
    }
    let mut hello = p::HELLO_MAGIC.to_vec();
    hello.extend((size as u32 * window).to_le_bytes());
    for _ in 0..3 {
        hello.truncate(8);
        hello.extend(link.transmit_next.to_le_bytes());
        link.send(p::HELLO, 0, &hello, false).await?;
        let end = Instant::now() + Duration::from_secs(3);
        while Instant::now() < end {
            let (f, _) = match link
                .next(end.saturating_duration_since(Instant::now()))
                .await
            {
                Ok(v) => v,
                Err(e) if e.downcast_ref::<tokio::time::error::Elapsed>().is_some() => break,
                Err(e) => return Err(e),
            };
            if f.kind != p::HELLO || f.len != 20 {
                continue;
            }
            if u16::from_le_bytes(f.payload[8..10].try_into()?) != p::VERSION
                || words(&f.payload[..4])[0] != link.expected_baud
            {
                return Err("unexpected firmware capabilities".into());
            }
            if f.ack_next > link.transmit_next {
                link.transmit_next = f.ack_next;
                break;
            }
            if f.ack_next < link.transmit_next {
                continue;
            }
            let peer_next = words(&f.payload[16..20])[0];
            if peer_next < link.receive_next {
                return Err("peer receive sequence would rewind".into());
            }
            link.receive_next = peer_next;
            return Ok((link.transmit_next, peer_next));
        }
    }
    Err("HELLO timeout or sequence mismatch".into())
}
fn now(start: Instant) -> u64 {
    start.elapsed().as_millis() as u64
}
fn pattern(id: u32, n: usize) -> Vec<u8> {
    let mut v = vec![0; n];
    ble_bench_payload::fill(id, &mut v);
    v
}
pub async fn run(
    link: &mut Link,
    size: usize,
    window: u32,
    count: u32,
    up: bool,
    inject: bool,
) -> Result<Value> {
    run_traffic(link, size, window, count, up, inject, false).await
}
pub(crate) async fn run_traffic(
    link: &mut Link,
    size: usize,
    window: u32,
    count: u32,
    up: bool,
    inject: bool,
    mixed: bool,
) -> Result<Value> {
    use ble_bench_payload as app;
    let (tx_base, rx_base) = handshake(link, size, window).await?;
    let tx_end = tx_base.checked_add(count).ok_or("sequence exhausted")?;
    link.reset_stats();
    let payload = size - p::OVERHEAD;
    let start = Instant::now();
    let deadline = start + Duration::from_secs(120);
    if !up {
        link.ack = Some(Ack {
            window: p::sliding::Sender::new_at(window, size as u32 * window, tx_base).unwrap(),
        });
        let (mut attempts, mut retries, mut partial, mut cancelled_bytes) =
            (0u32, 0u32, 0u32, 0usize);
        let mut injected = false;
        let mut failure = None;
        loop {
            link.drain()?;
            let s = &mut link.ack.as_mut().unwrap().window;
            if s.base == tx_end {
                break;
            }
            if Instant::now() >= deadline {
                failure = Some("transfer deadline");
                break;
            }
            let (id, retry) = match s.due(now(start)) {
                p::sliding::Due::Exhausted(_) => {
                    failure = Some("retry limit");
                    break;
                }
                p::sliding::Due::Retry(id) => {
                    s.retry(id, now(start));
                    (id, true)
                }
                p::sliding::Due::None => {
                    let size = if mixed {
                        app::random_wire(s.next - tx_base, false, size)
                    } else {
                        size
                    };
                    if s.next < tx_end && s.can_send(size) {
                        (s.start(size, now(start)).unwrap(), false)
                    } else {
                        let base = s.base;
                        let wait = s.wait_ms(now(start)).max(1);
                        link.wait_ack(base, Duration::from_millis(wait)).await?;
                        continue;
                    }
                }
            };
            let payload = if mixed {
                app::random_wire(id - tx_base, false, size) - p::OVERHEAD
            } else {
                payload
            };
            let mut raw = [0; p::MAX_FRAME];
            let n = p::encode(
                p::DATA,
                id,
                link.receive_next,
                &pattern(id, payload),
                &mut raw,
            )
            .unwrap();
            if inject && !injected && id == tx_base + 1 {
                raw[14] ^= 1;
                injected = true;
            }
            let submitted = link.send_raw(&raw[..n], Some(id)).await?;
            if submitted > 0 {
                attempts += 1;
                retries += u32::from(retry);
                if submitted < n {
                    partial += 1;
                    cancelled_bytes += submitted;
                }
            }
            link.ack.as_mut().unwrap().window.submitted(id, now(start));
        }
        // Keep ACK state live while reading the final receiver snapshot.
        let mut stats = None;
        for _ in 0..3 {
            link.send(p::ACK, 0, &[app::QUERY], false).await?;
            let end = Instant::now() + Duration::from_secs(3);
            while Instant::now() < end {
                match link
                    .next(end.saturating_duration_since(Instant::now()))
                    .await
                {
                    Ok((f, at))
                        if f.kind == p::ACK && f.len == 33 && f.payload[0] == app::STATS =>
                    {
                        stats = Some((f, at));
                        break;
                    }
                    Ok(_) => {}
                    Err(e) if e.downcast_ref::<tokio::time::error::Elapsed>().is_some() => break,
                    Err(e) => return Err(e),
                }
            }
            if stats.is_some() {
                break;
            }
        }
        link.transmit_next = link.ack.as_ref().unwrap().window.next;
        link.ack = None;
        let (f, at) = stats.ok_or("no receiver statistics")?;
        let s = words(&f.payload[1..33]);
        let elapsed = at.duration_since(start).as_secs_f64();
        let lost = attempts
            .checked_sub(s[0] + s[2] + s[7])
            .ok_or("invalid loss counters")?;
        Ok(
            json!({"direction":"mac_to_stm32","requested_frames":count,"valid_unique_frames":s[0],"missing_frames":count-s[0],"payload_bytes_received":s[1],"duplicates":s[2],"invalid_payload":s[3],"device_crc_parse_errors":s[4],"device_uart_errors":s[5],"discarded_out_of_order":s[7],"elapsed_seconds":elapsed,"payload_bytes_per_second":s[1] as f64/elapsed,"data_attempts":attempts,"retransmitted_frames":retries,"cancelled_partial_frames":partial,"cancelled_wire_bytes":cancelled_bytes,"final_window_frames":window,"unreceived_or_corrupt_data_attempts":lost,"injected_corruption":injected,"failure":failure,"complete":failure.is_none()&&s[0]==count}),
        )
    } else {
        link.ack = None;
        let mut cmd = vec![app::START];
        cmd.extend(count.to_le_bytes());
        cmd.extend((payload as u16).to_le_bytes());
        cmd.extend((window as u16).to_le_bytes());
        if mixed {
            cmd.push(2);
        }
        link.send(p::DATA, tx_base, &cmd, false).await?;
        link.transmit_next = tx_base + 1;
        let (mut next, mut duplicates, mut gaps, mut invalid) = (rx_base, 0u32, 0u32, 0u32);
        let (mut dropped_data, mut dropped_ack, mut started) = (false, false, false);
        let mut done = None;
        let mut last_data = None;
        let mut received_bytes = 0usize;
        let mut ack_max = 0f64;
        while Instant::now() < deadline {
            let wait = deadline
                .saturating_duration_since(Instant::now())
                .min(Duration::from_millis(400));
            let (f, at) = match link.next(wait).await {
                Ok(v) => v,
                Err(e) if e.downcast_ref::<tokio::time::error::Elapsed>().is_some() => {
                    if started {
                        link.send(p::ACK, 0, &[app::QUERY], false).await?;
                    } else {
                        link.send(p::DATA, tx_base, &cmd, false).await?;
                    }
                    continue;
                }
                Err(e) => return Err(e),
            };
            started = true;
            if f.kind == p::ACK && f.len == 33 && f.payload[0] == app::DONE {
                done = Some(words(&f.payload[1..33]));
                break;
            }
            if f.kind != p::DATA {
                continue;
            }
            if f.sequence < rx_base
                || f.sequence >= rx_base + count
                || f.len
                    != if mixed {
                        app::random_wire(f.sequence - rx_base, true, size) - p::OVERHEAD
                    } else {
                        payload
                    }
                || !app::valid(f.sequence, &f.payload[..f.len])
            {
                invalid += 1;
                continue;
            }
            if inject && !dropped_data && f.sequence == rx_base + 1 {
                dropped_data = true;
                continue;
            }
            if f.sequence < next {
                duplicates += 1;
            } else if f.sequence > next {
                gaps += 1;
            } else {
                next += 1;
                last_data = Some(at);
                received_bytes += f.len - 1;
            }
            if inject && !dropped_ack {
                dropped_ack = true;
                continue;
            }
            link.receive_next = next;
            link.send(p::ACK, 0, &[], false).await?;
            ack_max = ack_max.max(at.elapsed().as_secs_f64() * 1000.);
        }
        link.receive_next = next;
        let next = next - rx_base;
        let elapsed = start.elapsed().as_secs_f64();
        let lost = done
            .as_ref()
            .and_then(|d| d[1].checked_sub(next + duplicates + gaps));
        Ok(
            json!({"direction":"stm32_to_mac","requested_frames":count,"valid_unique_frames":next,"missing_frames":count-next,"payload_bytes_received":received_bytes,"duplicates":duplicates,"discarded_out_of_order":gaps,"invalid_payload":invalid,"host_crc_parse_errors":link.errors(),"elapsed_seconds":elapsed,"last_unique_elapsed_seconds":last_data.map(|t|t.duration_since(start).as_secs_f64()),"payload_bytes_per_second":received_bytes as f64/elapsed,"data_attempts":done.as_ref().map(|d|d[1]),"retransmitted_frames":done.as_ref().map(|d|d[2]),"device_status":done.as_ref().map(|d|d[5]),"acknowledged_frames":done.as_ref().map(|d|d[0]),"unreceived_or_corrupt_data_attempts":lost,"done_received":done.is_some(),"complete":done.as_ref().is_some_and(|d|d[5]==0&&d[0]==count&&next==count),"injected_data_loss":dropped_data,"injected_ack_loss":dropped_ack,"raw_notification_bytes_including_control":link.raw_bytes,"host_timing":{"data_to_ack_max_ms":ack_max,"decoded_frame_queue_peak":link.queue_peak}}),
        )
    }
}

pub async fn duplex(link: &mut Link, size: usize, width: u32, count: u32) -> Result<Value> {
    use ble_bench_payload as app;
    let (tx_base, rx_base) = handshake(link, size, width).await?;
    let end = tx_base.checked_add(count + 1).ok_or("sequence exhausted")?;
    let payload = size - p::OVERHEAD;
    let mut cmd = vec![app::START];
    cmd.extend(count.to_le_bytes());
    cmd.extend((payload as u16).to_le_bytes());
    cmd.extend((width as u16).to_le_bytes());
    link.ack = Some(Ack {
        window: p::sliding::Sender::new_at(width, size as u32 * width, tx_base).unwrap(),
    });
    let start = Instant::now();
    let mut done = None;
    let (mut next, mut duplicates, mut gaps) = (rx_base, 0u32, 0u32);
    let (mut attempts, mut retries, mut piggybacks, mut standalone) = (0u32, 0u32, 0u32, 0u32);
    let mut pending = false;
    let mut failed = false;
    loop {
        while let Some((f, _)) = link.try_next()? {
            if f.kind == p::ACK && f.len == 33 && f.payload[0] == app::DONE {
                done = Some(words(&f.payload[1..33]));
            }
            if f.kind == p::DATA && f.len == payload && app::valid(f.sequence, &f.payload[..f.len])
            {
                if f.sequence < next {
                    duplicates += 1;
                } else if f.sequence == next && next < rx_base + count {
                    next += 1;
                } else {
                    gaps += 1;
                }
                link.receive_next = next;
                pending = true;
            }
        }
        let s = &mut link.ack.as_mut().unwrap().window;
        if s.base == end && done.is_some() {
            break;
        }
        if start.elapsed() > Duration::from_secs(120) {
            failed = true;
            break;
        }
        let candidate = match s.due(now(start)) {
            p::sliding::Due::Exhausted(_) => {
                failed = true;
                break;
            }
            p::sliding::Due::Retry(id) => {
                s.retry(id, now(start));
                Some((id, true))
            }
            p::sliding::Due::None => {
                if s.next < end && s.can_send(size) && (s.next == tx_base || s.base > tx_base) {
                    Some((
                        s.start(
                            if s.next == tx_base {
                                cmd.len() + p::OVERHEAD
                            } else {
                                size
                            },
                            now(start),
                        )
                        .unwrap(),
                        false,
                    ))
                } else {
                    None
                }
            }
        };
        if let Some((id, retry)) = candidate {
            let data = if id == tx_base {
                cmd.clone()
            } else {
                pattern(id, payload)
            };
            let sent = link.send(p::DATA, id, &data, true).await?;
            if sent > 0 && id != tx_base {
                attempts += 1;
                retries += u32::from(retry);
            }
            if sent == data.len() + p::OVERHEAD {
                piggybacks += u32::from(pending);
                pending = false;
            }
            link.ack.as_mut().unwrap().window.submitted(id, now(start));
        } else {
            if pending {
                link.send(p::ACK, 0, &[], false).await?;
                standalone += 1;
                pending = false;
            }
            let wait = link
                .ack
                .as_ref()
                .unwrap()
                .window
                .wait_ms(now(start))
                .clamp(1, 400);
            link.pump(Duration::from_millis(wait)).await?;
            if link.ack.as_ref().unwrap().window.base == end && done.is_none() {
                link.send(p::ACK, 0, &[app::QUERY], false).await?;
            }
        }
    }
    if pending {
        link.send(p::ACK, 0, &[], false).await?;
        standalone += 1;
    }
    let acked = link
        .ack
        .as_ref()
        .unwrap()
        .window
        .base
        .saturating_sub(tx_base + 1);
    link.transmit_next = link.ack.take().unwrap().window.next;
    let elapsed = start.elapsed().as_secs_f64();
    Ok(
        json!({"mode":"simultaneous_duplex","wire_frame_bytes":size,"window":width,"requested_frames_each_direction":count,"elapsed_seconds":elapsed,"down_acknowledged":acked,"up_unique":next-rx_base,"up_duplicates":duplicates,"up_out_of_order":gaps,"down_attempts":attempts,"down_retries":retries,"host_piggyback_confirmations":piggybacks,"host_standalone_acks":standalone,"device_piggyback_confirmations":done.as_ref().map(|d|d[3]),"device_report":done,"complete":!failed && acked==count && next-rx_base==count && done.as_ref().is_some_and(|d|d[5]==0 && d[0]==count)}),
    )
}
