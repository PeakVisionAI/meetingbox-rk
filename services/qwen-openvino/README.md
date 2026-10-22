# Intel 本地模型服务

当前 ASR 为 Qwen3-ASR-1.7B INT8（CPU），词级对齐为 Qwen3-ForcedAligner-0.6B（CPU）。总结为 Qwen3-8B INT4（Intel GPU）。模型文件单独保存在目标机器；源码包不含模型与密钥。

部署总结服务时，将 `summary_ov_server.py` 与 `grounded_report.py` 一起复制到 `D:\MeetingBox-Intel\asr-openvino`。启动脚本设置 `SUMMARY_MODEL_DIR` 后运行服务。Arc B390 上的编译缓存加载失败已通过关闭总结模型的磁盘编译缓存规避，旧缓存保留用于定位。

标准会议纪要使用有时间戳的原文行作为依据：模型选择行号，服务验证范围与事项类型，并按标准模板填写原文。未明确指定的负责人、截止时间和状态保留为未指定或未说明。引用会保留原文的口语表达。其他模板与校对接口沿用原有调用方式。

服务错误写入本地异常日志；日志不新增请求原文。开始会议前的服务检查、Windows 桌面启动方式保持原有入口。
