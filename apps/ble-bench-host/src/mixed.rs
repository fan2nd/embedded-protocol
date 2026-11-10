//! Mixed-size simultaneous traffic and reporting belong to the diagnostic host.
use crate::{
    bench::handshake,
    transport::{Ack, Link, Result},
};
use ble_bench_payload as app;
use ble_link_protocol as p;
use serde_json::{json, Value};
use std::{
    path::Path,
    time::{Duration, Instant},
};
fn words(f: &p::Frame) -> Vec<u32> {
    f.payload[1..33]
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| u32::from_le_bytes(*b))
        .collect()
}
#[allow(clippy::too_many_arguments)] // Explicit receiver and sender counters for the report.
fn metric(
    direction: &str,
    count: u32,
    received: u32,
    bytes: u32,
    duplicates: u32,
    gaps: u32,
    attempts: Option<u32>,
    retries: Option<u32>,
    elapsed: f64,
    complete: bool,
) -> Result<Value> {
    let lost = attempts
        .map(|a| {
            a.checked_sub(received + duplicates + gaps)
                .ok_or("inconsistent receive counters")
        })
        .transpose()?;
    if retries.zip(attempts).is_some_and(|(r, a)| r > a) {
        return Err("inconsistent retry counters".into());
    }
    Ok(
        json!({"direction":direction,"requested_frames":count,"valid_unique_frames":received,"missing_frames":count-received,
        "payload_bytes_received":bytes,"duplicates":duplicates,"discarded_out_of_order":gaps,"data_attempts":attempts,
        "retransmitted_frames":retries,"first_transmission_attempts":attempts.zip(retries).map(|(a,r)|a-r),
        "unreceived_or_corrupt_data_attempts":lost,"loss_rate_pct":lost.zip(attempts).filter(|(_,a)|*a>0).map(|(l,a)|100.*l as f64/a as f64),
        "retransmission_rate_pct":retries.zip(attempts).filter(|(_,a)|*a>0).map(|(r,a)|100.*r as f64/a as f64),
        "elapsed_seconds":elapsed,"payload_bytes_per_second":bytes as f64/elapsed,"complete":complete}),
    )
}
async fn run(link: &mut Link, down: u32, up: u32, count: u32, cap: usize) -> Result<Value> {
    let (tx_base, rx_base) = handshake(link, cap, up).await?;
    let end = tx_base.checked_add(count + 1).ok_or("sequence exhausted")?;
    let rx_end = rx_base.checked_add(count).ok_or("sequence exhausted")?;
    link.reset_stats();
    let mut cmd = vec![app::START];
    cmd.extend(count.to_le_bytes());
    cmd.extend((cap as u16 - p::OVERHEAD as u16).to_le_bytes());
    cmd.extend((up as u16).to_le_bytes());
    cmd.push(2); // Application START extension: seeded random lengths within cap.
    link.ack = Some(Ack {
        window: p::sliding::Sender::new_at(down, cap as u32 * down, tx_base).unwrap(),
    });
    let start = Instant::now();
    let mut done: Option<Vec<u32>> = None;
    let (mut next, mut duplicates, mut gaps, mut invalid, mut bytes) =
        (rx_base, 0u32, 0u32, 0u32, 0u32);
    let (mut attempts, mut retries, mut partial, mut cancelled_bytes) = (0u32, 0u32, 0u32, 0usize);
    let (mut piggybacks, mut standalone) = (0u32, 0u32);
    let mut pending = false;
    let mut failure: Option<&str> = None;
    let mut down_ended = None;
    let mut up_first = None;
    let mut up_last = None;
    let mut down_first = None;
    let mut down_last = None;
    let mut last_query = Instant::now();
    let mut ack_pending_at: Option<Instant> = None;
    let mut ack_delay_max = 0f64;
    loop {
        while let Some((f, at)) = link.try_next()? {
            if f.kind == p::ACK && f.len == 33 && f.payload[0] == app::DONE {
                done = Some(words(&f));
            }
            if f.kind != p::DATA {
                continue;
            }
            if f.sequence < rx_base
                || f.sequence >= rx_end
                || f.len + p::OVERHEAD != app::random_wire(f.sequence - rx_base, true, cap)
                || !app::valid(f.sequence, &f.payload[..f.len])
            {
                invalid += 1;
                continue;
            }
            if f.sequence < next {
                duplicates += 1;
            } else if f.sequence > next {
                gaps += 1;
            } else {
                next += 1;
                bytes += (f.len - 1) as u32;
                up_first.get_or_insert(at.duration_since(start).as_secs_f64());
                up_last = Some(at.duration_since(start).as_secs_f64());
            }
            link.receive_next = next;
            pending = true;
            ack_pending_at.get_or_insert(at);
        }
        let now = start.elapsed().as_millis() as u64;
        let s = &mut link.ack.as_mut().unwrap().window;
        if s.base == end || failure.is_some() {
            down_ended.get_or_insert(start.elapsed().as_secs_f64());
        }
        if down_ended.is_some() && done.is_some() {
            break;
        }
        if start.elapsed() >= Duration::from_secs(120) {
            failure = Some("transfer deadline");
            break;
        }
        let candidate = if failure.is_some() {
            None
        } else {
            match s.due(now) {
                p::sliding::Due::Exhausted(_) => {
                    failure = Some("down retry limit");
                    None
                }
                p::sliding::Due::Retry(id) => {
                    s.retry(id, now);
                    Some((id, true))
                }
                p::sliding::Due::None => {
                    let size = if s.next == tx_base {
                        cmd.len() + p::OVERHEAD
                    } else {
                        app::random_wire(s.next - tx_base - 1, false, cap)
                    };
                    if s.next < end && s.can_send(size) && (s.next == tx_base || s.base > tx_base) {
                        Some((s.start(size, now).unwrap(), false))
                    } else {
                        None
                    }
                }
            }
        };
        if let Some((id, retry)) = candidate {
            let data = if id == tx_base {
                cmd.clone()
            } else {
                let mut v = vec![0; app::random_wire(id - tx_base - 1, false, cap) - p::OVERHEAD];
                app::fill(id, &mut v);
                v
            };
            let sent = link.send(p::DATA, id, &data, true).await?;
            if sent > 0 && id != tx_base {
                attempts += 1;
                retries += u32::from(retry);
                let at = start.elapsed().as_secs_f64();
                down_first.get_or_insert(at);
                down_last = Some(at);
                if sent < data.len() + p::OVERHEAD {
                    partial += 1;
                    cancelled_bytes += sent;
                }
            }
            if sent == data.len() + p::OVERHEAD {
                piggybacks += u32::from(pending);
                pending = false;
                if let Some(at) = ack_pending_at.take() {
                    ack_delay_max = ack_delay_max.max(at.elapsed().as_secs_f64() * 1000.);
                }
            }
            link.ack
                .as_mut()
                .unwrap()
                .window
                .submitted(id, start.elapsed().as_millis() as u64);
        } else {
            if pending {
                link.send(p::ACK, 0, &[], false).await?;
                standalone += 1;
                pending = false;
                if let Some(at) = ack_pending_at.take() {
                    ack_delay_max = ack_delay_max.max(at.elapsed().as_secs_f64() * 1000.);
                }
            }
            if down_ended.is_some()
                && done.is_none()
                && last_query.elapsed() >= Duration::from_millis(400)
            {
                link.send(p::ACK, 0, &[app::QUERY], false).await?;
                last_query = Instant::now();
            }
            let wait = if failure.is_some() {
                400
            } else {
                link.ack.as_ref().unwrap().window.wait_ms(now).clamp(1, 400)
            };
            link.pump(Duration::from_millis(wait)).await?;
        }
    }
    if pending {
        link.send(p::ACK, 0, &[], false).await?;
        standalone += 1;
    }
    // Explicit receiver snapshot, even when a completed upstream report is cached.
    // QUERY follows all submitted downstream bytes on the same write stream.
    let mut stats = None;
    for _ in 0..3 {
        link.send(p::ACK, 0, &[app::QUERY, app::STATS], false)
            .await?;
        let until = Instant::now() + Duration::from_secs(3);
        while Instant::now() < until {
            match link
                .next(until.saturating_duration_since(Instant::now()))
                .await
            {
                Ok((f, _)) if f.kind == p::ACK && f.len == 33 && f.payload[0] == app::STATS => {
                    stats = Some(words(&f));
                    break;
                }
                Ok((f, _)) if f.kind == p::ACK && f.len == 33 && f.payload[0] == app::DONE => {
                    done = Some(words(&f));
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
    let elapsed = start.elapsed().as_secs_f64();
    let sender = link.ack.take().unwrap().window;
    link.transmit_next = sender.next;
    let d = stats.ok_or("mixed duplex receiver snapshot missing")?;
    let expected_down: u32 = (0..d[0])
        .map(|i| (app::random_wire(i, false, cap) - app::TEST_OVERHEAD) as u32)
        .sum();
    if d[0] > count || d[1] != expected_down {
        return Err("mixed downstream byte count mismatch".into());
    }
    let mut down_result = metric(
        "mac_to_stm32",
        count,
        d[0],
        d[1],
        d[2],
        d[7],
        Some(attempts),
        Some(retries),
        elapsed,
        failure.is_none() && d[0] == count && sender.base == end,
    )?;
    down_result["device_crc_parse_errors"] = json!(d[4]);
    down_result["device_uart_errors"] = json!(d[5]);
    down_result["cancelled_partial_frames"] = json!(partial);
    down_result["cancelled_wire_bytes"] = json!(cancelled_bytes);
    down_result["failure"] = json!(failure);
    down_result["invalid_payload"] = json!(d[3]);
    let mut up_result = metric(
        "stm32_to_mac",
        count,
        next - rx_base,
        bytes,
        duplicates,
        gaps,
        done.as_ref().map(|d| d[1]),
        done.as_ref().map(|d| d[2]),
        elapsed,
        done.as_ref().is_some_and(|d| d[5] == 0 && d[0] == count) && next == rx_end,
    )?;
    up_result["host_crc_parse_errors"] = json!(link.errors());
    up_result["invalid_payload"] = json!(invalid);
    up_result["device_status"] = json!(done.as_ref().map(|d| d[5]));
    let complete = down_result["complete"] == true && up_result["complete"] == true;
    Ok(
        json!({"mode":"simultaneous_mixed_duplex","uart_baud":link.expected_baud,"down_window":down,"up_window":up,
        "down_byte_budget":cap as u32*down,"up_byte_budget":cap as u32*up,"count_each_direction":count,
        "rto_rule":"ceil(full_wire_bytes/128) * 100/200/400 ms; max 8 attempts; fixed windows",
        "write_characteristic":"A003","receive_mode":"notify A002","effective_write_chunk":link.chunk,
        "tests":[down_result,up_result],"complete":complete,"elapsed_seconds":elapsed,
        "timing":{"down_first_submission_s":down_first,"down_last_submission_s":down_last,"down_terminal_s":down_ended,
        "up_first_unique_s":up_first,"up_last_unique_s":up_last,"host_data_to_ack_submit_max_ms":ack_delay_max},
        "host_piggyback_confirmations":piggybacks,"host_standalone_acks":standalone,
        "device_piggyback_confirmations":done.as_ref().map(|d|d[3]),"device_report":done,
        "raw_notification_bytes_including_control":link.raw_bytes,"decoded_frame_queue_peak":link.queue_peak}),
    )
}
pub async fn matrix(
    link: &mut Link,
    output: &Path,
    target: usize,
    windows: &str,
    cap: usize,
) -> Result<()> {
    if ![128, 182, 256, 512, 1024].contains(&cap) {
        return Err("invalid mixed frame cap".into());
    }
    std::fs::create_dir_all(output)?;
    let windows: Vec<u32> = windows
        .split(',')
        .map(str::parse)
        .collect::<std::result::Result<_, _>>()?;
    if windows.is_empty() || windows.iter().any(|w| ![1, 2, 4, 8].contains(w)) {
        return Err("invalid fixed windows".into());
    }
    let count = app::random_count(target, cap).ok_or("invalid mixed byte target")?;
    let mut rows = Vec::new();
    for mode in ["down_only", "up_only", "simultaneous_mixed_duplex"] {
        for &w in &windows {
            println!("MIXED_BEGIN mode={mode} window={w} count={count}");
            let result = if mode == "simultaneous_mixed_duplex" {
                run(link, w, w, count, cap).await
            } else {
                crate::bench::run_traffic(link, cap, w, count, mode == "up_only", false, true)
                    .await
                    .map(|mut t| {
                        crate::add_rates(&mut t);
                        t["loss_rate_pct"] = json!(t["unreceived_or_corrupt_data_attempts"]
                            .as_f64()
                            .zip(t["data_attempts"].as_f64())
                            .filter(|(_, a)| *a > 0.)
                            .map(|(l, a)| 100. * l / a));
                        let complete = t["complete"].clone();
                        let tests = if mode == "down_only" {
                            json!([t, null])
                        } else {
                            json!([null, t])
                        };
                        json!({"mode":mode,"tests":tests,"complete":complete})
                    })
            };
            let mut row = match result {
                Ok(r) => r,
                Err(e) => json!({"complete":false,"error":e.to_string()}),
            };
            row["mode"] = json!(mode);
            row["window"] = json!(w);
            row["uart_baud"] = json!(link.expected_baud);
            row["count_each_active_direction"] = json!(count);
            row["maximum_wire_frame_bytes"] = json!(cap);
            row["minimum_wire_frame_bytes"] = json!(app::RANDOM_MIN_WIRE);
            row["length_seeds"] = json!(app::RANDOM_SEEDS);
            row["traffic_pattern"] =
                json!("seeded pseudo-random integer wire lengths; unchanged on retry");
            row["down_wire_lengths"] = json!((0..count)
                .map(|i| app::random_wire(i, false, cap))
                .collect::<Vec<_>>());
            row["up_wire_lengths"] = json!((0..count)
                .map(|i| app::random_wire(i, true, cap))
                .collect::<Vec<_>>());
            row["rto_rule"] =
                json!("ceil(full_wire_bytes/128) * 100/200/400 ms; max 8 attempts; fixed windows");
            row["write_characteristic"] = json!("A003");
            row["receive_mode"] = json!("notify A002");
            row["effective_write_chunk"] = json!(link.chunk);
            row["connection_receive_stats"] = link.receive_stats();
            row["gatt"] = link.gatt.clone();
            std::fs::write(
                output.join(format!("{mode}-window{w}.json")),
                serde_json::to_string_pretty(&row)?,
            )?;
            println!(
                "{}",
                json!({"mode":mode,"window":w,"complete":row["complete"],"tests":row["tests"],"error":row["error"]})
            );
            let failed = row.get("error").is_some();
            rows.push(row);
            std::fs::write(
                output.join("summary.json"),
                serde_json::to_string_pretty(&rows)?,
            )?;
            std::fs::write(output.join("matrix.md"), table(&rows))?;
            if failed {
                return Err("mixed transport/statistics failure; evidence saved".into());
            }
            println!("MIXED_END mode={mode} window={w}");
        }
    }
    Ok(())
}
fn table(rows: &[Value]) -> String {
    let mut s=String::from("每格：下/上 kB/s · 下/上丢包率% · 下/上重传率%；† 至少一方向未完成，— 该方向不发送。双向速率使用同一组总耗时，包含恢复及统计收尾。\n\n|窗口|单向下行|单向上行|同时双向|\n|---:|---|---|---|\n");
    for w in [1, 2, 4, 8] {
        s += &format!("|{w}|");
        for mode in ["down_only", "up_only", "simultaneous_mixed_duplex"] {
            if let Some(r) = rows.iter().find(|r| r["window"] == w && r["mode"] == mode) {
                let mut pairs = Vec::new();
                for key in [
                    "payload_bytes_per_second",
                    "loss_rate_pct",
                    "retransmission_rate_pct",
                ] {
                    pairs.push(
                        (0..2)
                            .map(|i| {
                                r["tests"][i][key]
                                    .as_f64()
                                    .map(|v| {
                                        format!(
                                            "{:.2}",
                                            if key == "payload_bytes_per_second" {
                                                v / 1000.
                                            } else {
                                                v
                                            }
                                        )
                                    })
                                    .unwrap_or("—".into())
                            })
                            .collect::<Vec<_>>()
                            .join("/"),
                    );
                }
                s += &pairs.join(" · ");
                if r["complete"] != true {
                    s += "†"
                }
                s += "|";
            } else {
                s += "—|"
            }
        }
        s += "\n";
    }
    s
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn loss_retry_and_throughput_use_receiver_counts() {
        let t = metric("test", 10, 7, 1000, 2, 1, Some(12), Some(4), 2., false).unwrap();
        assert_eq!(t["missing_frames"], 3);
        assert_eq!(t["unreceived_or_corrupt_data_attempts"], 2);
        assert_eq!(t["payload_bytes_per_second"], 500.);
        assert_eq!(t["first_transmission_attempts"], 8);
        assert!(metric("test", 10, 7, 1000, 2, 1, Some(9), Some(0), 2., false).is_err());
    }
}
