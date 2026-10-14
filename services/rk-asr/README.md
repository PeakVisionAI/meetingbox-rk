# RK3588 SenseVoice RKNN service

这是 meetingbox 的可选独立推理 sidecar，使用 sherpa-onnx **真实 RKNN provider**，不依赖已归档的 `backend/`。会议数据、VAD、音频采集和正文热词修正仍由 Rust/Tauri 负责。默认仅监听 `127.0.0.1:8020`；不提供公网服务。

源码与协议测试 14 项已在 macOS 验证，原生桥接已对固定版本上游头文件完成 C 语法检查。**尚未在 RK3588 上链接、加载模型或运行 NPU**。以下步骤生成真实部署资产，完成后仍须实机验收。

## 固定来源

| 资产 | 固定版本 / 来源 |
| --- | --- |
| 推理实现 | [sherpa-onnx v1.13.8](https://github.com/k2-fsa/sherpa-onnx/tree/11afbd009a7f8c08f4bcf2fc1b265d0df4670fbf)，Linux/aarch64 RKNN shared release |
| 模型 | SenseVoiceSmall RK3588 **30 秒**，2024-07-17，zh/en/ja/ko/yue；[上游 RKNN 模型说明](https://k2-fsa.github.io/sherpa/onnx/rknn/models.html) |
| RKNN runtime | Rockchip toolkit2 v2.2.0 的 aarch64 `librknnrt.so`，固定 commit `7efa763d1da04c4c4447fa5632dc2d9c94fb3063` |
| 头文件 | 与 sherpa release 相同 commit 的 `c-api.h` |

[upstream.json](upstream.json) 保存实际下载 URL、字节数和 SHA-256。两个 release archive 的校验值来自 GitHub release digest；头文件、runtime 与许可证的校验值来自下载后的字节哈希。模型 archive 419,498,837 bytes；sherpa archive 28,250,944 bytes；runtime 7,192,616 bytes。准备期间至少需要 **2 GiB 空闲磁盘**，不会把这些二进制提交到 fork。本机空闲不足 1 GiB，未执行完整模型下载、archive 展开与目标库链接；archive 内文件布局根据上游打包脚本核对，准备工具还会检查实际下载包中所需文件是否存在。

sherpa 源码为 Apache-2.0。SenseVoiceSmall 原始作者为 FunAudioLLM/阿里巴巴达摩院，权重遵循 [FunASR MODEL_LICENSE](https://github.com/modelscope/FunASR/blob/f793eae5778eae93854646f9b4b9c42a23584651/MODEL_LICENSE)，有保留来源、作者和模型名称等条件。Rockchip runtime 使用自己的 Copyright Statement；客户镜像再分发前应按供应商授权确认。准备工具将许可证单独下载并保留；本仓库交付适配源码与下载工具，不再分发第三方权重或 runtime。

## 在 Ubuntu/aarch64 RK3588 上准备

需要 Python 3.10+、C 编译器、目标 NPU 驱动与 `render`/`video` 权限；Python 服务只用标准库，不安装 PyTorch/FunASR。runtime 2.2.0 是上游文档已知可用版本，不保证任意板卡驱动与镜像兼容，不自动更换内核驱动。

以下假设仓库安装到 `/opt/meetingbox-rk`；若路径不同，同步修改 unit、库路径及构建参数。模型不在启动期间自动下载。

```bash
cd /opt/meetingbox-rk
python3 -B services/rk-asr/prepare.py --output services/rk-asr/models
python3 -B services/rk-asr/build.py --assets services/rk-asr/models
LD_LIBRARY_PATH=/opt/meetingbox-rk/services/rk-asr/models/lib \
  python3 -B services/rk-asr/server.py --assets services/rk-asr/models
```

`prepare.py` 校验所有下载大小和 SHA-256，拒绝 archive 路径越界、特殊文件和危险链接；输出 `models/manifest.json`，包含每个安装文件的独立哈希。启动时再次校验清单全部文件。现有输出目录不会被覆盖。

离线部署：按 `upstream.json` 下载每项资源，分别命名为 `model`、`sherpa`、`header`、`rknnrt`、`sherpa_license`、`model_license`、`rknn_license`，放在同一目录，运行：

```bash
python3 -B services/rk-asr/prepare.py --cache /absolute/cache --output services/rk-asr/models
```

构建在资产目录生成薄 C 桥接 `libmeetingbox_rknn.so`，记录其哈希，复用预编译 sherpa 库。启动将子进程的库路径绑定到同一已校验资产目录。上游 release 不带头文件，因此工具单独获取固定版本头文件。`LD_LIBRARY_PATH` 应始终指向准备好的库目录，保证传递依赖使用相同 runtime。`ldd` 输出若存在 `not found`，须先解决依赖；不能将编译通过当作推理成功。

## systemd 部署

先完成前台健康与音频测试，再安装提供的 [unit](deploy/meetingbox-rk-asr.service)。检查 8020 端口：旧 `asr.service` 若已监听，应先按实际设备维护流程处理冲突，不同时启动两个 ASR 服务。

```bash
sudo useradd --system --no-create-home --shell /usr/sbin/nologin meetingbox-asr
sudo install -m 0644 services/rk-asr/deploy/meetingbox-rk-asr.service \
  /etc/systemd/system/meetingbox-rk-asr.service
sudo systemctl daemon-reload
sudo systemctl enable --now meetingbox-rk-asr.service
systemctl status meetingbox-rk-asr.service
journalctl -u meetingbox-rk-asr.service -n 50
```

账号已存在时跳过 `useradd`。服务账号须能读取仓库/模型目录并访问实际 NPU 节点。unit 的 `SupplementaryGroups=render video` 适用于常见 Ubuntu 镜像，具体设备需核对。没有使用 `PrivateDevices`，避免隐藏 NPU 节点。非特权服务不记录上传音频或完整正文；原生库日志被隔离，失败以错误类别报告。

## 客户端与协议

```bash
export MEETINGBOX_ASR_BACKEND=rk-npu
export MEETINGBOX_RK_ASR_URL=http://127.0.0.1:8020
export MEETINGBOX_RK_ASR_TIMEOUT_SECONDS=90
export MEETINGBOX_ASR_FALLBACK=bossai
export MEETINGBOX_BOSSAI_ASR_URL=http://127.0.0.1:8002
```

BossAI CPU 服务仍按 [复用文档](../../docs/BOSSAI_ASR_INTEGRATION.md) 独立准备；上述配置只启用已实现的显式回退。

- `GET /healthz`：模型哈希验证和实际原生模型初始化成功后才报告 ready；包含实际模型标识、backend/device、runtime 和能力。
- `POST /v1/asr/transcribe-upload`：原有 multipart 协议，接受 0.1–30 秒 16kHz mono PCM16 WAV，包括 meeting_id=0 的探测。上游固定 30 秒模型对短音频补齐；拒绝更长输入，避免模型静默截断。客户端已做增益与前后补零，服务不重复处理。
- `hotwords` 为 JSON 字符串数组，校验后交由客户端正文修正；不声称模型具备解码热词偏置。`skip_vad` 兼容接收；服务依赖客户端 VAD，不加载第二个 VAD。
- 模型自动检测五种语言；目前 RK 客户端不传语言。接口只支持 `language=auto`。
- 不推定词级时间戳：输入中包含客户端合成的静音，原始长度未随请求发送。响应只提供正文与空 speaker，由桌面保留原始录音时间轴。
- **没有说话人模型**。finalize 返回 `finalized=false,pending=false,segments=[]` 和明确原因，避免覆盖原逐字稿；不伪造校准结果。
- 单次推理串行，最多 8 个 HTTP 连接，忙时返回 503。原生子进程保留模型，85 秒超时或致命退出即终止并标记服务不可用，需要重启；客户端可显式回退 CPU。初始化预算 300 秒。上传读取超时 10 秒，最大 body 约 1 MiB。
- 客户端默认 90 秒，应保持该设置或根据实机性能提高到支持范围；降低至小于服务计算时限时，客户端超时不会立即取消后台计算。

## 验证

开发机协议、资产处理、原生进程隔离测试：

```bash
python3 -B -m unittest discover -s tests/rk-asr -v
```

目标机实际模型与硬件证据：

```bash
python3 scripts/check-rk-asr.py --audio /absolute/test-16k-mono.wav
sudo python3 scripts/collect-rk-asr-evidence.py \
  --audio /absolute/test-16k-mono.wav \
  --assets /opt/meetingbox-rk/services/rk-asr/models \
  --output /absolute/new-rk-asr-evidence.json
```

探测上传指定音频，不打印正文。证据工具保存 OS/内核/内存、驱动版本、runtime/model 清单及测试期间 NPU load（debugfs 路径缺失时明确为 null），需要人工与实际推理对应。`hardware_verified` 始终 false，不用服务自报元数据自动判定硬件通过。再按 [NPU 验收要求](../../docs/RK_NPU_ADAPTATION.md) 完成真实会议、掉线/超时/CPU 回退和长片段质量测试。此包仅补齐 ASR 路径，不提供历史部署基线的 Pyannote/ERes2Net 或 RK1828 摘要模型。
