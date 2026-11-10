# dobt8as-driver

依据[doBT-8AS AT 手册](../../docs/hardware/dobt8as-at.pdf) 中 **doBT-8AS 从机 AT 指令手册 V1.1.5**（PDF 第 5–28 页）实现。后面的 doBT-8AM/8AMS 是其他型号，不混用。`no_std`、无堆分配，无 HAL、OS、板卡和业务名称依赖。

直接使用 `embedded_io_async::Read + Write`，与 `dobt01-driver` 独立；不把 01X 的 `AT+QT`、`AT+CTxx`、`AT+CZ` 用在 8AS 上。

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
4. `execute` 通过标准异步 `write_all`/`flush`/`read` 发送并等待匹配回复；按字节读取，不吞掉响应后的通知。调用方负责超时、旧输入排空、配置顺序及复位等待。事务 Future 被取消或 I/O 失败会释放 pending；EOF 与超时不同，返回 `UnexpectedEof`。AT 没有事务号，无法保证区分同类型迟到回复。
5. `AT+BAUD=n` **立即生效**，使用 `send_command(SetBaud(...))` 仅发送并 flush，不等旧速率 OK；调用方切换本地 UART、排空旧响应，再 `execute(Query(Baud))` 回读验证。普通 `execute(SetBaud)` 返回 `UseSendCommand`，不隐式改硬件。

## 初始化与透传

驱动不提供自动 configure 或波特率切换。调用方扫描各档波特率、设置名称和 LOG/低功耗、按需复位并等待，查询验证设置，最后设置并回读 CMD=0。低功耗造成漏收时，重试次数和超时均由调用方决定。不得与透传数据并发发送 AT。

```rust,ignore
let mut driver = dobt8as_driver::Driver::new();
// io 实现 embedded_io_async::Read + Write；调用方为整个事务设置超时。
let reply = driver.execute(&mut io, Command::Query(Field::Baud)).await?;
// 改速率：仅提交命令，之后由调用方配置本地 UART 并回读校验。
driver.send_command(&mut io, Command::SetBaud(115200)).await?;
// ...调用方切换 UART、处理残留输入...
let reply = driver.execute(&mut io, Command::Query(Field::Baud)).await?;
assert_eq!(reply.number(), Some(115200));
// ...设置并确认 CMD=0，验证二进制对端握手后...
driver.confirm_peer();
driver.enter_transparent()?;
```

`send_command` 成功仅表示本地写入完成，不代表模块执行成功，也不更新未经回读验证的状态。超时取消同样不能撤回已发送字节；再次发命令前由调用方让链路静止并排空旧回复。需要自行消费所有 URC 时，可使用 `begin`/`feed` 低层接口。

**AT 防碰撞边界（PDF 第 19 页）：** `AT+CMD=0` 关闭连接后普通 AT 识别，但 `AT+CMD?` 和 `AT+CMD=...` 两种命令仍会被识别，不是完全关闭 AT 解释器。协议二进制包头、整帧 UART 提交和停止并发 AT 是应用侧约束，不能宣称任意切片数据绝对免碰撞。

透传门控是本地状态：`enter_transparent` 要求已确认对端连接且已收到 CMD=0 的成功回复，此后拒绝所有 AT 并不解析二进制载荷。要恢复命令操作，调用方先停止并排空数据，再 `resume_at_after_quiesce`；连接状态下 CMD=0 时只放行 CMD 查询/设置，先 CMD=1 后才能操作其他命令。外部确认断开后调用 `disconnected`。不自动发送逃逸字符或恢复出厂命令。

## 验证状态

- 已有主机测试：参数与范围、MAC/UUID 编码、分块多行回复、URC 穿插、孤立 OK、溢出、ERR、透传门控、异步 Pending/部分写入、EOF/I/O 失败、取消释放事务、不等待应答的波特率命令提交。
- 可为 `thumbv7em-none-eabihf` 编译，无标准库依赖。
- **未在 doBT-8AS 实物上验证**；当前 G431 接的是 doBT-01X，不能用其通过的测试冒充 8AS 验证。

`cargo test --locked -p dobt8as-driver`
