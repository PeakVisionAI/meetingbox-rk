# HYOS 部署手册

版本：v0.1  
适用设备：Ubuntu 22.04（aarch64/ARM64），RK3588 + RK1828 会议设备  
应用名称：AI 智能会议助手（Tauri 桌面应用）

本文面向没有开发环境的全新设备，覆盖依赖安装、构建、许可证、推理服务、桌面自启动和验收。生产部署应优先使用在目标 ARM64 设备上构建的 Debian 包；不要把源码、Cargo 构建目录或开发依赖复制到交付设备。

## 1. 组件边界

| 目录/组件 | 职责 | 是否随应用包交付 |
| --- | --- | --- |
| frontend/src/ | Next.js/React 页面和交互 | 是（编译后进入 Tauri 资源） |
| frontend/src-tauri/src/ | Rust/Tauri 桌面核心、录音、解码、数据库、转写和摘要桥接 | 是（二进制） |
| frontend/src-tauri/scripts/ | 许可证、设备指纹、关机策略和交付辅助脚本 | 按交付流程选择 |
| llama-helper/ | 本地摘要 sidecar 的兼容实现 | 构建时按配置打包 |
| backend/ | 历史 FastAPI/Whisper 代码和开发工具 | 否，不是当前 RK 生产入口 |
| RK 推理服务 | ASR、说话人处理、分钟摘要和最终纪要 | 由设备镜像/服务包单独交付 |

当前默认的本地服务地址和职责见 [RK 部署与验收基线](./RK_DEPLOYMENT_ACCEPTANCE_BASELINE.md)。应用不会替代或自动安装这些 RK 服务和模型。

## 2. 目标机检查

在目标设备本地或通过 SSH 执行：

~~~bash
uname -m
cat /etc/os-release
free -h
df -h /
~~~

必须满足：

- uname -m 输出 aarch64；
- Ubuntu 22.04（建议 22.04.5 LTS）；
- 至少 8 GiB 内存、2 GiB swap 和 10 GiB 可用磁盘（构建机建议 30 GiB）；
- 有实际 HDMI/桌面会话和可用麦克风；
- 账号具有 sudo 权限。

若只安装预构建 .deb，目标机不需要 Node、Rust 或 Cargo；若要在目标机编译，则继续执行第 3 节。

## 3. 空白机器构建环境

### 3.1 系统依赖

~~~bash
sudo apt update
sudo DEBIAN_FRONTEND=noninteractive apt install -y \
  build-essential cmake pkg-config git curl ca-certificates patchelf ffmpeg \
  libssl-dev libasound2-dev \
  libgtk-3-dev libwebkit2gtk-4.1-dev libjavascriptcoregtk-4.1-dev \
  libayatana-appindicator3-dev librsvg2-dev \
  clang libclang-dev llvm-dev
~~~

如果系统镜像的软件源没有 WebKitGTK 4.1，可暂时使用 4.0 开发包进行诊断，但正式构建应补齐 4.1；Tauri 2 的 javascriptcore-rs-sys 和 webkit2gtk 需要 4.1。

### 3.2 Node.js 与 pnpm

以下示例使用 Node.js 20 ARM64。版本可升级，但应在发布记录中固定：

~~~bash
NODE_VERSION=20.19.4
mkdir -p ~/.local
curl -fL "https://nodejs.org/dist/v${NODE_VERSION}/node-v${NODE_VERSION}-linux-arm64.tar.xz" -o /tmp/node-arm64.tar.xz
tar -xJf /tmp/node-arm64.tar.xz -C ~/.local
mv ~/.local/node-v${NODE_VERSION}-linux-arm64 ~/.local/node-v${NODE_VERSION}
ln -sfn ~/.local/node-v${NODE_VERSION} ~/.local/node
export PATH="$HOME/.local/node/bin:$HOME/.local/npm/bin:$PATH"
npm config set prefix "$HOME/.local/npm"
npm install --global pnpm@9
~~~

### 3.3 Rust

~~~bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
. "$HOME/.cargo/env"
rustc --version
cargo --version
~~~

## 4. 获取代码并构建

不要在生产设备上直接使用未审查的工作区改动。使用已批准的分支或 tag：

~~~bash
git clone https://github.com/PeakVisionAI/meetingbox-rk.git
cd meetingbox-rk
git checkout <approved-branch-or-tag>
export PATH="$HOME/.local/node/bin:$HOME/.local/npm/bin:$HOME/.cargo/bin:$PATH"
~~~

安装依赖并做前端检查：

~~~bash
cd frontend
pnpm install --frozen-lockfile
pnpm exec tsc --noEmit --pretty false
~~~

如果 pnpm 报 LOCKFILE_CONFIG_MISMATCH（overrides 与锁文件不一致），先确认当前分支确实是批准版本，再执行：

~~~bash
pnpm install --no-frozen-lockfile
pnpm exec tsc --noEmit --pretty false
~~~

构建 ARM64 Release 包：

~~~bash
pnpm tauri build
~~~

或从仓库根目录执行带审计的发布构建：

~~~bash
./scripts/build-release.sh
~~~

产物位于：

~~~text
frontend/src-tauri/target/release/bundle/deb/
frontend/src-tauri/target/release/bundle/appimage/
frontend/src-tauri/target/release/bundle/SHA256SUMS
~~~

只把与目标架构匹配的 .deb 或 AppImage 交付出去。build.rs 会尝试准备打包所需的 FFmpeg；如果构建机无法访问 GitHub，应提前将对应 ARM64 FFmpeg 放入 frontend/src-tauri/binaries/，或确保系统已安装 /usr/bin/ffmpeg，并在发布记录中保存 SHA-256。

## 5. 许可证与安装

Release 构建默认要求离线许可证。许可证绑定设备硬件指纹，不可跨机器复制。

### 5.1 生成指纹

在目标机执行：

~~~bash
sudo frontend/src-tauri/scripts/device-fingerprint.sh
~~~

将完整的 sha256:... 输出交给授权方。私钥只能保留在开发机。

### 5.2 安装许可证和应用

授权方生成 license.json 后，将应用包与许可证复制到目标机，再执行：

~~~bash
sudo frontend/src-tauri/scripts/prepare-device-release.sh \
  ./meetingbox_<version>_arm64.deb ./license.json
~~~

该脚本会安装许可证、安装关机/重启/待机所需的 polkit 策略，并用 apt 安装 Debian 包。

也可以手动安装：

~~~bash
sudo apt install ./meetingbox_<version>_arm64.deb
sudo frontend/src-tauri/scripts/install-license.sh ./license.json
sudo frontend/src-tauri/scripts/install-power-policy.sh
~~~

许可证权限应保持为 root 所有，运行用户可读（通常为 640）。不要把私钥、源码、.git、target/ 或模型下载缓存放到客户设备。

## 6. RK 推理服务

应用安装成功不代表推理链路已就绪。设备镜像或服务包需要另外提供并启用：

~~~bash
sudo systemctl enable --now asr.service
sudo systemctl enable --now diar.service
sudo systemctl enable --now summary.service
sudo systemctl enable --now llm.service
~~~

检查状态和端口：

~~~bash
systemctl --no-pager --full status asr.service diar.service summary.service llm.service
ss -lntp | grep -E ':(8020|8030|8040|8050)\b'
~~~

服务、模型、端口和回退顺序必须与 [RK_DEPLOYMENT_ACCEPTANCE_BASELINE.md](./RK_DEPLOYMENT_ACCEPTANCE_BASELINE.md) 一致。模型权重不在本仓库，部署人员必须记录来源、版本、大小和 SHA-256。

## 7. 桌面启动与开机自启动

应用是桌面程序，不能作为 root systemd 服务启动。推荐在 ubuntu 图形会话中配置 XDG 自启动：

~~~bash
mkdir -p ~/.config/autostart
cat > ~/.config/autostart/hyos.desktop <<'EOF'
[Desktop Entry]
Type=Application
Name=AI 智能会议助手
Exec=/usr/bin/meetily
Terminal=false
X-GNOME-Autostart-enabled=true
EOF
~~~

tauri.conf.json 已配置启动窗口全屏。若 Debian 包安装后的可执行文件路径不同，用以下命令确认并修改 Exec：

~~~bash
command -v meetily
dpkg -L meetily | grep -E '/(meetily|HYOS)$'
~~~

首次登录后验证：

~~~bash
pgrep -a meetily
~~~

推理服务用 systemd，桌面应用用 XDG 自启动；不要把 GUI 应用塞进 system 服务，否则通常会因为没有 DISPLAY、D-Bus 或音频会话而启动失败。

## 8. 部署验收

### 8.1 基础检查

~~~bash
command -v meetily
sha256sum /path/to/meetingbox_<version>_arm64.deb
systemctl is-enabled asr.service diar.service summary.service llm.service
systemctl is-active asr.service diar.service summary.service llm.service
~~~

### 8.2 端到端检查

在实际 HDMI/桌面会话中完成：

1. 登录并进入首页；
2. 开始会议，确认麦克风有输入；
3. 检查录音、VAD、ASR、说话人和摘要状态；
4. 连续讲话和静音各测试一次，确认静音不会产生虚假转写；
5. 结束会议，设置 6 位会议密码；
6. 等待分时摘要和最终会议纪要生成；
7. 在会议文件管理中打开录音、逐字稿和纪要；
8. 重启设备，确认服务和应用能恢复；
9. 停止一个推理服务，确认界面显示失败/重试或降级提示，且录音和原始转写不丢失。

建议至少使用 10 分钟真实会议样本，并记录每阶段耗时、实际模型、服务日志和资源峰值。构建通过不等于端到端验收通过。

## 9. 常见问题

### could not connect to localhost

先检查四个推理服务是否 active、端口是否监听，再查看日志：

~~~bash
systemctl --no-pager -l status asr.service diar.service summary.service llm.service
journalctl -u asr.service -u diar.service -u summary.service -u llm.service -n 200 --no-pager
~~~

### 录音无法播放或“无法解码”

确认 ffmpeg 存在且可执行，并保留原始录音文件：

~~~bash
ffmpeg -version
ls -lh ~/Music/meetily-recordings ~/Documents/meetily-recordings 2>/dev/null
~~~

不要把扩展名强行改成 mp3；当前录音可能是 MP4 容器，播放和会后增强转写需要完整容器信息及 FFmpeg。

### Release 启动提示缺少许可证

确认 /etc/meetingbox/license.json 存在、权限正确，并重新获取当前设备指纹。许可证绑定硬件，不能从另一台机器直接复制。

### 构建时 Cargo/Git 下载失败

优先在可联网的 ARM64 构建机完成构建；离线构建需要预热 Cargo registry/git 缓存和 FFmpeg 二进制。不要为了绕过网络错误修改生产依赖版本或提交缓存目录。

## 10. 交付清单

- [ ] 已批准的 Git 分支/tag 和 commit SHA
- [ ] ARM64 .deb/AppImage 与 SHA256SUMS
- [ ] 目标设备指纹和对应许可证
- [ ] RK 推理服务 unit 文件、模型清单和校验和
- [ ] 应用/服务自启动验证记录
- [ ] 端到端录音、转写、说话人、摘要、播放和故障降级记录
- [ ] 已删除源码、.git、构建目录、私钥和调试日志
