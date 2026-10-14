# Meetily-RK 推理部署与验收基线

版本：V1.0  
基线日期：2026-08-12  
适用范围：Ubuntu（RK3588/aarch64）+ RK1828 双设备部署、Meetily-RK 推理适配与后续验收

2026-10-14 开发补充：新增可选 `rk-npu` 客户端策略，要求 SenseVoice RKNN 实际后端元数据，并支持显式 BossAI FunASR CPU 回退。另提供可复现的 [SenseVoice RKNN 服务部署包](../services/rk-asr/README.md)，含固定上游资产下载校验与 systemd 模板；新增服务没有说话人校准模型。这些新增能力尚未完成目标机验收，不替换本文既有默认链路和 Paraformer 回退基线；详见 [RK NPU 适配](RK_NPU_ADAPTATION.md)。

## 1. 基线原则

- Meetily 是产品界面、会议数据和工作流主体，RK3588/RK1828 提供可替换的推理服务。
- 正式部署与通过结论必须来自目标 RK3588 主机的 Ubuntu 实机。Windows 仅作为开发、单元测试和构建环境，不构成部署或运行验收。
- 软件首次启动直接进入主界面，不自动安装或下载 Parakeet 与 Meetily 内置摘要模型。
- 本文中的“默认”“回退”“备选”“候选”不可互换。变更默认链路、模型格式、核心参数或回退顺序前，必须更新本文并重新验收。
- 未提供的模型版本、参数量、文件校验和与内存实测结果均标记为待确认，不得在验收报告中推定为已满足。

## 2. 设备职责

| 设备 | 主要职责 | 默认部署 |
| --- | --- | --- |
| RK1828 | 分钟摘要、最终会议纪要 | Qwen3-4B-Instruct |
| RK3588 NPU | ASR 主线或备选主线 | 实验主配置 Qwen3-ASR-1.7B；产品备选 SenseVoice-small |
| RK3588 CPU | ASR 回退、语音分段、说话人聚类 | Paraformer-large INT8、Pyannote Segmentation 3.0 INT8、ERes2Net-base |

### 2.1 目标操作系统

| 项目 | 验收要求 |
| --- | --- |
| 操作系统 | Ubuntu 22.04.5 LTS；正式验收仍须保存实机 `/etc/os-release` |
| CPU 架构 | RK3588 `aarch64`；实机 `uname -m` 已确认 |
| 内核 | 实机为 `5.10.226`；正式验收保存 `uname -a` 完整输出，并确认 RK NPU、RK1828 PCIe、音频及显示驱动 |
| 服务管理 | 推理服务使用 Ubuntu `systemd` 管理，验证开机自启、异常重启、停止和状态查询 |
| 本地界面 | 在目标 Ubuntu 的实际 HDMI/桌面会话中启动 Meetily 并完成交互验收；不能用浏览器开发服务器代替 |
| 音频 | 使用目标 Ubuntu 的实际 ALSA/PulseAudio/PipeWire 环境完成麦克风采集与 16 kHz PCM 链路验收 |

### 2.2 目标机只读基线快照（2026-08-12）

- Ubuntu 22.04.5 LTS，Linux 5.10.226，`aarch64`，8 个逻辑 CPU。
- 内存 7.7 GiB，Swap 2.0 GiB；根分区 115 GiB，剩余约 81 GiB。
- PCIe 已枚举 RK1828：Rockchip `1d87:182a`；存在 `/dev/dri/renderD128` 和 `/dev/dri/renderD129`。
- `asr.service`、`diar.service`、`llm.service`、`summary.service` 均为 enabled/active。
- 监听端口：ASR `127.0.0.1:8020`、diar `127.0.0.1:8030`、LLM `127.0.0.1:8040`、summary `127.0.0.1:8050`。
- 当前实机 ASR 有效后端为 SenseVoice RKNN；Qwen3-ASR 尚未成为有效后端。后续切换必须按第 5 节重新验收回退链路。
- RK1828 Qwen3-4B、Pyannote、ERes2Net 服务健康检查已就绪；这只证明服务当时健康，不等同于 Meetily 端到端验收。

## 3. Ubuntu 构建依据

- 上游依据：[Meetily BUILDING.md](https://github.com/Zackriya-Solutions/meetily/blob/main/docs/BUILDING.md)。仓库中的实际脚本位于 `frontend/dev-gpu.sh` 与 `frontend/build-gpu.sh`，应从 `frontend/` 目录执行。
- Tauri 2 Linux 系统依赖以 [Tauri 官方 prerequisites](https://v2.tauri.app/start/prerequisites/) 为补充依据。上游 Meetily 文档只列出基础编译工具，不能替代 WebKitGTK、AppIndicator、OpenSSL 等 Tauri 系统依赖。
- RK 发行版的 ASR 与摘要由外部 RK 服务承担，因此 Meetily 自带 `llama-helper` 仅为兼容能力，Ubuntu 首次构建采用 CPU/no-feature 路径，不将 Vulkan/CUDA/ROCm 探测结果当作 RK NPU 接入。
- 目标机当前已有 `build-essential`、CMake 和 Git，但尚无 Node/pnpm、Rust/Cargo及 Tauri Linux 开发库。完成安装和 ARM64 构建前不得标记 Meetily 已部署。
- 生产构建产物只接受在 Ubuntu/aarch64 上生成的 Debian 包或 AppImage；上游文档示例中的 `amd64.AppImage` 文件名不适用于本目标机。

## 4. 模型与运行参数

### 4.1 摘要模型

| 模型 | 角色 | 部署位置 | 规模与格式 | 运行参数 | 状态 |
| --- | --- | --- | --- | --- | --- |
| Qwen3-4B-Instruct | 分钟摘要、最终会议纪要 | RK1828 | 约 4B；RKNN/RKLLM；权重约 2.4 GB | 上下文 4096；`top_k=1`；`top_p=0.9`；`temperature=0.3`；`repeat_penalty=1.1`；分钟摘要最多 384 tokens；最终总结最多 768 tokens | 默认 |
| Qwen3-8B-Instruct | 总结高配 | RK1828 | 约 8B；RKNN/RKLLM | 运行参数待实机确定 | 候选；未纳入默认部署，需验证约 5 GB 板载内存是否足够 |

### 4.2 ASR 模型

| 模型 | 角色 | 部署位置 | 模型文件/格式 | 运行参数 | 状态 |
| --- | --- | --- | --- | --- | --- |
| Qwen3-ASR-1.7B | ASR 主配置 | RK3588 NPU | 约 1.7B；`audio_encoder.rknn` + `language_model.rkllm` | 单次识别超时 90 秒；Worker 启动超时 300 秒；异常时自动回退 Paraformer | 实验主配置 |
| Paraformer-large 中文 INT8 | ASR 回退 | RK3588 CPU | `model.int8.onnx` + `tokens.txt` | 4 线程；`greedy_search`；16 kHz、16-bit PCM；支持中文、英文及部分方言 | 默认回退 |
| SenseVoice-small | ASR 备选主线 | RK3588 NPU | RKNN；参数量待确认 | 超时 300 秒；待命 RMS 阈值 250；最低语音比例 0.02；支持热词文件 | 产品备选主线 |

### 4.3 说话人处理模型

| 模型 | 角色 | 部署位置 | 模型文件/格式 | 运行参数 | 状态 |
| --- | --- | --- | --- | --- | --- |
| Pyannote Segmentation 3.0 INT8 | 说话人切换、语音分段 | RK3588 CPU | INT8 ONNX；参数量待确认 | 4 线程；最短语音 0.3 秒；最短静音 0.35 秒；FastClustering 距离阈值 0.75 | 默认分段模型 |
| ERes2Net-base | 声纹特征提取、发言人聚类 | RK3588 CPU | ONNX；模型约 38 MB；参数量待确认 | 4 线程；在线余弦相似度阈值 0.50（连续句粘滞 0.45）；可靠音频至少 2.0 秒；短于 2 秒不新建说话人；第三人需 ≥3 秒且更低相似度；最多 6 人 | 默认声纹模型 |
| CAM++ | 旧声纹回退 | RK3588 CPU | ONNX；输出向量 192 维 | 其他参数待确认 | 兼容回退；不作为默认声纹模型 |

## 5. 默认推理链路与回退顺序

1. Meetily 将会议音频交给 RK3588 ASR 服务。
2. 实验配置优先使用 Qwen3-ASR-1.7B；识别异常或超过 90 秒时自动回退 Paraformer-large INT8。
3. 产品部署可在单独验收后，将 SenseVoice-small 设为 ASR 主线；未完成验收前不得静默替换实验主配置。
4. RK3588 使用 Pyannote Segmentation 3.0 完成语音分段，使用 ERes2Net-base 生成声纹并聚类；CAM++只用于兼容回退。
5. Meetily 将转写文本发送至 RK1828 Qwen3-4B-Instruct，按 384/768 tokens 上限分别生成分钟摘要与最终会议纪要。
6. 任一 RK 服务不可用时，界面必须显示明确的不可用或已回退状态，不得自动下载 Meetily 内置模型替代。

## 6. 部署验收清单

### 6.1 启动与部署

- [ ] 验收报告保存 `/etc/os-release`、`uname -a`、`uname -m` 和关键驱动/设备节点信息；系统确认为目标 Ubuntu/aarch64。
- [ ] Meetily 启动后直接进入主界面，不出现 Parakeet/内置摘要模型安装引导。
- [ ] 启动期间未自动发起上述两个 Meetily 内置模型的下载或安装。
- [ ] RK3588 和 RK1828 的实际模型文件、版本、文件大小与 SHA-256 被记录在验收报告中。
- [ ] Qwen3-ASR Worker 在 300 秒内完成启动；SenseVoice 被选用时，其服务在 300 秒内完成启动。
- [ ] 日志能记录实际选中的推理后端、模型标识、耗时和回退原因，但不记录完整会议音频或不必要的会议正文。
- [ ] 相关 `systemd` 服务已启用并运行；模拟服务异常后能按配置恢复，Ubuntu 重启后服务和 Meetily 数据仍可用。
- [ ] 在 Ubuntu 实机 HDMI/桌面会话中完成主界面、设置、录音开始/停止及会议详情导航，不以 Windows 窗口或 `localhost` 页面替代。

### 6.2 参数一致性

- [ ] Qwen3-4B-Instruct 的上下文、采样参数和两类输出 token 上限与 4.1 节一致。
- [ ] Paraformer 输入为 16 kHz、16-bit PCM，使用 4 线程和 `greedy_search`。
- [ ] Pyannote 与 ERes2Net 的线程数、阈值、最短时长和最多说话人数与 4.3 节一致。
- [ ] 未将 Qwen3-8B-Instruct 或 CAM++误配为默认模型。

### 6.3 回退与故障

- [ ] 人为制造 Qwen3-ASR 错误，系统自动切换至 Paraformer，界面和日志均可识别此次回退。
- [ ] 人为制造 Qwen3-ASR 单次识别超过 90 秒，系统按相同规则回退 Paraformer。
- [ ] RK1828 摘要服务不可用时，会议原始录音与转写结果不丢失，并明确标记摘要失败/待重试。
- [ ] 说话人模型不可用时，不伪造说话人身份；允许保留无说话人标签的转写结果并明确降级状态。

### 6.4 端到端验收

- [ ] 使用不少于 10 分钟的真实会议样本完成录音、分段、转写、说话人聚类、分钟摘要和最终纪要全链路。
- [ ] 验收报告记录各阶段实际设备、模型、耗时、成功/回退状态和输出文件位置。
- [ ] 独立确认摘要上限：分钟摘要不超过 384 tokens，最终纪要不超过 768 tokens。
- [ ] 实验结果与“已部署”“已运行”“已通过端到端验收”三种结论分开记录，不以构建通过代替实机验收。
- [ ] 最终“通过”结论的全部运行证据来自目标 Ubuntu 实机；Windows 结果仅列入开发验证附录。

## 7. 待实机确认项

- Qwen3-8B-Instruct 在约 5 GB 板载内存条件下的峰值内存、稳定性和性能。
- SenseVoice-small、Pyannote Segmentation 3.0 INT8、ERes2Net-base 的精确模型版本与参数量。
- 全部模型文件的发布来源、版本号、文件大小和 SHA-256。
- 目标 Ubuntu 镜像来源、桌面会话类型与实际音频栈。
- ASR、说话人聚类和摘要的业务质量阈值；在形成带样本集的量化标准前，只能报告实测结果，不能宣称达到未定义的准确率指标。
