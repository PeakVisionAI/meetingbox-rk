# RK 上的 Intel 功能对照

目标：同一套功能，只换模型。下表逐项说明 Intel 功能的依赖、RK 的落地方式与当前状态。

图例：**等价** = 行为一致；**替代** = 换实现，能力接近；**降级** = 能力减少；**缺口** = 尚未实现。

## 1. 会议链路

| 功能 | Intel 版依赖 | RK 方案 | 状态 |
| --- | --- | --- | --- |
| 实时字幕（增量 partial） | sherpa-onnx 流式 zipformer（CPU） | `rknn_stream` 滑窗重解码 SenseVoice（NPU） | 替代，已实现，需实机调参 |
| 断句 / 整句合并 | sherpa endpoint detection | 尾静音阈值（默认 700ms）+ 20s 强制切句 | 替代 |
| 会后整段校准 | `/v1/asr/finalize-meeting` + 声纹聚类 | RK 服务返回 `speaker_calibration:false` | 降级 |
| 说话人标签 | torch 声纹模型（外部包 `app.services.realtime.speakers`） | sherpa-onnx 声纹 embedding + 在线余弦聚类（`diarization.py`） | 替代，已实现，准确率待实测 |
| 标点 | FireRedPunc（OpenVINO，x86） | SenseVoice 自带 + 可选 sherpa-onnx CT-Transformer int8（`text_timing.py`） | 替代 |
| 词级时间戳 | Qwen3 ForcedAligner（OpenVINO） | 句内按字符比例分配，产出 `clauses[]` + `token_times_ms[]` | 替代（近似，非强制对齐） |
| 热词纠正 | 应用侧文本替换 | 同（`apply_hotword_corrections`） | 等价 |

## 2. 摘要与知识

| 功能 | Intel 版依赖 | RK 方案 | 状态 |
| --- | --- | --- | --- |
| 分时摘要 | 桥接 `MEETINGBOX_LLM_URL`（35B） | 同客户端，指向 RK1828 sidecar | 等价（换端点） |
| 最终纪要 | `summary_35b_server`（llama.cpp/Vulkan） | RK1828 sidecar，OpenAI 兼容 | 等价 |
| 增量上下文 `context_for_meeting` | 应用内 | 同 | 等价 |
| 知识中心 | Python(FastAPI) + Node(Pi) + 本地 LLM | 同代码，LLM 指向 RK1828 | 等价（需部署） |
| DOCX 导入/生成/打开 | python-docx | 同 | 等价 |
| 事件抽取与向量检索 | bge-small-zh（CPU 512 维） | 同（CPU 推理，aarch64 可跑） | 等价 |
| 转录校对建议 | `/v1/asr/correct-text`（桥接实现） | 同桥接，LLM 指向 RK1828 | 等价 |

## 3. 应用与运维

| 功能 | Intel 版 | RK 方案 | 状态 |
| --- | --- | --- | --- |
| 开始前服务就绪门禁 | 3 项探活 + PowerShell 拉起 | 3 项探活，systemd 自启（`bootstrap_script=null`） | 等价 |
| 逐字稿按说话人合段 | 应用内 | 同 | 等价（无标签时按句合段） |
| 导出 `完整转录稿.txt` / `会议纪要.md` | 应用内 | 同 | 等价 |
| 电源菜单 | 无（只有退出） | 重启/关机（RK 保留） | RK 额外 |
| 设备状态条 | 无 | 有（RK 保留） | RK 额外 |
| 许可证 / 设备绑定 | 应用内 | 同 | 等价 |

## 4. 三个能力现在的做法

1. **说话人**（原缺口，已按方案 A 补齐）
   `diarization.py`：每句取一个 sherpa-onnx 声纹 embedding，在线余弦聚类，
   阈值 0.62、上限 8 人、短句不打标。和 SenseVoice 共用同一套 sherpa-onnx runtime，
   不引入 PyTorch。
   局限：**尚未在 RK 实机验证**。会议室远场、串音、同一人不同音量都会影响阈值，
   上线前必须用真实录音调 `MEETINGBOX_SPEAKER_THRESHOLD`。

2. **标点**（原降级，现在可精修）
   SenseVoice 自带标点作为基线；装 CT-Transformer int8 后由 `Punctuator` 二次精修。
   模型缺失时仍然保证句尾有终止符，不会出现半句。

3. **词级时间**（原缺口，现在有近似解）
   `utterance_timing()` 按字符比例把整句时间摊到子句和每个字上，产出
   `clauses[]`（子句 + 起止 ms）和 `token_times_ms[]`。
   用于字幕高亮、导出时间戳、跳到某句都够用；它不是强制对齐，
   遇到长停顿或语速突变会偏。
   要更准的两条升级路：换 Paraformer 拿 token 时间戳（多一次推理），
   或等 sherpa-onnx 对 SenseVoice 直接暴露 token 时间戳后替换 `utterance_timing`。

## 5. 为什么不能直接复用 Intel 的服务栈

| 服务 | 依赖 | aarch64 可行性 |
| --- | --- | --- |
| `services/asrserve-bridge` | fastapi/httpx/numpy | ✅ 可跑（本方案复用） |
| `services/knowledge-center` | fastapi/httpx/node | ✅ 可跑 |
| `services/qwen-openvino` | OpenVINO（x86 专属） | ❌ 由 RK1828 sidecar 替代 |
| `services/overlap-openvino` | OpenVINO + sherpa | ❌ 不部署 |
| `services/audio-front` | torch + WebRTC APM | ❌ 不部署（RK 直接采集） |
| `services/firered-onnx` | onnxruntime | ⚠️ 可跑但 CPU 太慢，无意义 |

所以“只换模型”准确的说法是：**会议主链路只换模型；标点/对齐/说话人三条 Intel 专有推理
在 RK 上用模型自带能力替代或暂缺**。

## 6. 验收建议

先在设备上跑通这三条再谈功能对齐：

```bash
# 端侧 ASR 可用
curl -s localhost:8020/healthz | jq '.ready, .model'
# 桥接可用且引擎是 RKNN
curl -s localhost:8030/healthz | jq '.ready, .backend, .engine'
# 一段 30s 音频的端到端时延与准确率
```

再对照 `docs/UNIFIED_PLATFORM.md` §7 的 RK 清单逐项验收。
