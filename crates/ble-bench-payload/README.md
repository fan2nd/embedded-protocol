# BLE 测速应用载荷

`no_std`、无堆分配，定义 START/QUERY/STATS/DONE 等测速载荷、确定性测试数据及可复现随机包长。通过 `ble-link-protocol` 发送，不增加链路包类型；不绑定板卡或主机蓝牙 API。

验证：`cargo test --locked -p ble-bench-payload`。
