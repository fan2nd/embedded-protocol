# 跨平台资源监视器

这是一个以**资源监视协议和跨平台上位机**为核心的项目。
电脑端采集 CPU、内存、网络速率与运行时间，通过公开的 PCMON1 协议发送给接收端。
接收端可以使用不同 MCU、屏幕或其他软件实现；仓库附带的 RP2350 小屏仅是一个硬件示例，不是协议或上位机的依赖。

## 项目组成

| 部分 | 位置 | 职责 |
|---|---|---|
| 协议规范 | [docs/protocol.md](docs/protocol.md) | 帧格式、握手、单位、边界与接收约定 |
| 协议实现 | [crates/monitor-protocol](crates/monitor-protocol) | 无依赖、`no_std`、无堆分配的编码与流式解码 |
| 上位机 | [apps/host](apps/host) | Windows / Linux / macOS 状态采集、串口发现、发送与重连 |
| 硬件示例 | [examples/rp2350](examples/rp2350) | Embassy 双核固件、tinyui 界面、原理图与烧录工具 |
| 打包 | [tools](tools) | 各平台上位机构建脚本 |

```text
系统状态 → 上位机采集 → PCMON1 字节流 → 接收端
                                     ├─ RP2350 示例
                                     ├─ 其他 MCU / 屏幕
                                     └─ 软件接收器
```

协议不包含 GPIO、屏幕尺寸或 USB VID/PID。现有上位机实现的传输方式是串口/USB CDC；其他传输需自行适配字节流，并非已经实现 TCP 等传输。

## 快速开始：只运行上位机

开发环境需要 Rust stable 和系统原生链接器。运行已编译程序不需要 Rust 或 Python。
在仓库根目录执行：

```sh
cargo build --release --locked -p pc-monitor-host
# 只查看数据，不连接硬件
cargo run --locked -p pc-monitor-host -- --dry-run --count 5
# 查看接收端和网卡
cargo run --locked -p pc-monitor-host -- --list-ports
cargo run --locked -p pc-monitor-host -- --list-interfaces
```

Windows 使用 `target/release/pc-monitor-host.exe`，macOS/Linux 使用 `target/release/pc-monitor-host`。

```sh
# macOS
./target/release/pc-monitor-host --port /dev/cu.usbmodem实际设备名
# Linux
./target/release/pc-monitor-host --port /dev/ttyACM0 --interface enp3s0
```

```powershell
# Windows PowerShell，网卡名可包含空格或中文
.\target\release\pc-monitor-host.exe --port COM3 --interface "以太网"
```

串口打开后先执行协议握手，收到 `PCMON1 READY` 才开始发送。程序以前台方式运行，`Ctrl-C` 停止。
不指定 `--port` 时，默认查找原型设备标识 `CAFE:4001`；其他硬件可传入 `--vid HEX --pid HEX`，或直接指定端口。
多个匹配设备需要手动选择，不会向所有串口盲目发送探测命令。

### 采集选项

- `--interval 0.5`：采样间隔，允许 0.25–2 秒。
- `--interface NAME`：仅统计指定网卡；`--list-interfaces` 中的 `*` 表示默认选择。
- `--dry-run`：输出协议帧，不连接接收端。
- `--demo`：使用模拟数据，便于调试接收端。
- `--count N`：发送/输出 N 帧后退出；默认持续运行。

CPU 为所有核平均占用率；内存为 `sysinfo` 的 used/total，不等同于 macOS“内存压力”。
网络速率按实际采样间隔计算，接口新增、消失或计数器复位不会产生巨额假流量。GPU 和温度暂未包含。
macOS 默认选择 `en*`；Linux 默认选择 `en*`、`eth*`、`wl*`、`ww*`、`usb*`；Windows 默认选择非回环接口，支持本地化网卡名。
VPN、虚拟网卡或自定义名称可能影响默认统计范围，需精确统计时使用 `--interface`。

## 构建与打包

这是一个 Cargo workspace，根目录 `cargo build` / `cargo test` 默认只处理协议与上位机，不编译硬件固件。
所有组件共享根目录 `Cargo.lock`；硬件示例的 Git UI 依赖也记录在该锁文件中。

```sh
# 在 macOS / Linux 原生构建并打包
sh tools/build-host.sh
```

```powershell
# 在 Windows 原生构建并打包
.\tools\build-host.ps1
```

产物在 `output/host/<target>/`，压缩包位于 `output/`。
Windows 构建需要 MSVC Rust 和 Visual Studio C++ Build Tools；Linux 需要本地 C 工具链；macOS 需要 Xcode Command Line Tools。
Linux GNU 二进制的 glibc 兼容范围取决于构建系统，不宣称跨所有发行版通用。

[GitHub Actions](.github/workflows/host-platforms.yml) 配置了 Windows x64、Linux x64/ARM64、macOS Intel/Apple Silicon 五个平台的原生检查、采集冒烟运行和产物上传。
本地验证环境是 Apple Silicon Mac；其他平台是否构建通过以对应 CI 结果为准，CI 不替代真实 USB 设备验证。

## 接入其他硬件

实现 [PCMON1 协议](docs/protocol.md)，为接收端提供能够收发字节流的接口即可。
Rust 接收端可直接使用 `monitor-protocol`；其他语言按协议规范实现，不需要使用 tinyui 或 Embassy。
USB 自动发现标识由设备和上位机参数约定，协议本身不绑定任何开发板。

Linux 用户需拥有对应串口权限。仓库的 [示例 udev 规则](packaging/linux/70-pc-monitor.rules) 仅匹配原型标识 `CAFE:4001`；其他设备应改为自己的标识。
使用 systemd-logind 的桌面可按需手动安装规则、重载 udev 后重新插拔设备；SSH/无桌面环境通常需配置串口属组（常见 `dialout` 或 `uucp`）并重新登录。
详见 [上位机说明](apps/host/README.md)。

## RP2350 示例

[examples/rp2350/README.md](examples/rp2350/README.md) 包含该板的引脚、屏幕参数、构建与烧录说明。
示例使用固定提交的 [fan2nd/tinyui](https://github.com/fan2nd/tinyui)，Core 0 负责双缓冲/脏区/DMA 显示，Core 1 负责 USB。
该目录可以独立替换；协议 crate 和上位机不依赖其 UI、HAL 或硬件文件。

## 检查

```sh
cargo fmt --all -- --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo tree -p pc-monitor-host
```

固件和界面预览需显式构建，命令见对应示例。执行构建或测试不会自动烧录设备，也不会启动后台采集服务。
