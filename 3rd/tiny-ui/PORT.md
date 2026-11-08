# RP2350 移植说明

复制自 `/Users/fan/esp32p4/3rd/tiny-ui`，2026-11-08。
保留原始模块划分和 `no_std`、无堆分配、保留式 Scene、单个合并脏矩形语义。
原始项目未改动；源项目的开发约束复制到本目录 `AGENTS.md`。

本地变更：给 `FontSize` 增加 `Small`（6×10 ASCII），适配 240×135 屏幕。
业务图表/仪表盘位于 `dashboard/src/page.rs`；帧缓冲适配位于
`dashboard/src/framebuffer.rs`；SPI/DMA 和缓冲所有权传递在 `firmware/src/`。
不把 RP2350 引脚、USB、状态页面或固定分辨率加入通用框架。

双缓冲复用 `render_with_extra_damage`：保存渲染前的当前 damage，
下一次绘制以它修复旧 back buffer。合并后的绘制区域只用于 RAM 修复；
LCD 传输仍使用当前 damage。不能把合并结果反复当成 previous damage。
