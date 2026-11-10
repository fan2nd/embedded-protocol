# dobt8as-driver

依据[doBT-8AS AT 手册](../../docs/hardware/dobt8as-at.pdf) 中 **doBT-8AS 从机 AT 指令手册 V1.1.5**（PDF 第 5–28 页）实现。后面的 doBT-8AM/8AMS 是其他型号，不混用。`no_std`、无堆分配，无 HAL、OS、板卡和业务名称依赖。

共享 `at-transport::Transport`，与 `dobt01-driver` 独立；不把 01X 的 `AT+QT`、`AT+CTxx`、`AT+CZ` 用在 8AS 上。

## 支持范围

- 查询版本、设备名、蓝牙 MAC、波特率；设置名称/MAC/波特率。
- 查询/设置广播开关及广播间隔（20–10240 ms）。
- 查询/设置连接参数模式（0 性能、1 兼容、2 低功耗），设置成功会重启。
- 查询/设置连接后的 AT 识别、启动及主动日志、MTU（23–247）、4 个 16-bit UUID。
- 查询/设置低功耗和自动断开时间（0 或 5–3600 秒）。
- 软复位、软关机、从机通道断连 `AT+DISC=8`。
- 有界增量解析参数行、末尾 OK、ERR 错误码、连接/断连通知。

暂未覆盖 LED、GPIO、自定义广播/扫描回复数据和恢复出厂设置；不提供未经手册定义的主机扫描、主动连接或角色切换接口。UUID 不自动修改。MAC API 使用手册的 **小端线序字节**；例如人类显示 ABCDEF123456 对应 `[0x56,0x34,0x12,0xef,0xcd,0xab]`。

## 处理逻辑

1. `begin` 校验参数并编码完整 CRLF 命令；一条未完成时返回 Busy，输出缓冲不足不建立事务。
2. `feed` 用 128 B 行缓冲接收；必须 CRLF 结束，超长行丢弃至 LF 后报 Overflow。返回值最多保留 96 B；不分配队列，调用方及时消费事件。
3. 匹配的 `+KEY:value` 仅缓存为候选，收到后续 `OK` 才返回 Complete。孤立 OK 不算成功；无关参数行和连接通知不会完成当前事务。`ERR:n` 返回明确设备错误码。
4. `execute` 调用传输适配器排空旧输入并发送，整条事务截止 800 ms，超时/I/O 失败清除 pending。AT 没有事务号，排空无法保证区分同类型迟到回复。
5. `AT+BAUD=n` **立即生效**，旧波特率回复可能乱码。使用 `change_baud`：完整写入并 flush→切本地 UART→新波特率查询并验证。不等待旧速率 OK；普通 `execute(SetBaud)` 明确拒绝。

## 初始化与透传

```rust,ignore
let mut driver = dobt8as_driver::Driver::new();
let ready = driver.configure(&mut port, dobt8as_driver::setup::Options {
    name: "my-device",
    baud: 115200,
    reset_wait_ms: 2000, // 调用方提供的复位稳定等待策略，不声称是手册硬性值。
}).await?;
// port 实现 at_transport::Transport；ready 不代表 BLE 已连接。
// 应用验证二进制对端握手后：
driver.confirm_peer();
driver.enter_transparent()?;
```

`configure` 扫描手册支持的波特率（每档最多 3 次查询），必要时改速率/名称，关闭 LOG 和低功耗，复位后回读速率/名称/LOG，重新禁用并回读低功耗，最后设置并回读 `AT+CMD=0`。Reset 的 OK 若因重启丢失可超时，但后续验证必须通过。返回 Ready 仅表示这些配置已经验证；LOG=0 后连接通知可能不再上报，因此使用外部验证握手确认连接。不得在有并发透传数据时运行 configure。

**AT 防碰撞边界（PDF 第 19 页）：** `AT+CMD=0` 关闭连接后普通 AT 识别，但 `AT+CMD?` 和 `AT+CMD=...` 两种命令仍会被识别，不是完全关闭 AT 解释器。协议二进制包头、整帧 UART 提交和停止并发 AT 是应用侧约束，不能宣称任意切片数据绝对免碰撞。

透传门控是本地状态：`enter_transparent` 要求已确认对端连接且已收到 CMD=0 的成功回复，此后拒绝所有 AT 并不解析二进制载荷。要恢复命令操作，调用方先停止并排空数据，再 `resume_at_after_quiesce`；连接状态下 CMD=0 时只放行 CMD 查询/设置，先 CMD=1 后才能操作其他命令。外部确认断开后调用 `disconnected`。不自动发送逃逸字符或恢复出厂命令。

## 验证状态

- 已有主机测试：参数与范围、MAC/UUID 编码、分块多行回复、URC 穿插、孤立 OK、溢出、ERR、透传门控、即时改波特率/失败回读、完整配置回读。
- 可为 `thumbv7em-none-eabihf` 编译，无标准库依赖。
- **未在 doBT-8AS 实物上验证**；当前 G431 接的是 doBT-01X，不能用其通过的测试冒充 8AS 验证。

`cargo test --locked -p dobt8as-driver`
