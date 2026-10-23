# RK 真机验证记录（2026-10-23）

设备：`192.168.20.187`，RK3588 / Ubuntu 22.04.5 / aarch64 / 8 核 / 7.9 GB / Python 3.10.12。

本文记录统一分支在这台设备上从零到跑通的完整过程，以及**尚未解决**的问题。

---

## 1. 结论速览

| 环节 | 状态 |
| --- | --- |
| 服务层（bridge + rknn_stream + 设备 ASR/分人） | ✅ 已验证，见 §4 |
| 工具链安装（Rust / Node / pnpm） | ✅ |
| TypeScript 检查 | ✅ 0 错误 |
| aarch64 编译 + 打包 | ✅ deb 51 MB |
| 应用后端启动（DB / 模板 / 托盘） | ✅ |
| **应用前端** | ❌ 停在访问网关的加载态，见 §5 |

---

## 2. 工具链

| 组件 | 版本 | 备注 |
| --- | --- | --- |
| Rust | 1.99.1 | `static.rust-lang.org` 在国内基本不动，改用清华镜像 |
| Node | 20.20.2 | NodeSource |
| pnpm | 9.15.9 | `npm i -g pnpm@9`，registry 换 npmmirror |
| 系统库 | — | `libwebkit2gtk-4.1-dev` 原本已装；补 `clang`/`libclang-dev` |

两个坑：

1. **3 个依赖来自 github.com**（`silero-rs`、`ffmpeg-sidecar`、`cidre`）。`git http.version=HTTP/1.1`
   无效，必须配反代：
   ```bash
   git config --global url."https://ghproxy.net/https://github.com/".insteadOf "https://github.com/"
   ```
2. **rustup 首次安装被网络打断会留下半成品**（`missing manifest in toolchain`），
   需 `rustup toolchain uninstall stable` 后重装。

---

## 3. 构建

```bash
cargo fetch                       # 依赖 1.5 GB
pnpm exec tsc --noEmit            # 0 错误
cargo build --release -p llama-helper
cp target/release/llama-helper frontend/src-tauri/binaries/llama-helper-aarch64-unknown-linux-gnu
cp /usr/bin/ffmpeg  frontend/src-tauri/binaries/ffmpeg-aarch64-unknown-linux-gnu   # 系统是静态版
pnpm exec tauri build --bundles deb
```

产物：`target/release/bundle/deb/meetingbox_0.1.0_arm64.deb`（51 MB），增量构建 8 分 04 秒。

首次构建暴露并修复了 3 个编译错误（见提交 `2e49603`）：
`apply_hotword_corrections` / `configured_hotwords_json` 需要 `pub(super)`；
`local_services` 的 `(!**state)` 多解一层。

`externalBin` 要求 sidecar 存在且带目标三元组后缀，否则 `tauri build` 在资源检查阶段直接失败。

---

## 4. 服务层验证（已验证）

复用设备上已有的 `asr.service`（`:8020`）与 `diar.service`（`:8030`），
桥接独立跑在 `:8130`，不干扰现网服务。

| 样本 | 时长 | chunks | finals | 说话人 | 耗时 | RTF |
| --- | --- | --- | --- | --- | --- | --- |
| 单人合成音 | 5.7 s | 29 | 2 | 发言人 1 | 2.78 s | 0.49 |
| 两人交替（Flo→Eddy→Flo） | 9.6 s | 49 | 3 | 发言人 1 / 2 / 1 | 3.70 s | 0.39 |
| 真人远场（USB 会议麦，数数 1–30） | 40 s | 200 | 2 | 发言人 1 | 36.4 s | 0.91 |
| 同上，4 s 窗口 / 2 s 步长 | 40 s | 200 | 2 | 发言人 1 | 28.4 s | 0.71 |
| 同上，关闭 partial | 40 s | 200 | 2 | 发言人 1 | 18.4 s | 0.46 |

**0.46 是设备 ASR 自身下限**（VAD + 推理），partial 是叠加开销。默认值已按 4 s / 2 s 固化。

另外确认：设备 ASR 本身就返回带说话人的分段，因此 `rknn_stream` 改为
「本地声纹模型优先，没有则透传设备标签」。

---

## 5. 未解决：应用前端卡在加载态

现象：进程正常启动，后端日志显示数据库、模板、托盘全部初始化成功，
但界面停在 `正在启动本地安全服务…`，且 `get_platform_profile` 从未被调用
（`Platform profile:` 日志一行都没有）。

已排除：

- 不是渲染帧问题（换 `WEBKIT_DISABLE_DMABUF_RENDERER=1` 无效）
- 不是许可证拦截（已带 `MEETINGBOX_ALLOW_UNLICENSED_DEV=1`）
- 后端初始化确实成功（`Database opened successfully` 等日志齐全）

待查方向：`HYOSAccessGate` 里 `accessControlService.getAdminStatus()` 的 promise 未 settle，
需要在设备上开 WebKit inspector，或在开发机上用浏览器跑同一份前端复现。

---

## 6. 部署注意（这次踩过的）

1. **包名冲突**：本分支的 `productName` 是 `meetingbox`，与设备上既有的
   `meetingbox 0.4.14` 同名，`dpkg -i` 会把它当降级覆盖。
   验证前必须改包名（如 `meetingbox-unified`）或先备份旧包。
   本次已用 `/home/ubuntu/meetingbox-deployment-0.4.14/meetingbox_0.4.14_arm64.deb` 回滚。
2. **旧进程会在内存里继续跑**：覆盖安装不会杀掉已运行实例，
   于是 `ps` 里看得到进程、界面也能操作，但实际跑的是被删除的旧二进制。
   判断方法：
   ```bash
   ls -l /proc/<pid>/exe      # 出现 " (deleted)" 就是旧二进制
   ps -o lstart= -p <pid>     # 启动时间早于安装时间即可确认
   ```
3. **单实例锁**：旧实例占着 `org.com_meetily_ai.SingleInstance`，新实例启动后会让位。
4. **`meetingbox-voice` 会抢麦克风**：常驻 39% 单核并占用音频设备，
   录音测试前 `sudo systemctl stop meetingbox-voice`。

---

## 7. 复现步骤（从零到服务层可用）

```bash
# 设备侧
python3 -m venv /opt/meetingbox-rk/venv
/opt/meetingbox-rk/venv/bin/pip install fastapi uvicorn httpx pydantic python-multipart
sudo systemctl stop meetingbox-voice
cd /opt/meetingbox-rk/services/asrserve-bridge
MEETINGBOX_PRIMARY_ASR=rknn MEETINGBOX_RK_ASR_URL=http://127.0.0.1:8020 \
  /opt/meetingbox-rk/venv/bin/python -m uvicorn bridge:app --host 127.0.0.1 --port 8130

# 验证
python3 smoke_rk.py --health-only
python3 smoke_rk.py --wav sample.wav
```
