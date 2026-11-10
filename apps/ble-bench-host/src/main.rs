mod bench;
mod mixed;
mod transport;
use serde_json::{json, Value};
use std::path::PathBuf;
use transport::{Link, Result};
#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let value = |name: &str, default: &str| {
        args.iter()
            .position(|x| x == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
            .unwrap_or_else(|| default.to_string())
    };
    let matrix = args.contains(&"--matrix".to_string());
    let inject = args.contains(&"--inject-loss".to_string());
    let output = PathBuf::from(value("--output", "output/stm32g431/rust-matrix"));
    std::fs::create_dir_all(&output)?;
    let sizes: Vec<usize> = value(
        "--matrix-frames",
        if matrix {
            "256,512,1024,182,128,64"
        } else {
            "128"
        },
    )
    .split(',')
    .map(str::parse)
    .collect::<std::result::Result<_, _>>()?;
    let windows: Vec<u32> = value("--matrix-windows", if matrix { "1,2,4,8" } else { "2" })
        .split(',')
        .map(str::parse)
        .collect::<std::result::Result<_, _>>()?;
    let target: usize = value("--matrix-bytes", if matrix { "16384" } else { "1552" }).parse()?;
    for &size in &sizes {
        for &window in &windows {
            if !(64..=ble_link_protocol::MAX_FRAME).contains(&size)
                || !(1..=8).contains(&window)
                || size * window as usize > 8192
                || target == 0
                || target.div_ceil(size - (ble_link_protocol::OVERHEAD + 1)) > 8192
            {
                return Err("invalid matrix parameters".into());
            }
        }
    }
    let expected_baud: u32 = value("--baud", "115200").parse()?;
    let mut link = Link::connect(&value("--name", "G431-BLE"), expected_baud).await?;
    if args.contains(&"--mixed-duplex".to_string()) {
        let target = value("--matrix-bytes", "16384").parse()?;
        let windows = value("--matrix-windows", "1,2,4,8");
        let caps: Vec<usize> = value("--mixed-max-frames", "1024")
            .split(',')
            .map(str::parse)
            .collect::<std::result::Result<_, _>>()?;
        let mut result = Ok(());
        for cap in &caps {
            let path = if caps.len() == 1 {
                output.clone()
            } else {
                output.join(format!("max{cap}"))
            };
            result = mixed::matrix(&mut link, &path, target, &windows, *cap).await;
            if result.is_err() {
                break;
            }
        }
        link.close().await;
        return result;
    }
    if args.contains(&"--duplex".to_string()) {
        let result = bench::duplex(
            &mut link,
            sizes[0],
            windows[0],
            target.div_ceil(sizes[0] - (ble_link_protocol::OVERHEAD + 1)) as u32,
        )
        .await?;
        println!("{result}");
        std::fs::write(
            output.join("duplex.json"),
            serde_json::to_string_pretty(&result)?,
        )?;
        link.close().await;
        return Ok(());
    }
    let mut rows = Vec::new();
    for &size in &sizes {
        for &window in &windows {
            let count = target.div_ceil(size - (ble_link_protocol::OVERHEAD + 1)) as u32;
            println!("MATRIX_BEGIN frame={size} window={window} count={count}");
            let mut row = json!({"uart_baud":expected_baud,"wire_frame_bytes":size,"window":window,"count":count,"rto_ms":[ble_link_protocol::sliding::retry_timeout_ms(1,size as u16),ble_link_protocol::sliding::retry_timeout_ms(2,size as u16),ble_link_protocol::sliding::retry_timeout_ms(3,size as u16)],"host":"Rust","implementation":"bluest 0.6.9 notify with native write backpressure","receive_mode":"notify A002","write_characteristic":"A003","effective_write_chunk":link.chunk,"tests":[]});
            for up in [false, true] {
                match bench::run(&mut link, size, window, count, up, inject).await {
                    Ok(mut test) => {
                        add_rates(&mut test);
                        println!("{}", test);
                        row["tests"].as_array_mut().unwrap().push(test)
                    }
                    Err(e) => {
                        row["error"] = json!(e.to_string());
                        break;
                    }
                }
            }
            row["connection_receive_stats"] = link.receive_stats();
            row["gatt"] = link.gatt.clone();
            row["complete"] = json!(
                row["tests"].as_array().unwrap().len() == 2
                    && row["tests"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .all(|t| t["complete"] == true)
            );
            std::fs::write(
                output.join(format!("frame{size}-window{window}.json")),
                serde_json::to_string_pretty(&row)?,
            )?;
            let transport_failed = row.get("error").is_some();
            rows.push(row);
            std::fs::write(
                output.join("summary.json"),
                serde_json::to_string_pretty(&rows)?,
            )?;
            std::fs::write(output.join("matrix.md"), table(&rows))?;
            println!("MATRIX_END frame={size} window={window}");
            if transport_failed {
                link.close().await;
                return Err("transport/handshake failed; evidence saved".into());
            }
        }
    }
    link.close().await;
    Ok(())
}
fn table(rows: &[Value]) -> String {
    let mut sizes: Vec<_> = rows
        .iter()
        .filter_map(|r| r["wire_frame_bytes"].as_u64())
        .collect();
    sizes.sort();
    sizes.dedup();
    let mut s = String::from(
        "每格：下行 kB/s / 上行 kB/s / 下行丢包率 / 上行丢包率 / 下行重传率 / 上行重传率；† 未完成。\n\n|窗口/整包字节|",
    );
    for size in &sizes {
        s += &format!("{size}|")
    }
    s += "\n|---:|";
    for _ in &sizes {
        s += "---|"
    }
    s += '\n'.to_string().as_str();
    for w in [1, 2, 4, 8] {
        s += &format!("|{w}|");
        for size in &sizes {
            if let Some(r) = rows
                .iter()
                .find(|r| r["wire_frame_bytes"] == *size && r["window"] == w)
            {
                let t = r["tests"].as_array().unwrap();
                let mut values = Vec::new();
                for i in 0..2 {
                    values.push(
                        t.get(i)
                            .and_then(|t| t["payload_bytes_per_second"].as_f64())
                            .map(|v| format!("{:.2}", v / 1000.))
                            .unwrap_or("—".into()),
                    );
                }
                for i in 0..2 {
                    values.push(
                        t.get(i)
                            .and_then(|t| {
                                Some(
                                    100. * t["unreceived_or_corrupt_data_attempts"].as_f64()?
                                        / t["data_attempts"].as_f64()?,
                                )
                            })
                            .map(|v| format!("{v:.2}%"))
                            .unwrap_or("—".into()),
                    );
                }
                for i in 0..2 {
                    values.push(
                        t.get(i)
                            .and_then(|v| v["retransmission_rate_pct"].as_f64())
                            .map(|v| format!("{v:.2}%"))
                            .unwrap_or("—".into()),
                    );
                }
                s += &values.join("/");
                if r["complete"] != true {
                    s += "†"
                }
                s += "|"
            } else {
                s += "—|"
            }
        }
        s += "\n"
    }
    s
}

fn add_rates(test: &mut Value) {
    let attempts = test["data_attempts"].as_u64();
    let retries = test["retransmitted_frames"].as_u64();
    test["retransmission_rate_pct"] = json!(attempts
        .zip(retries)
        .filter(|(a, r)| *a > 0 && r <= a)
        .map(|(a, r)| 100.0 * r as f64 / a as f64));
    test["first_transmission_attempts"] =
        json!(attempts.zip(retries).and_then(|(a, r)| a.checked_sub(r)));
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retransmission_ratio_counts_attempts_not_planned_or_received_frames() {
        let mut t = json!({"data_attempts":12,"retransmitted_frames":4,"requested_frames":100,"valid_unique_frames":7});
        add_rates(&mut t);
        assert_eq!(t["first_transmission_attempts"], 8);
        assert!((t["retransmission_rate_pct"].as_f64().unwrap() - 100.0 / 3.0).abs() < 1e-9);
        let mut t = json!({"data_attempts":0,"retransmitted_frames":0});
        add_rates(&mut t);
        assert!(t["retransmission_rate_pct"].is_null());
    }
}
