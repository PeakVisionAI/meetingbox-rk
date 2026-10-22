# 会议流式字幕预览

本服务只加载 BossAI 的 Paraformer 流式模型，不加载文件识别模型。每个 600ms 的 16kHz 单声道 PCM16 音频块返回当前句子的累计正文；客户端替换实时预览文字。句子结束时以 `final=true` 返回完整累计正文，并清除该句缓存。

会议保存仍走 RK ASR 与说话人服务：会议中的候选标签和结束后的声纹校准使用整场会议会话及时间轴。流式预览通过独立 Tauri 事件显示，不向逐字稿数据库追加临时文字；原识别结果覆盖相应时间后，界面清除预览。

启动 `serve.py` 使用已固定版本的 BossAI Python 环境，并设置：

- `BOSSAI_ASR_SERVER_DIR`：已固定版本的 BossAI ASR 源码目录，含 `server.py`。
- `BOSSAI_STREAM_MODEL`：本地 Paraformer online 模型目录，含 `model.pt` 与配置/词表。
- `BOSSAI_ASR_MODEL_DIR` / `MODELSCOPE_CACHE`：既有本地模型缓存。
- `ASR_PORT`：默认 8002，只监听回环地址。
- `BOSSAI_LOCAL_CPU_THREADS`：默认 1，向 FunASR 的 `ncpu` 以及 OMP/MKL 传递同一线程预算。
- `BOSSAI_STREAM_INT8=1`：只动态量化编码器的 Linear 层，解码器和预测器保持原精度。模板启用此项；设为 0 可回退浮点预览。

`serve.py` 固定 streaming/CPU 模式、`BOSSAI_ASR_PRELOAD_FILE=0`。systemd 模板将推理限定在 RK3588 的 4–7 性能核，并限制内存和重启频率。服务只提供健康检查、预热及流式入口，文件入口返回 409。HTTP 请求取消后，服务器仍等当前推理完成才释放全局锁，避免并行调用同一模型。服务每分钟在同一锁内回收超过 600 秒未使用的模型缓存与会话元数据，避免停止超时留下的预览缓存长期驻留。

客户端串行提交 600ms 块，音频队列有容量上限；超过上限会清除失效预览并显示故障状态。暂停会串行 flush，恢复使用新的 ASR 缓存 UUID；停止最多等待 5.5 秒。预览失败不影响原始录音和保存用转写。事件使用会议录音 UUID，前端核对原生当前会话以拒绝旧会议事件。

客户端环境：

```text
MEETINGBOX_ASR_BACKEND=rk
MEETINGBOX_RK_ASR_URL=http://127.0.0.1:8020
MEETINGBOX_STREAM_ASR_URL=http://127.0.0.1:8002
MEETINGBOX_WAKE_ENABLED=1
MEETINGBOX_VOICE_URL=http://127.0.0.1:8021
```

已有文件 worker 与流式 worker 互斥：先停止并禁用 `meetingbox-bossai-asr.service`，再安装启用 `meetingbox-live-asr.service`。模型和 Python 环境由设备服务部署管理，不随客户端 Debian 包自动安装。修改模板中的绝对路径以匹配目标机器。回滚须恢复原客户端包、启动器和文件 worker 单元，数据库不能用旧备份覆盖新会议。

2026-10-14 的 RK3588 验证：

| 测量 | 浮点编码器 | INT8 编码器 |
| --- | --- | --- |
| 600ms 块，桌面持续回放平均推理 | 0.644 秒 | 需结合下述端到端延迟评估 |
| 5.06 秒语音，8020 同时推理，流式单块平均 | 约 0.50 秒 | 0.36–0.41 秒 |
| 同一固定测试句 | 帮我根据这周的客户拜访记录写一份周报 | 相同 |
| 两分钟桌面回放，末尾预览延迟 | 约 9.50 秒 | 约 0.80 秒 |
| 两分钟桌面回放，最大预览延迟 | 约 9.50 秒 | 约 2.04 秒 |
| 两分钟流程预览事件 | 206 | 208 |
| 两分钟流程预览故障 | 停止时积压超过收尾预算 | 0 |

完整 INT8 量化会改变测试句正文，因此仅保留编码器量化。两分钟验收包含三段不同语音反复播放、3 秒暂停恢复、真实“小峰小峰，结束会议”录音回放；会议内没有提示音，语音结束直接保存，最终校准完成，数据库保存 71 条带说话人字段的逐字稿。说话人聚类可能分出额外身份，不能据此宣称人数或声纹识别完全准确。

历史未量化加载峰值约 3.07GiB；最终量化 worker 实测 `VmHWM=3184768KiB`（约 3.04GiB）、`VmRSS=1821044KiB`（约 1.74GiB）、`VmSwap=0`。服务限制为 `MemoryHigh=3584M`、`MemoryMax=4G`，验收后流式、8020、8030、8021 均就绪。验收使用 PulseAudio 回放并经过实际客户端采集、混音、识别及保存链路；未完成 10 分钟以上、真实房间双人对话或低音量声学测试。

最终安装包 `meetingbox_0.4.14_arm64.deb` SHA256：`aae157b8b54e76a84ed289fb61615f42e04fdf86aced84bb8c301502c662b05b`。安装包复测得到 61 条预览事件、58 个累计正文、0 次故障；保存 23 条带独立说话人字段的逐字稿，临时说话人前缀已从正文移除。生产客户端全屏 1280×960、USB 麦克风自动选择、唤醒和关机授权检查通过，原有 8 场会议、管理员凭据及许可证保留；测试会议与录音已备份归档。设备证据位于 `/home/ubuntu/meetingbox-deployment-0.4.14/`，两分钟性能和语音结束证据位于 `/home/ubuntu/meetingbox-deployment-0.4.13/final-acceptance.json`。

协议回归测试可用已有 BossAI 环境运行：

```bash
python -m unittest discover -s tests/live-asr -v
cargo test --locked --manifest-path tests/voice-adapter/Cargo.toml
```
