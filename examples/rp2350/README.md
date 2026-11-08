# RP2350 资源监视屏示例

这是主项目协议的一个接收端示例，不是主项目的目标平台限定。
使用 ATK-DNRP2350AM V1.0、ST7789 240×135 屏幕、Rust 和 `embassy-rp`。
原理图位于 [hardware/ATK-DNRP2350AM_V1.0.PDF](hardware/ATK-DNRP2350AM_V1.0.PDF)。

## 本示例目录

```text
examples/rp2350/
├── firmware/       Embassy 双核固件、USB 与 SPI DMA
├── ui/             面向 240×135 的业务界面与帧缓冲适配
├── hardware/       原理图
├── tools/          本示例专用 ELF → UF2 打包工具
└── build.sh        仅构建本硬件固件
```

固件直接依赖共享 `monitor-protocol` 解码。UI 只消费 `Metrics`，上位机不依赖此示例。

## tinyui 界面

直接使用 [fan2nd/tinyui](https://github.com/fan2nd/tinyui) 的固定提交
`8a628f031ee692bcfdab5a548e07cde2be760982`，版本与来源记录在根目录 `Cargo.toml` / `Cargo.lock`。
不再包含从 ESP32P4 目录复制的框架副本，也不修改上游源码。

- `tiny_ui::Scene`、`Panel`、`Label` 提供保留式组件、布局属性和脏区管理。
- `ui/src/page.rs`：CPU 主卡片/趋势图、内存环形仪表、网络卡片、历史页与离线页。
- `ui/src/typography.rs`：应用层小屏 6×10 标注；数值使用上游 `FontSize::Normal`。
- `ui/src/framebuffer.rs`：RGB565 大端帧缓冲。业务字体和图表不进入通用 UI 仓库。

## 双核、双缓冲和脏区

Core 0 运行 UI 与 LCD DMA 发送任务，Core 1 初始化并运行 USB CDC。
跨核仅交换最新指标快照，使用启用跨核临界区实现的 `CriticalSectionRawMutex`；锁内不绘图、不等待传输。

两块完整 RGB565 帧缓冲各 64,800 字节，共 129,600 字节，静态分配在片内 SRAM。

1. UI 仅在 `Scene::damage()` 非空时绘制 back buffer。
2. 绘制范围合并“本次脏区”和“上次原始脏区”，修复交替缓冲落后一帧的内容。
3. `READY` 队列交出缓冲唯一可变引用；LCD 只发送本次脏区，不重复发送仅用于 RAM 修复的范围。
4. 设置 ST7789 窗口后按行跨度进行 SPI1 TX DMA，等待 DMA 完成和 SPI FIFO 排空后经 `FREE` 队列返还缓冲。

空闲时不绘制、不传输像素。按 KEY1 切换状态/历史页，数据超过 3 秒未更新则显示离线。
沿用上游单个合并包围矩形，不是离散脏区列表。屏幕接口没有 TE，因此双缓冲保证发送数据不被同时修改，但不等同于面板 VSync，不能保证物理扫描绝对无撕裂。

## 构建、预览和烧录

以下命令均在**仓库根目录**执行：

```sh
rustup target add thumbv8m.main-none-eabihf
sh examples/rp2350/build.sh
cargo run --locked -p rp2350-dashboard --example preview -- output/examples/rp2350
```

固件输出：`output/examples/rp2350/pc-monitor.uf2`，对应 ELF 在同目录。
预览输出 PPM，可在 macOS 上用 `sips -s format png` 转换为 PNG。
预览使用实际绘图代码，并模拟交替缓冲和 LCD 局部更新，对照完整渲染；它不替代硬件时序验证。

先拔出 TF 卡，按住 BOOT、短按 RESET 后松开 BOOT，用**原生 USB-C**连接电脑，将 UF2 拖入 RP2350 下载盘。
另一个 CH343 USB_UART 接口不是本示例使用的 USB 接口。
重启后按根目录上位机说明启动采集程序即可；构建脚本不会自动烧录或运行采集。

```sh
cargo clippy --locked -p rp2350-dashboard --all-targets -- -D warnings
cargo clippy --locked -p rp2350-monitor --target thumbv8m.main-none-eabihf --release -- -D warnings
```

## 引脚和屏幕参数

| 功能 | GPIO / 连接 | 说明 |
|---|---|---|
| LCD DC | GPIO8 | 数据/命令 |
| LCD CS | GPIO9 | 低有效 |
| LCD SCK / MOSI | GPIO10 / GPIO11，SPI1 | 与 TF CLK / CMD 共用 |
| LCD 背光 | GPIO25 | S8550 PNP，低有效 |
| LCD RESET | 整板 RESET | 使用软件复位，不驱动整板复位 |
| KEY1 | GPIO2 | 上拉，按下低电平 |
| 用户 LED | GPIO3 | 低有效，新鲜数据时点亮 |
| TF CS | GPIO15 | 保持高，不初始化 TF；演示时拔卡 |
| 原生 USB | RP2350 USB_DP/DM | PCMON1 通信 |
| USB_UART | CH343 → GPIO0/1 | 本示例不用 |

SPI 时钟为 32 MHz，使用 DMA_CH0。屏幕默认偏移 `(40,53)`、横屏、RGB565、反色开启。
若实际屏幕方向、颜色或边界不同，调整 `firmware/src/lcd.rs` 中参数。
USB 原型 VID/PID 为 `CAFE:4001`；这些是示例配置，不属于 PCMON1 协议。

## 验证边界

当前目录重构与上游 tinyui 改版已通过构建、静态检查和桌面像素对照。2026-11-08 已将新版 UF2 烧录到本板，重启后确认 USB 枚举为 `RP2350 PC Monitor`（`CAFE:4001`）。本次未运行上位机，尚未验证新版的实时数据展示；原版双缓冲固件曾完成 USB 握手验证。
源框架约定禁止新增框架单元测试，本项目使用原有界面预览检查流程，不向上游新增测试或修改字体实现。
