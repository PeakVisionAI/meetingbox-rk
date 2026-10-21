# MeetingBox 35B · Intel 本地会议初版

2026-10-21 保存的初版源码快照，基于 PeakVisionAI/meetingbox-rk（上游提交 `b3a3bda`）。保留上游许可证与第三方声明；上游介绍见 [README.upstream.md](README.upstream.md)。

## 当前链路

- Windows Tauri 桌面前端；Ubuntu VMware 虚拟机运行会议桥接服务。
- FireRedASR2-AED INT8 / sherpa-onnx CPU，滚动音频识别；Qwen3 ForcedAligner 字词对齐，标点和说话人、重叠检测独立服务。
- Qwen3.5-35B-A3B Q4_K_M，llama.cpp Vulkan，关闭长思考；录音时生成简短分时摘要，结束后归纳正式纪要。
- 快速会议优先准备语音服务，35B 后台预热。启动页显示实际准备阶段、等待秒数和语音服务就绪数。
- 每次生成最终纪要均经过资源租约、录音状态检查、语音服务卸载与内存预检，完成后释放租约；下次开会恢复语音服务。
- 转写按说话人和完整句组合成段，支持标点、实时字幕、录音播放与导出。

## 文件与部署

- `frontend/`：桌面界面与 Rust 主程序。
- `services/asrserve-bridge/`：虚拟机桥接与音频流处理。
- `services/qwen-openvino/qwen_ov_server.py`：当前已部署 FireRed 服务，文件名保留历史兼容。
- `services/qwen-openvino/summary_35b_server.py`：当前已部署 35B 纪要适配器，运行时命名为 `summary_ov_server.py`。
- `services/audio-front/`、`services/overlap-openvino/`：音频前处理、标点及重叠服务。
- `deploy/intel/`：现场固定路径及网段的部署脚本；`native/` 为本次从目标机读取的启动脚本。

该仓库包含源码与部署配置；模型权重、Python 虚拟环境、llama.cpp/ffmpeg 可执行文件、VMware 虚拟机、录音、会议数据库、账号密码均需独立提供。部署路径目前为 `D:\MeetingBox-Intel`，虚拟机 `192.168.44.50`、宿主机网卡 `192.168.44.1`。更换环境需同步修改配置；旧部署说明中的 8B/Qwen ASR 内容属于历史路线，以本文与初版记录为准。

初始化上游依赖使用 `git submodule update --init --recursive`。子模块来源记录在 `.gitmodules`；frontend 安装依赖后执行 `npm run build`，Windows 构建参考 `deploy/intel/build-windows.ps1`，需要 Rust、MSVC、libclang 与 Tauri 工具链。

## 验收边界

已在 Intel 现场主程序验证真实录音、转写、录音保存、返回文件列表再返回首页后重新开会，以及冷加载状态可见。长会话曾验证分时摘要与最终纪要链路。测试时间、样本和边界见 [初版记录](docs/INITIAL_RELEASE_20261021.md)。这份快照未在全新机器上完成一键部署验证，也未据此声明统一 CER/DER 指标。
