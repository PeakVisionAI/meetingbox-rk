# RK3588 NPU 优先语音适配

## 当前交付边界

meetingbox 已有设备侧 RK HTTP 客户端；本次增加严格的 `rk-npu` 路由策略，优先请求 SenseVoice RKNN，故障时可显式选择 BossAI FunASR CPU 回退。实时语音片段、导入和重新转写复用同一入口。原有 `rk` 和 `bossai` 模式保持兼容，不自动替换默认部署。

这是外部推理服务的客户端适配，不包含 RKNN 模型转换、权重、RKNN runtime 或推理服务端。当前工作区未找到对应服务包、权重和目标机连接资料；现有文档中的 SenseVoice RKNN 运行记录是 2026-08-12 的历史记录。本次不能证明当前设备的 NPU 已运行。

## 配置

在启动桌面进程前设置环境变量，设置界面继续使用 RK ASR：

```bash
export MEETINGBOX_ASR_BACKEND=rk-npu
export MEETINGBOX_RK_ASR_URL=http://127.0.0.1:8020
export MEETINGBOX_RK_ASR_TIMEOUT_SECONDS=90
export MEETINGBOX_ASR_FALLBACK=bossai
export MEETINGBOX_BOSSAI_ASR_URL=http://127.0.0.1:8002
cd frontend
pnpm tauri:dev:cpu
```

`tauri:dev:cpu` 指桌面应用内置 Whisper 的编译选项，不会禁止外部 RK 服务使用 NPU。两个推理服务须事先准备，应用不会下载模型或启动服务。

| 参数 | 默认值 | 说明 |
| --- | --- | --- |
| `MEETINGBOX_ASR_BACKEND` | `rk` | `rk` 为原有宽松协议；`rk-npu` 要求实际后端元数据；`bossai` 为独立 FunASR |
| `MEETINGBOX_RK_ASR_URL` | `http://127.0.0.1:8020` | RK 服务根 URL，无凭据/query/fragment，不跟随重定向 |
| `MEETINGBOX_RK_ASR_TIMEOUT_SECONDS` | `90` | 严格 NPU 模式单次上传总超时，1–180 秒 |
| `MEETINGBOX_ASR_FALLBACK` | `none` | `bossai` 开启 CPU 回退；`none` 遇错返回明确失败 |
| `MEETINGBOX_BOSSAI_ASR_URL` | `http://127.0.0.1:8002` | BossAI 文件识别服务；CPU 回退要求 `/health` 返回 `device=cpu`、`fileLoaded=true` |

开发/生产进程均需继承这些变量，修改后重启。可在 systemd 用户服务 `[Service]` 中配置对应 `Environment=`，应使用实际应用 ExecStart 和账号，不在仓库猜测设备路径。

## 严格 NPU 服务契约

旧 `/healthz` 只有 `ready` 不足以确认后端。严格模式新增要求如下，**外部服务需适配并返回实际状态**：

```json
{
  "ready": true,
  "backend": "sensevoice-rknn",
  "device": "npu",
  "model": "实际部署模型的版本标识"
}
```

`device` 接受 `npu`、`rknn`、`rk3588-npu`。`model` 可缺失，此时展示为未知，不用客户端常量推定版本。只有 ready、backend、device 满足要求才接受为服务报告的 NPU 就绪。

识别仍调用 `POST /v1/asr/transcribe-upload`，multipart 字段为 `audio`（16kHz mono PCM16 WAV）、`meeting_id`、`chunk_start_ms`、`hotwords`、`skip_vad`。严格模式的成功响应也必须声明每次请求的实际后端：

```json
{
  "backend": "sensevoice-rknn",
  "device": "npu",
  "model": "实际部署模型的版本标识",
  "segments": [{"text": "识别正文", "speaker": null}]
}
```

HTTP 200 并不表示 NPU 执行成功。服务如果内部回退到 Paraformer CPU，必须返回实际 backend/device；客户端会按显式策略处理，不能继续标记 NPU。

服务元数据本身也不能代替硬件证明。`npu_reported=true` 仅表示服务声称通过此协议，需用真实推理期间的 NPU 活动和驱动/runtime 证据核实。

## 失败、状态与会议数据

- 每次 NPU 识别前检查服务实际后端，健康检查总超时 5 秒。上传超时、连接失败、HTTP/JSON 错误和后端不符可触发一次 CPU 回退。
- 回退前检查 BossAI 文件模型就绪且设备报告为 CPU；不就绪则返回错误，不自动安装或下载模型。
- CPU 接收原始音频，避免 RK 增益/补零污染回退输入。短于 1 秒但不短于 100ms 的 RK 片段只在 RK 分支尾部补零。
- 同一 provider（实时录音或一次批处理）回退后保持 CPU，避免故障时反复请求 RK；新任务可重新尝试 NPU。客户端取消/超时不保证服务端计算立即停止，应控制外部服务并发。
- 录音、导入、重新转写互斥执行，避免覆盖共享会议 ID 或清空回退状态。NPU 模式停止等待预算为 RK 超时 +195 秒（默认 285 秒），覆盖一次 RK 等待、CPU 检查和 CPU 推理；超出后取消并等待 worker 退出，标记会话不完整。长队列仍可能无法在此预算内处理完，应使用保留的原始录音重新转写。
- Tauri 命令 `get_asr_runtime_status` 和事件 `asr-backend-status` 提供 `configured_backend`、`active_backend`、`model`、`device`、`fallback_reason` 和 `npu_reported`。实时路径通过现有 toast 提示切换到 CPU；导入和重新转写可查询状态，日志记录原因。
- 会议中只要用过 CPU 回退，就跳过 RK 会后说话人校准，保留已有逐字稿和时间轴，避免 RK 不完整片段集合覆盖 CPU 识别正文。停止结果返回明确的校准未完成原因。
- 静音返回空 segments 是合法结果，不触发回退。服务应使用正常空响应表达无语音；NPU 模式不把任意 fsmnvad/IndexError 静默当成无语音。

## 部署检查与硬件验收

先取得外部 RK 服务及匹配的 `.rknn` 权重、词表/前处理资产、模型许可证、SHA-256、runtime 和驱动版本。按现有部署手册安装服务；本仓库不提供缺失的服务实现或权重，也不通过新增 unit 伪造已安装状态。

只检查健康协议：

```bash
python3 scripts/check-rk-asr.py --url http://127.0.0.1:8020
```

显式提交一段 1–25 秒的 16kHz mono PCM16 WAV，并校验已安装模型：

```bash
python3 scripts/check-rk-asr.py \
  --url http://127.0.0.1:8020 \
  --audio /absolute/test.wav \
  --model /absolute/sensevoice.rknn \
  --sha256 实际模型的SHA256
```

脚本只输出协议元数据、耗时、RTF、片段数和模型哈希，不打印会议正文；上传测试会把所指定 WAV 发送到所指定服务。结果中的 `hardware_verified` 保持 false，脚本不能证明远端 NPU 硬件活动。

验收还需：

1. 保存目标 Ubuntu/aarch64、内核、NPU 驱动和 runtime 版本。
2. 在同一真实音频推理期间记录 NPU 活动及服务实际执行日志。
3. 测试 NPU 成功、服务掉线、识别超时、HTTP 200 内部 CPU 回退及 CPU 服务不可用。
4. 记录首句延迟、RTF、RSS/Swap、队列积压及模型输出质量，完成不少于 10 分钟会议。
5. 检查导入、重新转写、停止录音、原始录音完整性及混合后端时不覆盖逐字稿。

独立协议回归：

```bash
cargo test --locked --manifest-path tests/voice-adapter/Cargo.toml
```

2026-10-14 本机验证：独立 Rust 适配测试 33 项通过；部署脚本用本地 HTTP 模拟验证了正常响应与缺失 segments 的失败响应；Tauri 装配模块通过 Rust 语法解析，前端事件 hook 通过 TypeScript 语法转译（不等于完整类型检查）。默认本机 RK 服务检查未通过，未发送真实会议音频。完整 Tauri 构建、真实模型推理与 NPU 硬件验收均未完成。

完整 Tauri 构建和设备验收仍需具备项目原生依赖和硬件。BossAI CPU 部署步骤见 [BOSSAI_ASR_INTEGRATION.md](BOSSAI_ASR_INTEGRATION.md)，原 RK 参数基线见 [RK_DEPLOYMENT_ACCEPTANCE_BASELINE.md](RK_DEPLOYMENT_ACCEPTANCE_BASELINE.md)。
