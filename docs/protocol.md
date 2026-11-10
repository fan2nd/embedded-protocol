# PCMON1 资源监视协议

协议只定义字节流上的消息，不依赖操作系统、开发板、UI 框架或 USB 标识。
当前上位机使用串口/USB CDC；协议编码与解码由 `crates/monitor-protocol` 提供。

串口 BLE 透传使用独立的 [可靠传输约定](ble-protocol.md)，新版采用固定滑动窗口、逐包累计确认、重传及半双工帧间调度；G431/Mac 示例当前采用 v6，接入状态见该文档；下文 PCMON1 的无应答发送规则不适用于 BLE 链路。

## 分帧与握手

消息为 ASCII，以 `LF`（`0x0A`）结束。接收端允许帧尾带 `CR`。
字节流可能把一条消息拆成多个传输包，也可能将多条消息合并到一个包，必须按换行分帧。

```text
上位机 → HELLO\n
接收端 → PCMON1 READY\n
```

上位机当前等待回复最多 2 秒；握手失败不发送指标，稍后重试。
`HELLO` 不清空设备状态，也不触发复位或烧录。

## 指标帧

```text
PCMON1,sequence,cpu,memory,rx_kib,tx_kib,uptime_s\n
```

| 字段 | 类型 / 范围 | 含义 |
|---|---|---|
| `sequence` | `u32` | 会话内递增序号，允许回绕及重连后重新开始 |
| `cpu` | 0–1000 | 百分比的十分之一，427 表示 42.7% |
| `memory` | 0–1000 | 百分比的十分之一 |
| `rx_kib` | `u32` | 下行速率，整数 KiB/s，1 KiB = 1024 bytes |
| `tx_kib` | `u32` | 上行速率，整数 KiB/s |
| `uptime_s` | `u32` | 电脑开机时间累计秒数 |

例如：

```text
PCMON1,7,427,632,1280,96,14520
```

表示 CPU 42.7%、内存 63.2%、下行 1.25 MiB/s、上行 96 KiB/s、运行 4 小时 2 分钟。
只允许十进制非负整数，不允许空字段、额外字段、符号、浮点或超过范围的值。

## 接收约定

- `Decoder::feed` 逐字节解析；完整有效帧返回 `Message`，尚未完成或无效帧返回 `None`。
- 一行最多缓冲 128 字节（不含 LF）；超长后丢弃到下一个 LF，再恢复接收。
- 协议不要求每帧应答。接收端可仅保留最新快照，避免显示排队的过期数据。
- 不按序号大小拒绝数据，因为它可以回绕或在新连接中重新开始。
- 采样频率和离线提示属于应用策略：当前上位机默认约 0.5 秒一帧，RP2350 示例超过 3 秒无有效帧显示离线。
- 该版本没有平台名、温度、GPU、时区、加密或校验和字段。新增不兼容字段应使用新协议版本，不改变 PCMON1 的六个整数布局。

## Rust 使用

```rust
use monitor_protocol::{Decoder, Message};

let mut decoder = Decoder::new();
for &byte in b"PCMON1,7,427,632,1280,96,14520\n" {
    if let Some(Message::Metrics(metrics)) = decoder.feed(byte) {
        // 更新应用自己的状态，不在协议层访问硬件或绘图。
        assert_eq!(metrics.cpu, 427);
    }
}
```

编码使用 `Metrics::write_line(&mut impl core::fmt::Write)`，可写入标准库 `String` 或调用方提供的固定容量缓冲。CPU/内存越界或缓冲不足会返回格式错误；格式写入失败时调用方应丢弃整帧。
