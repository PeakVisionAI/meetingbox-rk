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
# 1) 语言运行时（只用标准库 + fastapi/httpx/uvicorn）
python3 -m venv /opt/meetingbox-rk/venv
/opt/meetingbox-rk/venv/bin/pip install fastapi uvicorn httpx pydantic

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

## 4. 已知缺口

- **说话人标签**：Intel 版依赖一个 torch 说话人模型（外部包
  `app.services.realtime.speakers`），RK 侧没有等价实现。当前
  `rknn_stream` 明确返回 `speaker: null`，`/healthz` 报 `diarization: unavailable`。
  逐字稿仍可用，只是不区分人。要补齐需要单独上一个小声纹模型（CPU 或 NPU）。
- **标点与字词对齐**：Intel 的 `audio-front` / `overlap-openvino` 依赖 OpenVINO（x86），
  RK 上不部署。SenseVoice 自带标点，逐字稿可读；但不提供词级时间戳。
- **`/v1/asr/finalize-meeting`**：桥接不实现该端点，RK 服务会返回
  `speaker_calibration: false`。应用侧按“校准不可用”处理，不影响落库。
