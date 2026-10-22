# 统一平台：一份代码，Intel / RK 两种部署

本分支把原来的两条产品线合并成**一个 Tauri 应用 + 一个 Next.js 前端 + 两套可选后端服务**。
平台差异不再靠分叉代码，而是集中在**平台档案（platform profile）**里：启动时解析一次，
之后所有服务地址、路径、UI 能力都从它读取。

- 分支：`feat/unified-platform`
- 基座：`6f7177c`（含 RK NPU 改造的 RK 主线）＋ 并入 `226ca1d` 的 Intel 模块
- 编译目标不变：Windows x86_64（Intel）/ Ubuntu aarch64（RK3588/RK1828）

---

## 1. 架构

```
frontend/                 Next.js UI（一份，按能力开关渲染）
frontend/src-tauri/       Rust 核心（一份，按 profile 选传输与端点）
  config/profile-*.json   三个内嵌档案：intel / rk / dev
  src/platform/           档案解析（唯一知道“我是谁”的地方）
  src/audio/transcription/
    rk_asr_provider.rs    RKNN / 分块请求 / NPU 路由 + CPU 回退
    asrserve_stream.rs    Intel 桥接流式请求（同一 trait 下的另一条传输）
    worker.rs             两条传输共用的调度与落库
  src/local_services.rs   按档案探活，可选拉起现场脚本
  src/knowledge_center.rs 知识中心桥接（端点与路径来自档案）
services/
  rk-asr/                 RK：SenseVoice RKNN 服务（可复现部署）
  asrserve-bridge/        Intel：虚拟机桥接（FireRed 流式 + 对齐 + 说话人）
  qwen-openvino/          Intel：35B 纪要适配器
  audio-front/            Intel：音频前处理 / APM
  firered-onnx/ overlap-openvino/  Intel：单机替代与重叠检测
  knowledge-center/       两端共用：知识中心服务
deploy/intel/             Intel 现场脚本（compose、PowerShell）
vendor/                   Intel 用的 tauri-runtime-wry 补丁副本（默认不启用）
```

关键点：**ASR 只有一条 provider 链路**（`rk-asr`），差异只在传输方式。
`transcription_mode` / `streaming` 由档案决定，worker 内部按 `asrserve_stream::enabled()` 分流，
不需要在 UI 里新增“Intel 专用 provider”，也不需要两套 worker。

---

## 2. 档案解析顺序

```
MEETINGBOX_PROFILE (intel|rk|dev)
        ↓ 未设置
MEETINGBOX_CONFIG 指向的 JSON 文件（现场覆盖）
        ↓ 未设置
编译内置默认：Windows → intel；Linux/aarch64 → rk；其它 → dev
        ↓
逐字段环境变量覆盖（见 §4）
```

应用启动时解析一次并缓存（`OnceLock`），日志会打印实际生效的档案：

```
Platform profile: intel (Intel 双机（Windows 桌面 + Ubuntu 虚拟机服务）), asr=http://192.168.44.50:8020 stream=true summary=http://192.168.44.1:8061/v1
```

前端通过 Tauri 命令 `get_platform_profile` 拿到能力开关，不再靠 UA 猜平台。
内置档案有三个半：`profile-intel.json`、`profile-rk.json`（RK 默认，分块 ASR）、
`profile-dev.json`，以及 **`profile-rk-bridge.json`**——RK 硬件跑 Intel 那套服务栈，
功能与 Intel 对齐，只有模型不同（见 [RK 功能对照](RK_FEATURE_PARITY.md)）。
用 `MEETINGBOX_CONFIG` 指向该文件即可切换，不需要改代码。

---

## 3. 配置字段

`frontend/src-tauri/config/profile-<kind>.json`：

| 字段 | 含义 | intel 默认 | rk 默认 |
| --- | --- | --- | --- |
| `asr.endpoint` | ASR 服务根地址 | `http://192.168.44.50:8020` | `http://127.0.0.1:8020` |
| `asr.stream` | 是否走流式传输（`/v1/asr/stream`） | `true` | `false` |
| `asr.backend` | `rk` / `rk-npu` / `bossai` | `rk` | `rk-npu` |
| `summary.endpoint` | OpenAI 兼容根地址 | `…:8061/v1` | `127.0.0.1:8040/v1` |
| `summary.model` | 默认模型名 | `meetingbox-35b` | `rk1828-summary` |
| `summary.live_endpoint` | 分时摘要服务 | `…:8050` | `127.0.0.1:8050` |
| `services[]` | 录音前探活清单（`name`/`url`/`require_fast_realtime`） | 3 项 | 2 项 |
| `knowledge.enabled` | 是否暴露知识中心 | `true` | `true` |
| `knowledge.endpoint` | 知识服务 RPC | `127.0.0.1:8069` | `127.0.0.1:8069` |
| `knowledge.token_path` | 令牌文件（空=应用数据目录） | `D:\MeetingBox-Intel\…\service.token` | 空 |
| `knowledge.audio_cache` | 试听缓存根（空=应用数据目录） | `D:\MeetingBox-Intel\…\audio-cache` | 空 |
| `bootstrap_script` | 就绪门禁前拉起的脚本（仅 Windows 执行） | `…\Ensure-LocalServices.ps1` | `null` |
| `capabilities.*` | UI 能力开关 | 见下 | 见下 |

`capabilities`：

| 能力 | 作用 | intel | rk |
| --- | --- | --- | --- |
| `knowledge_center` | 顶栏“知识中心”入口 | ✓ | ✓ |
| `power_actions` | 重启/关机菜单 | ✗ | ✓ |
| `device_strip` | 首页设备状态条 | ✗（改用服务就绪） | ✓ |
| `service_readiness` | 开始会议前阻塞等待服务就绪 | ✓ | ✗ |

---

## 4. 环境变量（兼容旧部署）

| 变量 | 覆盖 |
| --- | --- |
| `MEETINGBOX_PROFILE` | 档案种类 |
| `MEETINGBOX_CONFIG` | 档案 JSON 路径 |
| `MEETINGBOX_ASR_URL` | `asr.endpoint` |
| `MEETINGBOX_ASR_STREAM` | `asr.stream`（`1`/`true`） |
| `MEETINGBOX_ASR_BACKEND` | `asr.backend`（RK 现场调试用） |
| `MEETINGBOX_SUMMARY_URL` / `_MODEL` | 摘要端点 / 模型 |
| `MEETINGBOX_KNOWLEDGE_ROOT` | 知识中心安装根（同时推导 token 与 audio-cache） |
| `MEETINGBOX_KNOWLEDGE_URL` | 知识服务 RPC 地址 |
| `MEETINGBOX_KNOWLEDGE_DISABLE` | 关闭知识中心 |

旧部署脚本里已有的变量继续有效，不需要一次性改现场。

---

## 5. 运行时差异矩阵

| 环节 | Intel 档案 | RK 档案 |
| --- | --- | --- |
| 音频窗口 | 200ms 窗口（`pipeline.rs` 按流式调整） | 2s 窗口 |
| ASR 调用 | HTTP 流式 `/v1/asr/stream/<meeting>` | 分块 `POST` + 会后 `finalize` |
| 会后收尾 | 等待桥接尾包（最长 1 小时） | 服务策略超时，超时标记可重转写 |
| 说话人 | 桥接返回 `speaker` + `upsert` | 会后声纹聚类 |
| 摘要 | 35B 适配器，`source-paragraphs` 模式 | RK1828 sidecar，分块 |
| 分时摘要 | 5 分钟自适应 + 增量上下文 | 同左（同一实现） |
| 知识中心 | 默认路径 `D:\MeetingBox-Intel\knowledge-center` | 应用数据目录，可用 `MEETINGBOX_KNOWLEDGE_ROOT` |
| 电源 | 顶栏“退出” | 顶栏“关机”（重启/关机） |
| 启动门禁 | 三项服务探活 + 自动拉起 | 不阻塞 |
| 音频播放 | Windows 上转 WAV 缓存 | 转 MP3 缓存 |

---

## 6. 构建

```bash
# 检查（TypeScript + cargo check）
./scripts/build-profile.sh rk --check
./scripts/build-profile.sh intel --check

# 打包
./scripts/build-profile.sh rk
./scripts/build-profile.sh intel
```

两个平台共用 `Cargo.toml`。唯一需要人工确认的是 tauri 运行时补丁：

```bash
# 仅 Intel 发布构建需要，且只做一次（会改 Cargo.toml 与 Cargo.lock）
./deploy/intel/enable-wry-patch.sh
```

没有运行该脚本时，Intel 构建使用 registry 上的 `tauri-runtime-wry`，
无边框窗口拖拽/缩放可能出现异常——这是原 Intel 版本自带的行为差异，
现在改成显式开关，避免 RK/Linux 构建被顺带改掉依赖。

---

## 7. 实机验证清单

**两端都要**

- [ ] 首次启动：日志出现正确的 `Platform profile:` 行
- [ ] 开始会议 → 实时字幕 → 结束会议 → 逐字稿落库 → 重新打开可见
- [ ] 导出“会议文字成果.md”：`完整转录稿.txt`、`会议纪要.md` 同时生成
- [ ] 分时摘要与最终纪要均生成；长会（>30 分钟）不截断
- [ ] 顶栏能力项与档案一致（Intel 无“关机”，RK 有）

**Intel（Windows）**

- [ ] 语音服务未启动时点“开始会议”→ 就绪进度可见，脚本被拉起，180s 后给出缺项提示
- [ ] 流式转写尾包完整（结束会议后最后一句不丢）
- [ ] 35B 纪要 URL/模型来自档案，未设置任何 `MEETINGBOX_*` 也能工作
- [ ] 知识中心：管理员密码 → 进入 → 返回后会议可正常开始
- [ ] 录音播放（WAV 缓存）

**RK（aarch64）**

- [ ] `MEETINGBOX_ASR_BACKEND=rk-npu` 生效，NPU 不可用时回退并弹提示
- [ ] 转写超时后标记“可重新转写”
- [ ] 关机/重启菜单（含二次确认与 Esc）
- [ ] `cargo check --locked` 通过（未启用 wry 补丁）

---

## 8. 已知风险 / 待确认

1. **`[patch.crates-io]` 与 `Cargo.lock`**：启用补丁会改依赖源，`--locked` 会报需要更新锁文件。
   这是把它做成脚本而不是默认写入的原因。
2. **llm_client 超时**：沿用 Intel 的 1800s（35B 冷启动需要），RK 侧 4B 模型也会等更久才报错。
   如需按档案区分，可后续加到 profile。
3. **纪要提示词**：采用 RK 版本（保留 MM:SS 依据、要求完整）。Intel 的“精简、去时间戳”版本
   作为模板 `standard_meeting` 的差异未合并；若 Intel 客户需要精简版，建议新增一个模板文件而不是改提示词。
4. **未在本机编译**：开发机为 macOS + rustc 1.63，低于 Tauri 2 要求的 1.77，
   且无 `frontend/node_modules`。本轮只做了 Rust 语法解析、脚本 `bash -n`、配置 JSON 校验，
   **类型检查与链接必须在 Intel 机和 RK 实机上各跑一次**。
5. **知识中心在 RK 上默认开启**：`knowledge.enabled=true`，但服务不存在时进入会提示
   “知识服务尚未安装”。若要默认隐藏，改档案为 `false`。
