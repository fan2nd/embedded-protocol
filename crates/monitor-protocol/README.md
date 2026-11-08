# monitor-protocol

PCMON1 的独立 Rust 实现：`no_std`、无依赖、无堆分配。

- `Metrics`：六个资源状态字段。
- `Metrics::write_line`：向调用方提供的 `core::fmt::Write` 缓冲编码一行。
- `Decoder::feed`：处理拆包、粘包、非法/超长帧恢复。
- `HELLO` / `READY`：完整握手消息（含换行）。

该 crate 不包含操作系统采集、串口、USB、MCU HAL 或图形库。
完整规范见 [docs/protocol.md](../../docs/protocol.md)。
