# 资源监视上位机

基于 `sysinfo` 采集系统状态、`serialport` 传输，并通过共享的 `monitor-protocol` 编码。
不依赖任何硬件示例或 UI crate。

在仓库根目录运行：

```sh
cargo run --locked -p pc-monitor-host -- --help
cargo run --locked -p pc-monitor-host -- --list-interfaces
cargo run --locked -p pc-monitor-host -- --dry-run --count 5
```

Windows 使用 `--port COM3`，Linux 使用 `--port /dev/ttyACM0`，macOS 使用 `--port /dev/cu.usbmodem...`。
未指定端口时默认按原型 VID/PID `CAFE:4001` 筛选；可用 `--vid 0x1234 --pid 0xabcd` 指定其他设备实际使用的标识（示例数值不是分配给本项目的产品标识）。
自动发现多个设备时要求指定 `--port`，不会任选一块连接。

网卡可用 `--interface "名称"` 精确选择；`--list-interfaces` 输出默认聚合范围。
`--demo --dry-run` 可检查协议输出而不读取真实指标、不连接设备。

## Linux 串口权限

在使用 systemd-logind 的本地桌面上，可以安装匹配自己设备的 udev 规则。例如仓库附带的原型规则：

```sh
sudo install -m 0644 packaging/linux/70-pc-monitor.rules /etc/udev/rules.d/
sudo udevadm control --reload-rules
```

随后重新插拔设备。无桌面或 SSH 会话可能不应用 `uaccess`，应根据串口实际属组配置当前用户（常见 `dialout` 或 `uucp`），重新登录生效。无需长期以 root 运行采集程序。

## 生命周期

这是前台程序，不注册自启动、不安装后台服务。`Ctrl-C` 停止后串口句柄随进程退出释放。
USB 断开或握手失败会间隔重试；选定网卡暂时消失时计入零流量，恢复后重新建立差值基线。
接收端应独立实现数据超时策略。
