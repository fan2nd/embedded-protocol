# ble-bench-protocol

`no_std`、无依赖、无堆分配。帧、固定窗口与会话防重放状态独立于 HAL 和 AT 驱动。这是历史 v3 诊断协议，仅供旧 Python 工具和结果对照；当前实现见 [ble-link-protocol](../ble-link-protocol)。

```text
A5 5A | type:u8 | session:u128 | id:u32 | length:u16 | payload | CRC32C:u32 | 5A A5
```

开销 31 字节，最大载荷 1024 字节，整数小端。数据 ID 独立递增，控制/应答不占数据序号。

| 类型 | 值 | 载荷 |
|---|---:|---|
| HELLO（控制） | 0x11 | RLB3 四字节 + 客户端随机数 u128 + 窗口字节上限 u32；session/id=0 |
| REPORT（控制） | 0x12 | 空：下行统计；TX 两字节：查询缓存上行结束结果 |
| TX_START（控制） | 0x13 | 总包数 u32、载荷大小 u16、固定窗口包数 u16 |
| POLL（控制） | 0x14 | 当前窗口末尾数据 ID 加一 u32；id 为独立请求号 |
| CONFIRM（控制） | 0x15 | 客户端随机数 u128；session 为设备新挑战，id=0 |
| DATA（数据） | 0x21 | Mac→MCU 的测试数据 |
| TX_DATA（数据） | 0x22 | MCU→Mac 的测试数据 |
| READY（应答） | 0x31 | 波特率 u32、最大载荷 u16、最大帧数 u16、版本 u16=3、最大窗口包数 u16=8、窗口字节限制 u32 |
| STATS（应答） | 0x32 | 八个 u32：按序有效包、有效载荷字节、重复包、内容错误、解码错误、UART 错误、首末有效帧间隔微秒、丢弃的乱序包 |
| TX_DONE（应答） | 0x33 | 八个 u32：累计确认包、数据发送次数、重传包、POLL 发送次数、传输耗时微秒、状态、POLL 轮数、ACK 超时数 |
| ACK（应答） | 0x34 | 下一期待数据 ID u32；id 回显 POLL 请求号，不增加数据 ID |
| CHALLENGE（应答） | 0x35 | 回显客户端随机数 u128；session 为设备硬件 RNG 生成的新挑战 |

TX_DONE 状态 0 成功、1 UART 发送失败、2 重试耗尽。数据最多 8192 包，载荷第 i 字节为 `(id×31 + i×17) mod 256`。任意业务数据需由调用方保存不可变的未确认内容。

`window::Window` 固定窗口，`retry_timeout_ms` 依次 100/200/400 ms、400 封顶，最多 8 次发送。部分确认或超时不会缩窗。`next_pending` 在每次提交前依据最新 ACK 跳过已确认包，`receive_acks` 处理完整接收块中的所有 ACK，校验会话、轮次和实际发送范围；重复或回退 ACK 不触发重传。重复数据不交付，旧会话不会恢复。`session::Session` 由平台注入安全随机挑战；它防止记录包重放，不提供带密钥的消息认证。

验证：`cargo test --locked -p ble-bench-protocol`；`.venv/bin/python apps/ble-bench-python/ble_bench.py --self-test`；硬件抗重放验证 `.venv/bin/python apps/ble-bench-python/ble_bench.py --replay-test`。
