# 历史 Python BLE 测速工具

保留 v3 诊断、ACK 调度与抗重放测试，使用 `ble-bench-protocol` 对应帧格式；不兼容当前 G431 v6 固件。历史报告中的更早版本命令仅记录当时实验，未保证与本脚本兼容。

在仓库根目录执行：

```sh
python3 -m venv .venv
.venv/bin/pip install -r apps/ble-bench-python/requirements.txt
.venv/bin/python apps/ble-bench-python/ble_bench.py --self-test
.venv/bin/python -m unittest discover -s apps/ble-bench-python
```

当前工具见 [Rust 上位机](../ble-bench-host/README.md)。
