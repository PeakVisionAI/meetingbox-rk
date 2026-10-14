# Linux 语音唤醒部署

语音唤醒采用 BossAI VoiceFront KWS/VAD，待机时监听物理麦克风，默认唤醒词“小峰小峰”。唤醒只显示提示；识别到完整“开始会议”或“开始录音”才进入既有开始会议流程。不支持免唤醒自动录音、会议中的语音停止或系统电源唤醒。

组件与可移植启动方式见 [services/voice-front/README.md](../services/voice-front/README.md)。`meetingbox-voice.service` 是 edgebox 上 0.4.2 的实例配置，其他设备须替换用户、Python 环境和模型目录。Python 环境只安装 KWS/VAD/HTTP 所需依赖，不加载整套 BossAI PyTorch/FunASR，指令识别使用已有 8020 服务。

桌面启用后由 native supervisor 管理 parec；每块 200 ms 的 16 kHz mono PCM16 WAV 只在内存中传递，HTTP 不重定向。待机监听不保存音频。短指令到已有 ASR 时，该 ASR 会创建并在请求完成后删除临时 WAV；meeting_id=0 不加入会议音频累积。监听进程在录音开始前取消并 kill/wait，录音、暂停、停止整理、导入和重新转写期间保持暂停；错误后限速恢复，不把部分识别文本当作指令。

界面显示服务状态与监听开关，关闭选项保存在桌面存储。应用启动默认由 `MEETINGBOX_WAKE_ENABLED=1` 控制；未配置的其他部署默认关闭。

验收分开记录：

- 回归：36 项 Rust adapter/policy 测试、28 项 voice HTTP/ASR 边界测试、14 项 RK 服务测试。
- 真实模型：录制的 wake_only.wav 通过真实 KWS/VAD 产生 wake；录制唤醒词加本地合成“开始会议”通过真实 KWS/ASR 产生完整 utterance。
- 桌面：构建后检查 ARM64 包、运行进程、物理 source 采集子进程和实际窗口；单独记录桌面事件链的测试结果。
- 现场：远场距离、环境噪音、真人发音和误唤醒率需在盒子前验证。离线测试音频通过不能替代这些测量。

本次 KWS/VAD 是 CPU ONNX。盒子 ASR 的实际 effective_backend 按已有服务健康报告为准，不把配置中的 NPU 字段当作实际加速结果。
