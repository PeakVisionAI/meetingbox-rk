# BossAI-ASR 语音组件复用

## 当前适配范围

本适配使用 BossAI-ASR 的独立 Python FunASR 服务，通过 HTTP 复用文件识别能力。meetingbox 保留 Rust 音频采集、混音、重采样、Silero VAD、Tauri 事件、SQLite 和录音保存。无需启动 BossAI 的 Node 服务、Web 前端、账号或会议 Agent。

已适配的路径：实时录音产生的语音片段、音频文件导入、会后重新转写。实时路径仍然是 VAD 切句后识别，返回完整句子；本次没有实现边说边更新的 Paraformer 流式字幕，也没有引入唤醒词或 TTS。

事实依据：BossAI-ASR `main` 提交 `3fd94abdd`，`asr_server/server.py` 的 `/health`、`/asr/transcribe-file`；meetingbox 基线 `b3a3bda`。外部接口和依赖升级后应重新做兼容验证。

## 复用方式

`audio/transcription/bossai_provider.rs` 实现现有 `TranscriptionProvider`。`RkAsrProvider::local()` 根据部署环境选择内部实现，因而现有 engine、import、retranscription 调用点统一生效。

| 环境变量 | 默认值 | 含义 |
| --- | --- | --- |
| `MEETINGBOX_ASR_BACKEND` | `rk` | `rk` 使用现有 RK 服务；`bossai` 使用 FunASR 文件接口；其他值返回配置错误 |
| `MEETINGBOX_BOSSAI_ASR_URL` | `http://127.0.0.1:8002` | BossAI sidecar 根地址；允许 HTTP(S)，禁止 URL 中嵌入凭据、query 或 fragment |

仍在应用设置中选择 **RK ASR**。该选项是现有设备侧服务入口，环境变量指定其实际后端；本次不增加界面选项或数据库迁移。启动日志会记录实际使用的 `BossAI FunASR`。环境变量需在启动 Tauri 进程前设置，修改后重启应用；本项目不自动加载 `.env`。

```bash
cd /path/to/meetingbox-rk/frontend
MEETINGBOX_ASR_BACKEND=bossai \
MEETINGBOX_BOSSAI_ASR_URL=http://127.0.0.1:8002 \
pnpm tauri:dev:cpu
```

生产进程也应设置相同环境变量。恢复 RK 路径时设置 `MEETINGBOX_ASR_BACKEND=rk` 后重启。

请求契约：

```text
GET /health
  必须 ok=true、backend=funasr 且 fileLoaded=true；不能仅以 ready/loaded 判断文件模型。

POST /asr/transcribe-file
  multipart audio: 16 kHz、mono、PCM16 WAV
  multipart language: 应用语言偏好，未指定则 auto
```

响应使用 `segments[].text` 拼接；没有 segment 时才回退 `text`。BossAI 顶层 `text` 已带文件内时间戳及 Speaker 标签，正常路径不再重复显示。结果无可信度字段，返回 `confidence=None`；当前调用是完整片段，返回 `is_partial=false`。

## 已做的适配和优化

- 复用现有 PCM16 WAV 编码器和 HTTP 客户端连接池，不增加 Rust 依赖。
- 模型就绪检查使用 5 秒超时，必须等待文件模型加载；识别请求有 5 秒连接和 180 秒总超时。
- 允许 100ms 以上的短语音进入 BossAI，避免复用 RK 的 1 秒限制而遗漏短句；既有导入路径可能仍有 RK 补零。
- 不调用 RK 的音频增益和首尾 100ms padding，避免改变时间轴及重复处理；拒绝 NaN/Inf 输入。
- 响应读取限制为 1 MiB，HTTP 错误仅呈现状态码，不复制 sidecar 中可能含会议正文的错误内容；不跟随重定向。
- 保留现有本地热词替换规则。BossAI 文件接口没有识别级热词参数，无法把 RK 的 `hotwords` 或 `skip_vad` 直接传过去。
- BossAI 后端停止录音时清理 RK 本地会话状态，跳过不存在的 `/v1/asr/finalize-meeting`。
- 停止录音等待超过 45 秒时，取消 dispatcher 和全部 worker，并等待它们退出，防止迟到的转写结果进入下一场会议；提示用户通过原始录音重新转写。
- 不将每次请求重新生成的 Speaker A/B 拼成整场会议身份，不覆盖 meetingbox 原有播放时间戳。

## sidecar 部署

优先使用 BossAI 已准备并校验的 CPU Python 环境、模型缓存、平台 wheelhouse 和锁文件。部署细节见其 `docs/local-model-components.md`、`docs/asr-realtime.md` 和 `docs/local-components-hardware-matrix.md`。基础依赖在 `asr_server/requirements.txt`；生产环境应按对应平台的锁文件安装，而不是直接安装不固定版本的依赖。

以下示例假设 Python 环境和模型缓存已准备好；替换绝对路径与环境 ID：

```bash
env \
  PYTHONNOUSERSITE=1 \
  ASR_PORT=8002 \
  BOSSAI_ASR_MODEL_DIR=/absolute/data/models/voice \
  MODELSCOPE_CACHE=/absolute/data/models/voice/modelscope \
  FUNASR_HUB=ms FUNASR_DEVICE=cpu \
  FUNASR_MODEL=paraformer-zh-streaming \
  FUNASR_FILE_MODEL=iic/SenseVoiceSmall \
  FUNASR_MODE=streaming FUNASR_ENABLE_SPEAKER=0 \
  BOSSAI_ASR_PRELOAD=1 BOSSAI_ASR_PRELOAD_FILE=1 \
  BOSSAI_VOICE_FRONT=0 BOSSAI_VOICE_TTS=0 \
  OMP_NUM_THREADS=4 MKL_NUM_THREADS=4 NUMEXPR_NUM_THREADS=4 \
  TOKENIZERS_PARALLELISM=false \
  /absolute/data/components/asr/envs/<environmentId>/bin/python \
  /absolute/BossAI-ASR/asr_server/server.py
```

关闭 voice front/TTS 可避免加载本次不用的模型。主 streaming 引擎仍会按 sidecar 现有设计预加载；文件引擎始终启用 speaker，`FUNASR_ENABLE_SPEAKER=0` 不能关闭它。这些额外模型的内存消耗需在设备上测量。

服务入口默认监听 loopback，接口没有认证。建议与 meetingbox 部署在同机；若配置远端地址，应在实际部署中提供受控网络或认证网关，不能把该服务直接暴露到公网。

```bash
curl --fail http://127.0.0.1:8002/health
curl --fail http://127.0.0.1:8002/asr/transcribe-file \
  -F 'audio=@sample.wav;type=audio/wav' -F 'language=auto'
```

健康检查通过不代表真实模型识别质量或会议链路通过。

## RK3588 与说话人限制

BossAI 当前服务使用 FunASR/PyTorch CPU，不能直接获得 RK3588 NPU 加速。原 RKNN 服务仍是默认路径。BossAI 的 ARM64 文档有模型级运行记录，也有整套回放因内存/Swap 压力未完成的记录；本适配不把它当作 meetingbox 在 RK3588 上的端到端通过证据。

独立 WAV 请求中的 Speaker A/B 只在该请求内有效。当前适配输出正文，保留录音和时间轴，不提供 BossAI 跨片段说话人身份或会后声纹校准。要复用文件侧说话人结果，应下一步增加一次完整会议的离线识别或独立声纹聚类，测量其内存、时长和准确度，再定义替换数据库逐字稿的策略。

现有 meetingbox 转写队列没有容量上限；BossAI 若长期慢于实时，队列会增长。当前适配不宣称解决长会积压；应实测 RTF，并在资源不足时继续使用 RK 后端。超时取消 HTTP 请求也不保证 Python 推理马上终止，不能自动重试造成重复推理。

## 可继续复用的语音组件

| 组件 | 接口/实现 | 接入前必须处理 |
| --- | --- | --- |
| Paraformer 流式 ASR | `/asr/transcribe` | session_id、chunk_index、累计正文去重、final/autoFinal 和停止时 flush |
| Silero VAD/唤醒/打断 | `/voice/feed`、`/voice/reset` | 200ms 音频块、seq、延迟返回事件和停止后的结果收尾；避免双重 VAD |
| Piper TTS | `/voice/tts/synthesize` | 输出 22050Hz WAV、播放路由及音色授权；当前会议录音无需 TTS |
| 整场说话人 | `/asr/transcribe-file` 的 speaker segments | 单次整场推理、可空时间戳、内存边界、不能把不同请求标签直接合并 |

BossAI 没有可直接替代 meetingbox 原生混音/采集的服务器端 AEC 和降噪模块，其浏览器采集依赖浏览器自身处理。

## 验证

Rust 测试覆盖健康状态、实际 multipart 字段、短语音 WAV、语言、segment 正文、HTTP/格式错误、非有限输入、默认/显式后端选择、BossAI finalize 的会话清理和取消后迟到结果的丢弃。可在不构建 Tauri/Whisper 的环境直接编译生产 adapter 模块并运行协议测试：

```bash
cargo test --locked --manifest-path tests/voice-adapter/Cargo.toml
```

该独立测试包固定了测试环境依赖，不改变应用主工作区依赖；它不编译 Tauri worker/command 的完整装配。完整应用环境可运行：

```bash
cargo test -p meetily audio::transcription
cargo check --locked -p meetily
```

2026-10-14 本机验证：独立适配测试 20 项通过；变更中的 Rust 模块已做语法解析和 diff 检查。完整应用 `cargo check --locked --offline -p meetily` 未通过，原因是本机缺少 `ffmpeg-sidecar` Git 依赖缓存；磁盘空间不足，未继续获取完整 Tauri/Whisper 原生构建环境。未运行真实模型、桌面录音或 RK3588 验收。

目标 RK 设备还须测试真实麦克风、静音、短句、低音量、两人交替发言、导入、重新转写及不少于 10 分钟会议。记录识别错误率、首句延迟、RTF、RSS/Swap、队列积压、停止耗时及原始录音完整性。当前实现和协议测试不能替代模型或 RK 硬件验收。

## 来源及授权

本变更新写 Rust HTTP 适配，不复制 BossAI 语音源码、模型权重或客户配置。BossAI-ASR 未发现根源码许可证；实际分发其服务或模型时，需确认相应授权，不能依据 meetingbox 的 MIT 许可证推定。Piper 中文音色清单标为 `NOASSERTION`，本次不打包。

- https://github.com/PeakVisionAI/BossAI-ASR
- https://github.com/PeakVisionAI/BossAI-ASR/blob/main/docs/asr-realtime.md
- https://github.com/PeakVisionAI/BossAI-ASR/blob/main/docs/local-model-components.md
- https://github.com/PeakVisionAI/BossAI-ASR/blob/main/docs/local-components-hardware-matrix.md
