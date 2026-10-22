# RK 上的 Intel 功能对照

目标：同一套功能，只换模型。下表逐项说明 Intel 功能的依赖、RK 的落地方式与当前状态。

图例：**等价** = 行为一致；**替代** = 换实现，能力接近；**降级** = 能力减少；**缺口** = 尚未实现。

## 1. 会议链路

| 功能 | Intel 版依赖 | RK 方案 | 状态 |
| --- | --- | --- | --- |
| 实时字幕（增量 partial） | sherpa-onnx 流式 zipformer（CPU） | `rknn_stream` 滑窗重解码 SenseVoice（NPU） | 替代，已实现，需实机调参 |
| 断句 / 整句合并 | sherpa endpoint detection | 尾静音阈值（默认 700ms）+ 20s 强制切句 | 替代 |
| 会后整段校准 | `/v1/asr/finalize-meeting` + 声纹聚类 | RK 服务返回 `speaker_calibration:false` | 降级 |
| 说话人标签 | torch 声纹模型（外部包 `app.services.realtime.speakers`） | 无 | **缺口** |
| 标点 | FireRedPunc（OpenVINO，x86） | SenseVoice 自带标点 | 降级（不部署独立标点服务） |
| 词级时间戳 | Qwen3 ForcedAligner（OpenVINO） | 无 | **缺口** |
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

## 4. 两个真实缺口

1. **说话人区分**。Intel 的分人依赖一个 torch 声纹模型，它甚至不在本仓库里
   （`app.services.realtime.speakers`）。RK 侧当前不提供，逐字稿只有文本。
   补齐需要额外引入一个轻声纹模型（CAM++ / 3D-Speaker 的 onnx 版本，CPU 可跑），
   或者在 RK NPU 上做量化部署。这是唯一影响“看起来一样”的差距。
2. **词级时间戳**。Intel 用 Qwen3 对齐器产出逐词时间，用于说话人轮次切分和原文引用定位。
   SenseVoice 不是对齐模型，RK 侧只有句级时间。

两项都不影响录音、转写、摘要、导出、知识中心这些主流程。

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
