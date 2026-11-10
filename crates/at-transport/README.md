# at-transport

AT 驱动共享的 `no_std` 传输和时间接口，无型号命令、无 HAL 依赖。

调用方实现单调毫秒时钟、UART 波特率切换、有上限的输入排空、带截止时间的完整写入/flush、读取及延时。读取返回 0 表示超时，I/O 故障用关联 Error 类型返回。驱动独占 AT 阶段；调用方负责停止透传并处理背压。

`dobt01-driver` 与 `dobt8as-driver` 各自拥有型号专属解析器和配置流程。实现示例见 STM32G431 的 `AtPort`。
