# Rust BLE 测速上位机

独立应用 crate，使用 `ble-link-protocol` 的 v6 帧与固定滑动窗口，`ble-bench-payload` 定义测试载荷。不依赖 HAL 或 PCMON 业务。

## 连接与调度

默认连接 G431-BLE，A003 write-without-response 写入、A002 notify 接收；订阅后握手校验版本及波特率（默认 115200）。A002 主动 Read 实测返回设备名，不用于收包。

`transport.rs` 负责 Bluest/CoreBluetooth、写队列背压及发送期间 ACK 处理；`bench.rs` 负责故障注入和统计；`mixed.rs` 负责随机包长三模式矩阵；`main.rs` 负责 CLI 和报告。接收输入队列 512 条、已解码帧队列 1024 条，溢出或流错误使本组失败。

每个 ATT 分片前和写队列等待期间处理 ACK，已确认重传副本取消后续分片。本地写入成功不等于对端收到。窗口启动即固定，受包数与字节预算限制；逐包累计确认后立即补包。每包按 `ceil(完整帧字节数/128) × 100/200/400 ms` 重传，最多发送 8 次，不加固定帧间延时。

## 构建与运行

在仓库根目录执行，需 macOS 蓝牙权限和配套 v6 固件：

```sh
cargo build --locked --release -p ble-bench-host
# 丢数据/丢 ACK 恢复验证
target/release/ble-bench-host --inject-loss --output output/stm32g431/v6-injected-new
# 固定包长 × 窗口；每组先下行再上行
target/release/ble-bench-host --matrix --output output/stm32g431/v6-matrix-new
# 短时同时双向
target/release/ble-bench-host --duplex --output output/stm32g431/v6-duplex-new
# 随机包长：4 个上限 × 4 个窗口 × 单向下行/单向上行/同时双向
target/release/ble-bench-host --mixed-duplex --mixed-max-frames 128,182,256,512 --output output/stm32g431/v6-random-new
```

`--name`、`--baud` 指定设备及预期串口配置；`--matrix-frames`、`--matrix-windows` 为逗号分隔列表，`--matrix-bytes` 指定每方向最低有效字节数。固定矩阵默认整包 64/128/182/256/512/1024 B，窗口 1/2/4/8，每方向至少 16384 B。

随机完整 DATA 帧在 `[25,最大包长]` 连续整数区间取值，固定种子、不同方向不同序列，所有窗口复用同一序列，重传保持长度。v6 固定开销 17 B，另有 1 B 测试标识，有效字节为整包减 18。双向使用相同窗口。

## 统计与结果

速率以接收端校验通过、按序去重的有效数据计算。丢包率为未完整收到有效包的发送尝试/实际 DATA 发送尝试；重传率为重传 DATA 次数/实际 DATA 发送次数。已提交部分分片后取消的副本计入发送尝试并单列，不等于业务数据缺失；未发送计划包不计分母。失败组保留，不作为稳定工作点。

报告每格同时显示下行/上行速率、丢包率、重传率。双向组采用共同总耗时（含恢复与统计收尾），另记录活动时间及捎带确认次数。`host_timing` 从 Rust 消费通知起计时，不包含 CoreBluetooth 内部等待。

当前 v6 只完成短时验证，尚未重跑完整矩阵；历史 v5 矩阵不能当作 v6 结果。测试输出仅供当次反馈，完成后清理，不提交。[Python 工具](../ble-bench-python/README.md) 使用历史 v3；协议和蓝牙库也发生变化，不能把两者对照解释为仅改变语言的实验。
