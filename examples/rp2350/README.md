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
- `ui/src/page.rs`：CPU / 内存 240° 圆弧仪表、网络卡片、历史页与离线页。
- `ui/src/typography.rs`：应用层小屏 6×10 标注；数值使用上游 `FontSize::Normal`。
- `ui/src/framebuffer.rs`：RGB565 大端帧缓冲。业务字体和图表不进入通用 UI 仓库。

## 双核、双缓冲和脏区

Core 0 运行 UI 与 LCD DMA 发送任务，Core 1 初始化并并发运行 USB CDC 与只读 MSC 虚拟 U 盘。
跨核仅交换最新指标快照，使用启用跨核临界区实现的 `CriticalSectionRawMutex`；锁内不绘图、不等待传输。

两块完整 RGB565 帧缓冲各 64,800 字节，共 129,600 字节，静态分配在片内 SRAM。

1. UI 仅在 `Scene::damage()` 非空时绘制 back buffer。
2. 绘制范围合并“本次脏区”和“上次原始脏区”，修复交替缓冲落后一帧的内容。
3. `READY` 队列交出缓冲唯一可变引用；LCD 只发送本次脏区，不重复发送仅用于 RAM 修复的范围。
4. 设置 ST7789 窗口后按行跨度进行 SPI1 TX DMA，等待 DMA 完成和 SPI FIFO 排空后经 `FREE` 队列返还缓冲。

第一页空闲时不绘制、不传输像素；历史页每秒推进时间轴。CPU 与内存共用 0–100% 纵轴，分别用青色与紫色区分；历史按单调时钟定位，只显示最近 60 秒，采样间隔变化不改变窗口时长。按 KEY1 切换状态/历史页，数据超过 3 秒未更新则显示离线。
沿用上游单个合并包围矩形，不是离散脏区列表。屏幕接口没有 TE，因此双缓冲保证发送数据不被同时修改，但不等同于面板 VSync，不能保证物理扫描绝对无撕裂。

## 构建、预览和烧录

以下命令均在**仓库根目录**执行：

```sh
rustup target add thumbv8m.main-none-eabihf
# 先把三个平台的原生构建产物放到 output/host/<target>/：
# x86_64-pc-windows-gnu/pc-monitor-host.exe
# x86_64-unknown-linux-musl/pc-monitor-host
# aarch64-apple-darwin/pc-monitor-host
# 可使用 tools/build-host.sh <target>（交叉构建还需要对应链接器）。
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

双核系统时钟显式设为额定最高 150 MHz，USB 保持 48 MHz。旧版 HAL 默认配置也运行在 150 MHz；本次明确锁定该配置，没有超频。
显示循环额外等待由 20 ms 缩短到 1 ms；实际更新速度仍由采样间隔、绘制、SPI 和面板扫描共同决定，不宣称实测最高帧率。
SPI 请求 32 MHz，在 150 MHz 外设时钟及整数分频下实际为 25 MHz，使用 DMA_CH0。屏幕默认偏移 `(40,53)`、横屏、RGB565、反色开启。
若实际屏幕方向、颜色或边界不同，调整 `firmware/src/lcd.rs` 中参数。
USB 原型 VID/PID 为 `CAFE:4001`；这些是示例配置，不属于 PCMON1 协议。

## 验证边界

当前目录重构与上游 tinyui 改版已通过构建、静态检查和桌面像素对照。2026-11-08 已将新版 UF2 烧录到本板，重启后确认 USB 枚举为 `RP2350 PC Monitor`（`CAFE:4001`）。本次未运行上位机，尚未验证新版的实时数据展示；原版双缓冲固件曾完成 USB 握手验证。
源框架约定禁止新增框架单元测试，本项目使用原有界面预览检查流程，不向上游新增测试或修改字体实现。

## 板载三端上位机虚拟盘

原生 USB 同时枚举 CDC 串口和 `PCMON HOST` 只读 FAT16 磁盘；TF 卡不参与。
盘内 `WIN64.EXE`、`LINUX64`、`MACARM` 分别是 Windows x64、Linux x64 静态 musl、macOS Apple Silicon 上位机，以及使用说明和 SHA-256 校验文件。
三端程序直接存放，不压缩；复制对应文件到电脑后即可运行。macOS 执行 `chmod +x MACARM` 后运行 `./MACARM --interval 0.25`；Linux 同理使用 `LINUX64`；Windows 运行 `WIN64.EXE`。
盘内 `HOST.TXT` 为详细说明，`PCMON.RUL` 为示例 udev 规则（安装时命名为 `70-pc-monitor.rules`）。
推荐 `--interval 0.25`，提高指标更新频率。没有自动启动或后台安装。

`tools/make_disk.py` 必须找到三个真实可执行程序，缺失时直接失败，不放占位文件。
磁盘镜像以 `include_bytes!` 存入 XIP Flash，不占用同等大小 SRAM，不执行 Flash 擦写；写请求报告写保护。
生成器限制镜像不超过 3 MiB，链接脚本与 UF2 工具继续限制总固件在 4 MiB 内。
`PCMON_DISK_IMAGE` 可指定预先生成的镜像供直接 cargo 构建使用。

本次新增 MSC、240° 仪表及 60 秒历史必须重新烧录后验证；之前 USB 枚举记录仅适用于旧固件。
Windows/Linux 包在 Mac 上交叉构建，尚未在对应系统执行，也尚未完成三系统实机挂载验证。

Apple Silicon 上可用附带的交叉构建脚本（需 Zig 0.13.0 和对应 Rust targets）：

```sh
PCMON_ZIG=/absolute/path/to/zig sh tools/build-disk-hosts-macos.sh
sh examples/rp2350/build.sh
python3 examples/rp2350/tools/test_disk.py
PCMON_DISK_IMAGE="$PWD/output/examples/rp2350/host-tools.img" \
  cargo test --locked --manifest-path examples/rp2350/tools/usb-check/Cargo.toml
```

USB 桌面检查复用固件存储代码，模拟容量查询、扇区读取、越界读取、写保护后的后续命令、短响应和非法 CBW。
它不代替真实 USB 主机的枚举、复位恢复与并发 CDC 验证。
Windows 交叉程序采用 panic=abort，并静态链接 Zig compiler-rt / libunwind；导入表仅含 Windows 系统 DLL，不附带 MinGW DLL。

### Windows 文件损坏错误修复

虚拟盘生成器至少保留 4128 个数据簇，避开 FAT12/FAT16 判定边界。
旧镜像只有 4085 簇，微软公开的 fastfat 驱动按 `< 4087` 判为 FAT12，
会把 FAT16 簇链按错误格式解析，造成能看到文件名但读文件/复制失败的现象。
依据：[Microsoft fastfat FatIndexBitSize](https://github.com/microsoft/Windows-driver-samples/blob/main/filesys/fastfat/fat.h)。
回归检查包含 Windows 的类型判定，并验证全部文件及 SHA-256；不只检查 macOS 挂载兼容性。
修复版同时更新卷序列号。重新烧录后断开再连接 USB，再将 `WIN64.EXE` 复制到本地测试。
这次修复针对虚拟盘读取，Windows 程序本身仍需在目标电脑上运行验证。
