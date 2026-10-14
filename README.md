# 峰瞰密会智盒

峰瞰密会智盒（MeetingBox RK）是面向会议室设备的桌面会议助手。它在本地完成录音、实时转写、说话人处理、会议文件管理和会议纪要生成，适合部署在 Ubuntu 与 Rockchip RK3588/RK1828 设备上。

## 主要能力

- 快速开始会议并实时显示逐字稿。
- 使用语音活动检测减少静音阶段的识别请求。
- 实时阶段显示候选发言人；会议结束后根据声纹和时长进行说话人聚类。
- 每 10 分钟生成上一时段的会议分时摘要。
- 会议结束后生成完整会议纪要，并支持自定义总结模板。
- 在应用内查看会议文件、逐字稿、摘要和录音。
- 录音、转写、摘要和数据库默认保存在本机。
- 通过 Tauri 桌面应用运行，不依赖浏览器开发服务器作为生产入口。

## 技术架构

```text
Next.js / React
        │ Tauri commands and events
Rust / Tauri desktop core
        ├─ 音频采集、重采样和录音保存
        ├─ 实时 ASR 与会后完整转写
        ├─ 说话人分段、声纹聚类
        ├─ SQLite 会议数据库
        └─ 分时摘要与最终会议纪要
```

可通过可选的 BossAI FunASR 适配复用本地语音服务，覆盖实时语音片段、音频导入和重新转写；配置、组件边界及验证要求见 [BossAI-ASR 复用说明](docs/BOSSAI_ASR_INTEGRATION.md)。

RK 设备上的推理服务由部署配置决定。当前验收基线覆盖 RK3588 ASR、说话人处理和 RK1828 摘要服务，详见 [RK 部署与验收基线](docs/RK_DEPLOYMENT_ACCEPTANCE_BASELINE.md)。

## 仓库结构

```text
frontend/                  Next.js 前端与 Tauri 应用
frontend/src-tauri/        Rust 核心、数据库、音频和打包配置
frontend/src-tauri/scripts/许可证、设备指纹和交付脚本
llama-helper/              摘要引擎兼容 sidecar
docs/                      构建、架构、RK 部署和交付说明
backend/                   历史 FastAPI/Whisper 归档，不是当前生产入口
```

面向新设备的完整安装步骤、许可证、自启动和验收清单见 [HYOS 部署手册](docs/DEPLOYMENT.md)。部署时请将仓库中的桌面应用与设备侧 RK 推理服务、模型和 systemd unit 文件分开管理。

## 开发环境

建议在目标 Ubuntu/aarch64 设备或等效 Linux 环境构建正式包。开发前准备：

- Node.js 与 pnpm
- Rust、Cargo、CMake 和 C/C++ 编译工具链
- Tauri 2 所需的 WebKitGTK、GTK、AppIndicator 等系统依赖
- 目标设备对应的音频、显示和 Rockchip 驱动

完整系统依赖请参考 [HYOS 部署手册](docs/DEPLOYMENT.md)、[Linux 构建说明](docs/building_in_linux.md) 和 [通用构建说明](docs/BUILDING.md)。

安装前端依赖：

```bash
cd frontend
pnpm install
```

## 本地运行

开发模式：

```bash
cd frontend
pnpm tauri:dev:cpu
```

如果只需要检查前端页面：

```bash
cd frontend
pnpm dev
```

浏览器开发服务器只用于 UI 开发，不代表完整的桌面录音、Tauri bridge 或生产部署状态。

## 构建发布包

CPU 构建：

```bash
cd frontend
pnpm tauri:build:cpu
```

构建产物位于 `frontend/src-tauri/target/release/bundle/`。正式 RK 设备应在 Ubuntu/aarch64 实机上构建并验收，常用产物为 Debian 包或 AppImage。构建时会执行前端生产构建、Rust release 构建和 Tauri 打包。

构建前检查：

```bash
pnpm exec tsc --noEmit --pretty false
cd ..
cargo check --locked
```

## RK 部署

RK 设备部署需要同时确认应用和推理服务：

1. 确认 Ubuntu、内核、CPU 架构、NPU/PCIe 设备节点和音频设备。
2. 按部署基线启动 ASR、diarization 和摘要服务。
3. 安装 Tauri Debian 包或 AppImage。
4. 在实际 HDMI/桌面会话中验证开始会议、停止会议、实时转写、分时摘要、最终纪要和录音播放。
5. 验证服务开机自启、异常重启、日志和资源占用。

不要用浏览器页面或 Windows 构建结果替代 RK 实机验收。验收清单和模型回退顺序见 [RK_DEPLOYMENT_ACCEPTANCE_BASELINE.md](docs/RK_DEPLOYMENT_ACCEPTANCE_BASELINE.md)。

## 设备许可证

发布构建启动时会校验离线 Ed25519 许可证。许可证绑定目标设备的板卡序列号、CPU 序列号和系统标识；复制到另一台设备、篡改许可证或许可证过期时，应用会拒绝启动。

私钥只保存在开发机，不进入 Git、安装包或客户设备。交付流程：

```bash
# 目标设备获取指纹
sudo frontend/src-tauri/scripts/device-fingerprint.sh

# 开发机签发许可证
frontend/src-tauri/scripts/sign-license.sh \
  --device-id 'sha256:目标设备完整指纹' \
  --issued-at '2026-08-19T00:00:00Z' \
  --expires-at '2027-08-19T00:00:00Z' \
  --output ./license.json

# 目标设备安装应用与许可证
sudo frontend/src-tauri/scripts/prepare-device-release.sh \
  ./会议助手.deb ./license.json
```

详细说明见 [设备许可证与交付](docs/device-license.md)。交付前删除源码、`.git`、`target/`、调试日志和所有私钥。

发布加固构建可执行：

```bash
./scripts/build-release.sh
```

该脚本使用 Release 配置构建 Tauri 应用，关闭浏览器 source map，生成 `SHA256SUMS`，并审计交付目录中是否混入源码、依赖目录或调试文件。它不会加密客户端代码；前端和二进制仍应按“可被拥有 root 权限者逆向”设计。

## 数据与安全

- 首次启动由操作者设置管理员密码，仓库和发布包不包含默认管理员密码。
- 会议结束后再设置会议访问密码，会议密码为 6 位数字。
- 会议文件、录音、转写和摘要受本地访问控制保护。
- 生产设备应使用专用低权限账号运行服务，关闭不必要的 SSH 密码登录，并限制 `/etc/meetingbox`、应用数据和模型目录权限。
- 许可证能阻止普通复制和跨设备运行，但不能绝对阻止拥有 root 权限者逆向或修改二进制；设备系统加固和物理访问控制仍然必要。

## 验证与排障

前端类型检查：

```bash
cd frontend
pnpm exec tsc --noEmit --pretty false
```

Rust 检查：

```bash
cargo check --locked
```

设备状态和 NPU 监控由目标机上的 `rknn-smi` 提供：

```bash
rknn-smi info
rknn-smi info -w -d 0
rknn-smi info -t work_mode -d 0
```

`rknn-smi set -t work_mode -s 2 -d 0` 会切换到高性能模式并提高功耗与温度，只应在压力测试或明确的性能验收期间使用。

## 后续功能规划

以下能力作为后续迭代方向，优先级会根据实际会议场景、RK 设备资源和验收数据调整。

### 近期优先

- **可恢复会议**：应用异常退出或设备断电后，自动发现未完成会议，支持继续录音、补齐转写并重新生成摘要。
- **实时状态反馈**：显示录音、语音检测、ASR、说话人处理和摘要任务的独立状态；服务异常时提供重试和降级提示。
- **说话人管理**：会后支持修改“候选发言人”名称、合并或拆分说话人，并将常用声纹保存为联系人。
- **会议文件检索**：按标题、日期、说话人、关键词和摘要内容搜索，支持分页或虚拟列表，保证长时间使用仍流畅。
- **摘要可靠性**：分钟摘要失败时保留原始转写并支持单段重试；会议结束后支持重新生成完整纪要，不影响录音和转写数据。

### 中期增强

- **纪要模板中心**：提供项目例会、客户沟通、研发评审和访谈等模板，支持管理员导入、编辑、版本管理和导出。
- **结构化行动项**：从纪要中提取决策、待办、负责人、截止时间和风险，并支持勾选完成状态。
- **多格式导出**：支持 Markdown、DOCX、PDF、TXT 和带时间戳的字幕文件，导出时可选择是否包含录音链接和说话人信息。
- **音频与转写校对**：在同一页面联动播放录音、定位时间戳、修改转写文本，并保留原始内容和修订记录。
- **热词与领域词表**：按设备或会议类型维护热词、专有名词和替换规则，降低人名、产品名及行业术语的识别错误。

### 设备与运维

- **服务自愈**：ASR、说话人和摘要服务增加健康检查、自动重启、指数退避和任务断点恢复。
- **资源保护**：根据 CPU、内存、NPU 温度和频率动态调整并发与模型档位，避免高负载下卡死或 OOM。
- **设备管理**：增加设备状态页，集中查看版本、许可证、磁盘空间、温度、NPU 模式和最近错误日志。
- **签名更新**：提供带签名校验的离线升级包和回滚机制，升级失败时保留可启动的上一版本。
- **数据备份**：支持将会议数据库、录音和模板备份到指定移动存储，并提供校验、恢复和备份清理策略。

### 安全与协作

- **更细的权限控制**：区分管理员、会议主持人和只读查看者，分别控制设备设置、会议访问、导出和删除权限。
- **审计日志**：记录登录、会议密码设置、导出、删除、许可证校验和设备配置变更，支持按时间导出。
- **局域网协作**：在授权的局域网内提供只读会议查看或临时分享链接，并设置有效期和撤销能力。
- **隐私增强**：支持录音自动保留期限、敏感词脱敏和导出前隐私检查，满足不同组织的数据管理要求。

## 相关文档

- [系统架构](docs/architecture.md)
- [Linux 构建](docs/building_in_linux.md)
- [通用构建说明](docs/BUILDING.md)
- [RK 部署与验收基线](docs/RK_DEPLOYMENT_ACCEPTANCE_BASELINE.md)
- [设备许可证与交付](docs/device-license.md)

## 许可证

本仓库沿用项目原有许可证文件和第三方依赖许可证。发布和商业交付时，请同时遵守 Rust、Tauri、模型、字体和各推理服务依赖的许可证及其分发要求。
