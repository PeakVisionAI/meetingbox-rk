# 在 RK 上跑 Intel 的功能（只换模型）

目标：RK3588 设备上提供与 Intel 版**相同的产品功能**（实时字幕、服务就绪面板、
分时摘要、知识中心、逐段 DOCX、校对建议、按说话人合段导出），
差异只允许出现在**模型**这一层。

本目录是配套的部署说明；代码改动在：

- `services/asrserve-bridge/rknn_stream.py`：RKNN 流式引擎（新增）
- `services/asrserve-bridge/bridge.py`：`MEETINGBOX_PRIMARY_ASR=rknn` 分支
- `services/asrserve-bridge/deploy/meetingbox-bridge.service`：systemd 单元
- `frontend/src-tauri/config/profile-rk-bridge.json`：RK 硬件的 Intel 功能档案
- `docs/RK_FEATURE_PARITY.md`：逐项对照表与已知缺口

---

## 1. 拓扑

```
Windows / Linux 桌面端（同一份 Tauri 应用）
        │  MEETINGBOX_CONFIG=profile-rk-bridge.json
        │
        ├── 127.0.0.1:8030  asrserve-bridge（Python）─┬─ /v1/asr/stream/{id}
        │                                             └─ rknn_stream → 8020
        ├── 127.0.0.1:8020  rk-asr（SenseVoice RKNN, NPU）
        ├── 127.0.0.1:8040  RK1828 纪要 sidecar（OpenAI 兼容）
        └── 127.0.0.1:8069  knowledge-center（可选）
```

与 Intel 版唯一的差别：桥接内部的 ASR 引擎从 sherpa-onnx/FireRed 换成
`rknn_stream`，它调用本机 SenseVoice；其余协议、分段、导出、UI 完全一致。

---

## 2. 部署步骤

假设安装到 `/opt/meetingbox-rk`：

```bash
# 1) 语言运行时
python3 -m venv /opt/meetingbox-rk/venv
# python-multipart 是 FastAPI 表单端点的硬依赖，缺了 bridge 起不来
/opt/meetingbox-rk/venv/bin/pip install fastapi uvicorn httpx pydantic python-multipart
# 只在需要声纹/标点时才装（aarch64 有 manylinux wheel）
/opt/meetingbox-rk/venv/bin/pip install sherpa-onnx

# 2) 端侧 ASR 服务（NPU）
python3 -B services/rk-asr/prepare.py --output services/rk-asr/models
python3 -B services/rk-asr/build.py   --assets services/rk-asr/models
sudo install -m644 services/rk-asr/deploy/meetingbox-rk-asr.service /etc/systemd/system/
sudo systemctl enable --now meetingbox-rk-asr
curl -s http://127.0.0.1:8020/healthz | jq .ready   # 期望 true

# 3) 桥接（Intel 兼容 API + RKNN 引擎）
sudo install -m644 services/asrserve-bridge/deploy/meetingbox-bridge.service /etc/systemd/system/
sudo mkdir -p /var/lib/meetingbox/meeting-data
sudo chown -R meetingbox:meetingbox /var/lib/meetingbox
sudo systemctl enable --now meetingbox-bridge
curl -s http://127.0.0.1:8030/healthz | jq '.ready, .backend'
# 期望 true / "asrserve-rknn"

# 4) 桌面应用切到 Intel 功能档案
export MEETINGBOX_CONFIG=/opt/meetingbox-rk/frontend/src-tauri/config/profile-rk-bridge.json
# 或固化为 /etc/meetingbox/app.env 并由启动器注入
```

---

## 2.5 上板后的最小验证

构建 Tauri 应用之前，先用自带脚本把服务层跑通（纯标准库，不需要前端）：

```bash
cd /opt/meetingbox-rk/services/asrserve-bridge
python3 smoke_rk.py --health-only          # 两项服务是否就绪
python3 smoke_rk.py --tone 3               # 合成音：只验链路，不验识别
python3 smoke_rk.py --wav sample.wav       # 真实录音：16kHz 单声道 16bit
```

通过标准：`RESULT: PASS`，并且 `finals >= 1`；两人对话录音应看到多段、
`elapsed` 不超过音频时长的 3 倍。失败时会直接打印哪一项不满足。

我已在 macOS 上用桩服务验证过整条链路（真实 bridge + `rknn_stream` + 静音切句）：
3 秒单段 → `chunks=15 finals=1 partials=13`；两段中间隔 1 秒静音 →
`finals=2`，边界在 2000ms。**未验证的只有 NPU 推理本身。**

## 2.6 实测记录（192.168.20.187）

设备：RK3588 / Ubuntu 22.04.5 / 8 核 / 7.7 GB / Python 3.10.12。

**现场已有一套产品在跑**（`/opt/meeting-box`，systemd 单元 `asr/diar/llm/audio/boxd/...`），
端口占用与我们的档案冲突，因此测试实例改用：

| 用途 | 现场服务 | 我们的测试实例 |
| --- | --- | --- |
| ASR | `:8020`（asr.service，NPU） | 复用，不改 |
| 说话人 | `:8030`（diar.service，pyannote+eres2net） | 复用，不改 |
| 纪要 LLM | `:8040`（llm.service，RK1828） | 复用（当前 init 失败） |
| 桥接 | — | **`:8130`**（`/opt/meetingbox-rk`，venv + uvicorn） |

实测（`smoke_rk.py --wav`）：

| 样本 | 时长 | chunks | finals | 说话人 | 耗时 | RTF |
| --- | --- | --- | --- | --- | --- | --- |
| 单人中文 | 5.7 s | 29 | 2 | 发言人 1 | 2.78 s | 0.49 |
| 两人交替（Flo→Eddy→Flo） | 9.6 s | 49 | 3 | 发言人 1 / 2 / 1 | 3.70 s | 0.39 |

结论：**NPU 识别、设备侧分人、流式分段、标点兜底、句内时间在真机上全部工作**；
两人交替的说话人编号稳定，同一人跨句保持同一标签。

现场发现的两个既有问题（不属于本次改动）：

1. `asr.service` 的 Qwen3-ASR 后端缺 `ztu_somemodelruntime_ez_rknn_async` 模块，
   自动回退到 `sherpa-onnx-paraformer-zh-int8`（回退路径本身是正常的）；
2. `llm.service` 的 RK1828 报 `init_runtime failed: -1`，模型未加载。

资源提示：`meetingbox-voice`（唤醒词）常驻占 39% 单核并占用音频设备，它拉起的
`meetingbox-live-asr/server.py` 另占 1.5 GB；测试前停掉二者，可用内存从 3.3 GB 升到 5.0 GB。

## 2.7 实时余量实测（同一条 40 秒真人录音）

用 USB 会议麦（card 5 `PowerConf MN108`）实录 40 秒连续说话，跑四组参数：

| WINDOW | STRIDE | partial 数 | 耗时 | RTF | 字幕延迟 |
| --- | --- | --- | --- | --- | --- |
| 8000 | 600 | 23 | 36.4 s | 0.91 | ~0.6 s |
| 5000 | 900 | 29 | 35.0 s | 0.88 | ~0.9 s |
| **4000** | **2000** | **18** | **28.4 s** | **0.71** | **~2 s** |
| 4000 | 关 | 0 | 18.4 s | 0.46 | 无（句级） |

**0.46 是下限**：即使完全不生成 partial，设备 ASR 处理 40 秒连续语音也要 18 秒
（VAD + 推理本身），这部分不由桥接决定。partial 每多解码一次就叠加一次全流程开销。

结论与默认值：

- 默认取 **4 s 窗口 / 2 s 步长**，RTF 0.71，字幕延迟约 2 秒，留 30% 余量；
- 需要更低延迟就调小 STRIDE，但 0.9x 时已无余量，长会会积压；
- 不需要逐词上屏时把 STRIDE 设成 `600000`（等于关闭 partial），走句级字幕，RTF 0.46。

识别文本对照（真人「数数」）：`你好你好嗯觉得说现在一二三四五六七八九十…三十。`
数字全部正确，说明远场拾音与模型链路可用。

## 3. 引擎参数

`rknn_stream` 的默认值针对 RK3588 NPU 的 30 秒 SenseVoice 模型；都在 unit 里可调：

| 变量 | 默认 | 说明 |
| --- | --- | --- |
| `MEETINGBOX_RK_WINDOW_MS` | 8000 | 局部假设的重解码窗口 |
| `MEETINGBOX_RK_STRIDE_MS` | 600 | 最小解码间隔（实测耗时会自动压后） |
| `MEETINGBOX_RK_SILENCE_MS` | 700 | 判定一句话结束的尾静音 |
| `MEETINGBOX_RK_MAX_UTTERANCE_MS` | 20000 | 强制切句上限（模型上限 30s） |

调参顺序：先看 `rviz`/日志里的单次解码耗时（`elapsed`），保证
`STRIDE_MS > 单次耗时`；再调 `SILENCE_MS` 控制断句粒度。

---

## 4. 声纹、标点、词级时间

三样都在 `services/asrserve-bridge` 里，全部**可选**：模型不存在时自动降级，
不影响转写与落库。

### 4.1 说话人（已实现，需模型）

- 引擎：`diarization.py`，用 sherpa-onnx 的 `SpeakerEmbeddingExtractor`，
  和 SenseVoice 共用同一套 runtime，不额外装 PyTorch。
- 策略：每句（闭合 utterance）取一个 embedding，在线余弦聚类，
  阈值默认 0.62、最多 8 人；短于 0.8s 的句子不打标。
- 模型：sherpa-onnx 的 speaker-recognition release 里的中文通用模型，
  例如 `3dspeaker_speech_campplus_sv_zh-cn_16k-common.onnx`（以官方 release 页为准）。

```bash
mkdir -p /opt/meetingbox-rk/models
# 下载后放好路径，unit 里加：
#   Environment=MEETINGBOX_SPEAKER_MODEL=/opt/meetingbox-rk/models/3dspeaker_speech_campplus_sv_zh-cn_16k-common.onnx
#   Environment=MEETINGBOX_SPEAKER_THRESHOLD=0.62
#   Environment=MEETINGBOX_SPEAKER_MAX=8
curl -s localhost:8030/healthz | jq '.speakers'
```

验收：两人交替说话 2 分钟，逐字稿应出现 `发言人1/发言人2`，且同一人不跳号。
若串人就把阈值调高（0.70），若一人被拆成多个就把阈值调低（0.55）。

### 4.2 标点（已实现，可选模型）

- 首选：SenseVoice 自带标点，够用。
- 精修：`text_timing.py` 里的 `Punctuator`，调 sherpa-onnx CT-Transformer int8；
  release 里有 `sherpa-onnx-punct-ct-transformer-zh-en-vocab272727` 系列。
- 无论模型在不在，句尾都会被补全（`ensure_ending`），不会出现半句没有标点。

```bash
#   Environment=MEETINGBOX_PUNCT_MODEL=/opt/meetingbox-rk/models/punct-ct-transformer.int8.onnx
```

### 4.3 词级时间（已实现，无需模型）

Intel 用 Qwen3 对齐器产出逐词时间；RK 上不跑对齐模型，改成**句内按字符比例分配**
（`text_timing.utterance_timing`）：

- `clauses[]`：按标点切出的子句 + 起止毫秒，供 UI 高亮和“跳到这句”
- `token_times_ms[]`：逐字时间（同样按比例）

对中文、匀速讲话足够用于字幕高亮和导出；它不是强制对齐，
遇到长停顿或语速突变会有偏差。要更准有两条升级路：

1. 换 sherpa-onnx 的 Paraformer（自带 token 时间戳）做二次解码 —— 多一次推理，RK 上约 0.3–0.8×RTF；
2. 若所用 sherpa-onnx 版本对 SenseVoice 直接返回 token 时间戳，把
   `utterance_timing` 换成读识别结果即可，调用方不用改。

## 5. 已知缺口

- **说话人**：已按 A 方案补齐（sherpa-onnx 声纹 + 在线聚类），
  但**尚未在 RK 实机上验证准确率**；模型缺失时自动退回无标签。
- **词级时间**：改为句内比例分配，不是强制对齐；精度足够高亮，不足以做逐词引用。
- **`/v1/asr/finalize-meeting`**：桥接不实现该端点，RK 服务会返回
  `speaker_calibration: false`。应用侧按“校准不可用”处理，不影响落库。
