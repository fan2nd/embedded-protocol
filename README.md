# RP2350 双核电脑状态显示器

基于 ATK-DNRP2350AM V1.0，使用 Rust + `embassy-rp`，电脑端同样为 Rust。
屏幕是 ST7789、240×135 横屏。电脑端源码支持 macOS、Windows、Linux；运行采集程序无需 Python，固件构建无需 Pico SDK 或额外 ARM GCC。

## 已实现

- Core 0：tiny-ui 图形界面、两块 RGB565 帧缓冲、SPI1 TX DMA、按键消抖和页面切换。
- Core 1：独立 Embassy executor，初始化原生 USB 驱动，运行 USB CDC 和数据接收任务。
- 跨核通信：`CriticalSectionRawMutex` 保护最新数据快照；启用 `embassy-rp/critical-section-impl`。锁内仅复制数据，不绘图、不等待 USB。
- 状态页：CPU/内存环形仪表、芯片图标、百分比、迷你趋势和网络速率卡片，速率自动切换 KiB/s、MiB/s、GiB/s。
- 按 KEY1 切换趋势页：最近 106 次接收的 CPU/内存双曲线和电脑运行时间。默认采样间隔约 0.5 秒；断线重连后清空历史，避免把间断数据画成连续采样。
- 超过 3 秒没有有效数据即显示 OFFLINE，不把旧数值冒充实时状态。
- 跨平台采集程序：USB 自动识别、握手校验、断线重连，可列出并指定网络接口。

UI 每 20 ms 检查输入和状态，只在内容变化时绘制并提交；静止时不绘图、不发送像素。LCD 像素传输采用 DMA，USB 核独立运行。
屏幕接口没有引出 TE，因此 SRAM 双缓冲避免并发修改发送数据，但不等同于面板 VSync，不能保证物理扫描完全无撕裂。未使用 TF 存储。

## tiny-ui 移植与局部刷新

框架从 `~/esp32p4/3rd/tiny-ui` 复制到本项目 `3rd/tiny-ui`，不依赖另一项目目录，也未修改源项目。
保留其保留式 `Scene`、`Panel`、`Label`、变更失效、裁剪重绘与上一帧 damage 修复；仅补充小屏适用的 6×10 字号。详见 `3rd/tiny-ui/PORT.md`。

| 层 | 文件 | 职责 |
|---|---|---|
| 通用 UI 框架 | `3rd/tiny-ui/src/` | 组件、样式、Scene、脏矩形 |
| 业务页面 | `dashboard/src/page.rs` | 状态更新、布局、环形仪表和曲线覆盖层 |
| 内存绘图适配 | `dashboard/src/framebuffer.rs` | RGB565 大端字节、裁剪、行填充 |
| 缓冲调度 | `firmware/src/main.rs`、`display.rs` | UI 生产帧、DMA 消费帧、独占所有权交换 |
| 屏幕硬件 | `firmware/src/lcd.rs` | ST7789 初始化、窗口设置、DMA 按行发送 |

两块完整帧缓冲各 64,800 字节，总计 129,600 字节（126.6 KiB），均静态存于内部 SRAM，不分配堆内存。

1. UI 仅在 `Scene::damage()` 非空时绘制 back buffer。
2. RAM 绘制范围为“本次 damage ∪ 上次原始 damage”，修复交替缓冲落后一帧的像素。
3. `READY` 队列把整个缓冲的唯一可变引用和**本次 damage**交给 LCD 任务，UI 不再访问它。
4. LCD 设置 `CASET/RASET` 窗口，仅发送本次 damage；窄矩形按完整帧的行跨度逐行 DMA，整宽区域一次 DMA。
5. 等待 DMA 完成及 SPI FIFO 排空后，通过 `FREE` 队列返还缓冲。另一个缓冲可在此期间绘制。

必须保存未合并的本次 damage，不能把含历史修复的区域反复累积到下一帧。初次上线、页面切换、离线切换全屏失效；普通数据变动局部失效。
目前沿用源框架的**单个合并包围矩形**，不是多个离散脏矩形列表；同时变动的相距较远区域会一起传输其包围盒。

## 快速使用

本次构建文件位于 `output/`：

- `pc-monitor.uf2`：RP2350 Secure ARM 固件。
- `pc-monitor.elf`：含调试符号的固件。
- `pc-monitor-host`：当前 Mac 架构的本机程序。
- `dashboard.png`、`history.png`、`offline.png`：用固件相同绘制代码生成的预览，不是实物照片。
- `damage-rx-only.png`：紫框标记仅下载速率变化时的 LCD 发送区域。

1. 先拔出 TF 卡。按住板上 BOOT，再按一下 RESET，松开 BOOT，进入 USB 下载模式。
2. **接原生 USB-C 口**，不是接 CH343 的 USB_UART 口；把 UF2 拖入出现的 RP2350 下载磁盘。
3. 板子重启后，在本项目目录执行：

```sh
./output/pc-monitor-host
```

按 `Ctrl-C` 停止。若有多个设备或自动发现失败：

```sh
./output/pc-monitor-host --list-ports
./output/pc-monitor-host --port /dev/cu.usbmodem实际设备名
```

电脑端不会自动烧录固件、注册开机启动或更改系统设置。未自动连接实物板烧录。

## 构建

安装 Rust stable 后，在项目根目录运行：

```sh
rustup target add thumbv8m.main-none-eabihf
sh tools/build.sh
```

首次构建需要下载 Cargo 依赖；各 crate 的 `Cargo.lock` 已保存。
固件依赖固定为 `embassy-rp 0.9.0`、`embassy-executor 0.9.1`、`embassy-usb 0.5.1`。
必须在 `firmware/` 内构建固件，使其 `.cargo/config.toml` 生效；构建脚本已处理。

`tools/elf2uf2.rs` 只打包本项目的 flash-linked ELF32 ARM 镜像，使用 RP2350 Secure ARM family ID `0xe48bff59`。
它不是通用 UF2 工具，不适用于 RAM 固件或其他 MCU。

## 采集选项

```sh
# 不连接板子，只输出真实电脑状态
./output/pc-monitor-host --dry-run --count 5

# 无硬件的协议示例；数据明确为模拟值
./output/pc-monitor-host --demo --dry-run --count 5

# 仅统计 Wi-Fi/有线接口 en0
./output/pc-monitor-host --interface en0

# 显示测试用模拟负载，需要已连接的板子
./output/pc-monitor-host --demo
```

`--interval` 范围 0.25–2 秒，默认 0.5 秒。macOS 默认合计 `en*`；Linux 默认合计 `en*`、`eth*`、`wl*`、`ww*`、`usb*`；Windows 默认合计非回环接口，支持中文和自定义网卡名。
Windows 的 VPN/虚拟网卡也可能被计入，Linux 改名网卡可能不在默认列表。运行 `--list-interfaces` 查看带 `*` 的默认选择，再用 `--interface "网卡名"` 精确指定，可避免重复统计。
CPU 为所有核的平均使用率，MEM 为 `sysinfo` 的 used/total，不等同于 macOS“内存压力”。网络速率按实际采样间隔计算；接口出现、消失或计数器复位不会产生巨额假流量。
GPU 和 CPU 温度暂未提供。

## Windows / Linux / macOS 电脑端

各系统使用相同的 PCMON1 协议，当前已烧录的板子也能接收新版电脑端数据。
新版固件界面标题改为 `PC / MONITOR`，只是消除 macOS 专用文案；现有设备不必为通信兼容性重新烧录。

| 平台 | 原生构建命令 | 常见串口 | 验证状态 |
|---|---|---|---|
| macOS Apple Silicon | `sh tools/build-host.sh` | `/dev/cu.usbmodem...` | 本机编译、采集和 USB 通信已验证 |
| macOS Intel | 在 Intel Mac 执行同上命令 | `/dev/cu.usbmodem...` | 已配置构建流程，未在 Intel Mac 运行 |
| Windows x64 | PowerShell 执行 `./tools/build-host.ps1` | `COM3`、`COM10` | 已适配源码，未编译/实机验证 |
| Linux x64 / ARM64 | `sh tools/build-host.sh` | `/dev/ttyACM0` | 已适配源码，未编译/实机验证 |

构建电脑需要 Rust stable 及系统原生链接器：Windows 使用 MSVC Rust 和 Visual Studio C++ Build Tools；Linux 使用发行版的 C 编译工具链；macOS 使用 Xcode Command Line Tools。
脚本生成 `output/host/<target>/` 目录及压缩包。接收成品的电脑无需 Rust。Linux 包应在兼容的发行版上构建；GNU 包并非无条件兼容所有 glibc 版本。

Windows PowerShell 示例（进入生成的程序目录）：

```powershell
.\pc-monitor-host.exe --list-ports
.\pc-monitor-host.exe --list-interfaces
.\pc-monitor-host.exe --port COM3 --interface "以太网"
```

Linux 示例（进入生成的程序目录）：

```sh
./pc-monitor-host --list-ports
./pc-monitor-host --list-interfaces
./pc-monitor-host --port /dev/ttyACM0 --interface enp3s0
```

Linux 若出现 Permission denied，项目附带 `packaging/linux/70-pc-monitor.rules`，仅匹配本 demo 的 `CAFE:4001`。在使用 systemd-logind 的桌面上，可手动安装后重新插拔：

```sh
sudo install -m 0644 packaging/linux/70-pc-monitor.rules /etc/udev/rules.d/
sudo udevadm control --reload-rules
```

无桌面/SSH 环境可能不应用 `uaccess`；应根据设备实际属组配置用户权限（常见为 `dialout` 或 `uucp`），重新登录后生效。不建议把串口改成对所有用户可写，也不需要长期以 root 运行采集器。

`.github/workflows/host-platforms.yml` 提供五个原生运行器的构建矩阵：Windows x64、Linux x64/ARM64、macOS Intel/Apple Silicon。每项执行测试、Clippy、实际采集冒烟检查并上传压缩包；推送后可在 GitHub Actions 查看运行结果和下载产物，也可手动触发。
本地验证环境为 Apple Silicon Mac，未进行 Windows/Linux 本地交叉编译。其他平台的构建状态以对应 Actions 任务结果为准；CI 构建和采集检查不替代实物 USB 通信验证。

## 原理图引脚

来源：项目根目录的 `ATK-DNRP2350AM_V1.0.PDF` 两页原理图。

| 功能 | GPIO / 连接 | 说明 |
|---|---|---|
| LCD DC | GPIO8 | 数据/命令 |
| LCD CS | GPIO9 | 低有效 |
| LCD SCK | GPIO10 / SPI1 | 与 TF CLK 共用 |
| LCD MOSI | GPIO11 / SPI1 | 与 TF CMD 共用 |
| LCD 背光 | GPIO25 | S8550 PNP，低电平亮 |
| LCD RESET | 整板 RESET | 无独立 GPIO，使用 ST7789 软件复位 |
| KEY1 | GPIO2 | 上拉，按下低电平 |
| 红色用户 LED | GPIO3 | 低电平亮，有新鲜数据时点亮 |
| TF DATA3/CS | GPIO15 | 保持高，不初始化 TF；演示时拔卡 |
| 原生 USB | RP2350 USB_DP/DM | 连接电脑状态程序 |
| USB_UART | CH343 → GPIO0/1 | 本 demo 不使用 |

`firmware/src/lcd.rs` 集中配置 `MADCTL`、X/Y 偏移；`firmware/src/main.rs` 配置 SPI 频率和 DMA_CH0。当前为横屏 240×135、偏移 `(40,53)`、RGB、反色开启、32 MHz SPI。不同批次屏幕若偏移一行、方向或颜色不对，可按实际模块调整；这些参数仍需实物确认。

## USB 协议

CDC 是字节流：USB 包边界与消息边界无关。ASCII 换行分帧：

```text
电脑 -> HELLO\n
板子 -> PCMON1 READY\n
电脑 -> PCMON1,序号,CPU千分比,内存千分比,下行KiB每秒,上行KiB每秒,开机秒数\n
```

示例：`PCMON1,7,427,632,1280,96,14520` 表示 CPU 42.7%、内存 63.2%。CPU/MEM 限制 0–1000，其余为 `u32`；拒绝多字段、负数、溢出和非数字。超长行丢弃至下一换行后恢复解析。序号允许回绕、重连后重新开始。
跨核只保留最新快照，不积压过时数据。

演示 VID/PID 为 `CAFE:4001`、USB serial 为 `PCMON-DEMO-001`，用于本地原型；产品化时需分配合法标识和每板唯一序列号。

## 验证

```sh
cargo test --locked --manifest-path dashboard/Cargo.toml
cargo test --locked --manifest-path host/Cargo.toml
rustc --edition=2021 --test tools/elf2uf2.rs -o output/test-elf2uf2
./output/test-elf2uf2
(cd firmware && cargo clippy --release --locked -- -D warnings)
cargo clippy --locked --manifest-path host/Cargo.toml --all-targets -- -D warnings
```

已有测试覆盖协议分包/粘包/超长恢复、异常输入、主机编码与固件解析互通、握手、端口选择、网络计数器变化和 UF2 打包。移植框架遵循其原项目约束，未新增框架单元测试。

```sh
cargo clippy --manifest-path 3rd/tiny-ui/Cargo.toml --lib -- -D warnings
cargo clippy --manifest-path dashboard/Cargo.toml --all-targets -- -D warnings
cargo run --manifest-path dashboard/Cargo.toml --example preview -- output
sips -s format png output/dashboard.ppm --out output/dashboard.png
```

预览程序使用相同的业务页面和帧缓冲绘制代码，模拟两块缓冲轮换与 LCD GRAM 局部更新，并逐像素对照全量渲染。
覆盖稀疏更新、数字变宽/变窄、0/100%、历史滚动、切页、离线/重连和最大网络值；另检查 100 个空闲 tick 无提交。
仅 RX 文本更新的传输为 84×14×2 = 2,352 字节，约为全屏 64,800 字节的 3.6%；仅大图表更新为 212×43×2 = 18,232 字节。它们是特定场景的像素负载，不含命令开销，也不是硬件帧率实测。

完整的 USB 枚举、烧录启动、LCD 偏移/颜色、按键和双核长时间运行仍需上板验证。编译通过不能替代实物验证。

## 参考

- [Embassy 多核与跨核同步说明](https://docs.embassy.dev/embassy-rp/0.9.0/rp235xa/multicore/index.html)
- [Embassy RP235x 镜像与内存布局示例](https://github.com/embassy-rs/embassy/blob/main/examples/rp235x/memory.x)
- [UF2 family ID 官方表](https://github.com/microsoft/uf2/blob/master/utils/uf2families.json)
- [GitHub 原生运行器平台说明](https://docs.github.com/en/actions/reference/runners/github-hosted-runners)
