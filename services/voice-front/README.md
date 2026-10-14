# BossAI 语音唤醒复用

复用 BossAI-ASR 的 VoiceFront、Zipformer KWS 和 Silero VAD。在 Linux 桌面待机时监听物理麦克风，默认说“小峰小峰”，听到提示后说“开始会议”或“开始录音”。只有完整识别的白名单指令才能触发既有会议开始流程。会议录音期间暂停唤醒监听；停止后恢复。暂不提供会议中的语音停止或屏幕电源唤醒。

`vendor/PROVENANCE.json` 记录未经修改的 BossAI 源码版本和 SHA-256。上游 checkout 未提供项目根源码许可证，不能把其中模型的 Apache/MIT 许可证当作 BossAI 源码许可证。后续公开分发前需确认该源码的授权条款。

## 服务

使用 Python 3.11+ 和 `requirements.txt` 中已在 ARM64 验证的依赖。无需再加载 FunASR、PyTorch 或免唤醒分类器。仅复制模型组中以下资产：

```
front/kws/sherpa-onnx-kws-zipformer-wenetspeech-3.3M-2024-01-01/
front/vad/silero_vad.onnx
```

KWS 的模型组来源声明为 Apache-2.0，Silero VAD 为 MIT；原始清单在 BossAI 的 `resources/model-groups/voice-front-models.json`。

```
BOSSAI_VOICE_FRONT_MODEL_DIR=/path/to/front \
MEETINGBOX_RK_ASR_URL=http://127.0.0.1:8020 \
/path/to/python server.py
```

HTTP 仅监听 `127.0.0.1:8021`，沿用 `/voice/health`、`/voice/feed` 和 `/voice/reset`。feed 使用 16 kHz 单声道 PCM16 WAV、session_id、递增 seq、mode=wake；建议每块 200 ms，上限 500 ms。持续喂入静音才能完成切句并取回异步转写结果。拒绝免唤醒模式，不加载 addressee。指令音频上限 10 秒，待识别音频上限 12 秒，上传上限 40 KB，最多两个监听会话。

短句识别通过盒子已有 `/v1/asr/transcribe-upload`，使用 meeting_id=0，不写入会议数据库。该服务的当前实际 ASR 后端由原服务决定；唤醒 KWS/VAD 使用 CPU ONNX，不声称 NPU 加速。

## 桌面

启动桌面时设置 `MEETINGBOX_WAKE_ENABLED=1`、`MEETINGBOX_VOICE_URL=http://127.0.0.1:8021`。`MEETINGBOX_WAKE_SOURCE` 可指定 PulseAudio 的物理麦克风 source。系统需安装 `parec`（Ubuntu 的 pulseaudio-utils）。桌面管理采集子进程，录音开始前关闭并等待其退出；禁用、失败、退出时也释放采集。待机音频不会落盘。界面可以关闭监听，状态会区分待机、等待指令、录音暂停和服务故障。

服务与桌面分别自启。仅服务自启不会采集麦克风；桌面启用监听后才创建采集进程。默认未配置环境变量的安装不启用监听。

RK3588 的 X11/WebKit 初始全屏表面可能停留在首帧，内部 DOM 和 IPC 已正常运行但屏幕仍显示加载页。`desktop-launch.py` 使用 xdotool，仅针对自身子进程的 HYOS 主窗口在启动后延迟 12 秒 unmap/map 以触发重绘；窗口恢复为普通窗口。盒子启动脚本已采用该兼容入口，不改动登录认证。其他显示栈不必使用此入口。
