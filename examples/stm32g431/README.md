# STM32G431 doBT-01X 透传与吞吐率验证

用于验证通用驱动的板级示例，不依赖 PCMON 应用或 RP2350。

| 组件 | 职责 |
|---|---|
| `crates/dobt01-driver` | 可复用 AT 命令、响应匹配与配置/透传状态 |
| `crates/ble-link-protocol` | 通用三类型链路帧、CRC、增量解析、固定滑动窗口 |
| `crates/ble-bench-payload` | 测速应用载荷与计数编码 |
| `examples/stm32g431/firmware` | G431 时钟、引脚、UART、中断、测试任务装配 |
| `apps/ble-bench-host` | Rust Mac BLE Central、双向校验、JSON/矩阵报告 |
| `apps/ble-bench-python/ble_bench.py` | 保留的 Python 对照及历史诊断工具 |

## 接线与启动

STM32G431CBU6：PA9/USART1_TX → 模块 RX，PA10/USART1_RX ← 模块 TX，双方共地、3.3V 逻辑电平。ST-Link 通过 SWD 烧录并读 RTT，虚拟串口不是本示例的数据通道。未连接 RTS/CTS，不声明硬件流控。

固件使用 HSI 经 PLL 得到 64 MHz，USART1 8N1、8192 字节接收环形缓冲、2048 字节发送缓冲。接收尽早唤醒解析任务；中断计数硬件 PE/FE/NE/ORE 事件。环形缓冲内部溢出不由该 HAL 单独计数，端到端序号、CRC 和确定性载荷校验仍会暴露数据损失，不能仅依据 UART 错误计数为零判定无丢包。

启动按顺序探测 115200、9600、230400、256000、460800、512000、1000000、57600、38400、19200、31250、2400、4800；首次匹配有效查询结果即停止，不盲改当前波特率。支持实测 `QT+05` 编号格式。按支持情况查询角色，主机角色才尝试改成从机；旧固件的 `ER+2` 会结束查询，不捏造主从切换成功。

查询名称，不是 `G431-BLE` 时写入新名称；等待至少 1 秒，发送 `AT+CZ`，重启后回读波特率和名称。失败则停在错误状态。成功后不再轮询 AT，等待 Mac 的完整校验二进制 HELLO，回复 HELLO 后确认透传。

当前模块固件 `1.9-20231008`：串口按用户指定改回 115200 baud，重启后回读 `QT+05`。GATT 服务 A000，双向特征 A001，另有 A002/A003；与 AT 手册的 FFF0 默认值不同。Rust 上位机固定验证 A003 的 write-without-response 和 A002 的 notify 属性，不修改模块 UUID。

当前写入使用 A003（`0000a003-0000-1000-8000-00805f9b34fb`），接收使用 A002（`0000a002-0000-1000-8000-00805f9b34fb`）的 notify。A002 Read 曾实测只返回设备名，用户已明确改为 notify 接收。A001 是历史实验使用的 write-without-response+notify 通道。

## 构建与运行

在仓库根目录：

```sh
rustup target add thumbv7em-none-eabihf
DEFMT_LOG=info cargo build --release --locked -p stm32g431-ble-bench --target thumbv7em-none-eabihf
probe-rs run --chip STM32G431CBUx --protocol swd --speed 1000 --verify --rtt-channel-mode no-block-skip target/thumbv7em-none-eabihf/release/stm32g431-ble-bench
```

`DEFMT_LOG=info` 是编译时设置；日志走 RTT，不混入模块 UART。烧录会替换目标固件。本次用户明确要求不备份，工具不会自动创建 Flash 备份或恢复出厂设置。

另一个终端运行 Rust Mac 对端（需要系统蓝牙权限）：

```sh
cargo build --locked --release -p ble-bench-host
target/release/ble-bench-host --inject-loss --output output/stm32g431/v6-injected-new
target/release/ble-bench-host --matrix --output output/stm32g431/v6-matrix-new
```

Rust 默认连接 G431-BLE，使用 A003 写入和 A002 notify 接收。实现、参数和统计口径见 [Rust 上位机](../../apps/ble-bench-host/README.md)。

## 当前协议与验证

当前为 [BLE v6](../../docs/ble-protocol.md)：HELLO、DATA、ACK 共用 17 字节固定开销，无包尾、无会话号。DATA 捎带累计确认，没有可发送 DATA 时立即发 ACK。固定窗口、不慢启动、不缩窗；每包按 `ceil(完整帧字节数/128) × 100/200/400 ms` 重传，最多发送 8 次。整包 N 字节承载 N−18 字节有效测试数据，不提供跨重启抗重放。

`G431_BENCH_BAUD` 可覆盖编译目标波特率，默认 115200；主机 `--baud` 仅校验，不发送 AT。AT 扫描、配置、复位等待、超时和回读由板级 `firmware/src/at_setup.rs` 完成；驱动直接使用 USART1 的 `embedded_io_async::Read/Write` 实现，不再定义 AtPort 或时序接口。

v6 已完成烧录、AT 回读、故障注入恢复和短时同时双向验证，尚未重跑完整矩阵。历史矩阵不代表当前版本，测试记录按项目约束清理、不归档。doBT-8AS 是独立可复用驱动，当前板级示例仍连接 doBT-01X，不代表 8AS 已经过实机验证。
