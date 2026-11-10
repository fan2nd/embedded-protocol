# BLE 链路协议

`no_std`、无堆分配、有界缓冲。负责 v6 HELLO/DATA/ACK 编解码、CRC32C、增量重同步和固定滑动窗口；不依赖 HAL、AT 驱动或 BLE 主机 API。

帧格式、确认及超时规则见 [协议定义](../../docs/ble-protocol.md)。测速载荷独立放在 `ble-bench-payload`，历史 v3 实现在 `ble-bench-protocol`。

验证：`cargo test --locked -p ble-link-protocol`。
