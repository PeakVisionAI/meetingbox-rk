# Intel 版本接入

保留 MeetingBox 的 Tauri/Next.js 应用、录音、会议数据库、播放和手动校对界面。
语音识别和说话人分离使用未经修改的 AsrServe 1.0.5 CPU 镜像。

## 服务

| 服务 | 地址 | 用途 |
| --- | --- | --- |
| AsrServe | 17003 | 原始识别接口、浏览器调试页 |
| 兼容层 | 8020 | 原应用 ASR 上传、会后校准和 AI 校对建议 |
| 兼容层 | 8050 | 原应用分时摘要接口 |
| Ollama | 11434、8040 | 独立本地语言模型、原应用完整摘要接口 |

AsrServe 不生成会议纪要，也不提供通用的 LLM 文本校正。Ollama 承担这些任务。

## 启动

在此目录运行（需要 Linux Docker 与 Compose）：

```sh
docker compose up -d
docker compose exec -T llm ollama pull qwen3:4b-instruct
docker compose exec -T llm ollama cp qwen3:4b-instruct qwen3-4b
```

AsrServe 首次启动会下载固定版本的模型；`/healthz` 只有上游确认 model_loaded 后才返回 ready=true。
兼容层直接复用 AsrServe 镜像中的 Python/API 运行环境，不需要另外构建后端镜像。
模型全部缓存并校验后，可使用 `docker compose -f compose.yml -f compose.offline.yml up -d` 禁用 Hugging Face 联网下载。
默认端口只监听回环地址。VMware 部署将端口绑定到独立 NAT 网络 192.168.44.50，Windows 主机可访问。

## Windows 应用配置

VMware 后台时，通过启动脚本设置以下环境变量：

```powershell
$env:MEETINGBOX_ASR_URL = 'http://192.168.44.50:8020'
$env:MEETINGBOX_SUMMARY_URL = 'http://192.168.44.50:8050'
$env:MEETINGBOX_FINALIZE_TIMEOUT_SECONDS = '3600'
```

在原应用模型设置中，将完整摘要的 Custom OpenAI 地址设为
`http://192.168.44.50:8040/v1`，模型名 `qwen3-4b`。
本机调试版可使用代码已有的 `MEETINGBOX_ALLOW_UNLICENSED_DEV=1` 机制；正式 release 仍需有效设备许可证。

## 数据与行为

- Intel 启动脚本开启 `MEETINGBOX_ASR_STREAM=1`；连续 PCM（包括静音）进入同一 AsrServe `/v1/stream` 会话。
- 每 200 毫秒发送缓冲音频，快速模型会话保持；显示的语句边界来自后台停顿事件，按对应语句的说话人事件发布标签。
- 会话内发言人标签保持同一编号；原始连续音频保存为 `stream.pcm`，模型事件保存为 `stream.events.jsonl`。
- 解码进度控制发送速率，避免持续上传填满服务队列并触发断流。结束录音先排空音频和最终说话人标签，再移除转写持久化监听器。
- 连续流结束时保存已生成文字及异步标签，常规停止不再等待整场重识别。显式整场重处理接口仍保留。
- full-meeting 响应由前端采用完整转写；原 RK 服务的说话人覆盖逻辑保持兼容。
- AI 校对建议单独保存，用户在已有校对弹窗中查看、采用后再保存。
- 保存校对时，在同一数据库事务内记录原文、修改文本、说话人和时间。
- 校对失败或数字、否定词验证失败时保留原文。自动校验不能保证姓名和语义完全正确，仍需结合录音校对。
- 旧上传接口为 16 kHz、单声道、PCM16 WAV，最大 32 MiB/片段；拼接校准上限四小时。现成实时服务单会话上限一小时、CPU 同时一场。
- 断电后可重试 finalization；完成结果持久化，运行中任务不会跨服务重启恢复。

## 验证边界

已提供兼容层自动测试；模拟上游测试不代表真实模型识别、CPU 实时性或端到端会议验收。
Windows 前端需要 Tauri/MSVC/Rust 构建环境。浏览器页面不能替代桌面录音验证。
实际验收包含中文双人录音、整场说话人稳定性、校对原文追溯、分时摘要、最终纪要、重启与长时会议。

## 本次 Intel 部署记录（2026-10-14）

Windows 主机 DESKTOP-RD7D8L3，Intel Core Ultra X7 358H、64 GB 内存。
VMware Ubuntu 24.04.5，8 vCPU、32 GB 内存、160 GB 磁盘，NAT 地址 192.168.44.50。
Ubuntu 用户 meetingbox，通过 SSH 密钥登录；未设置控制台密码。私钥和账号凭据不包含在源码包内。
前端位于 D:\MeetingBox-Intel\app\HYOS.exe，桌面快捷方式 HYOS MeetingBox。
后台位于 /opt/meetingbox，Docker 服务随 Ubuntu 启动；VMware 虚拟机在 Windows Administrator 登录时启动。

已验证 Windows 原前端编译和登录页面、后台真实中文识别、会后单人标签和时间戳、整场校对建议、单段校对、摘要。
4.20 秒公开中文单人音频，首次识别 12.1 秒、暖机后 5.4 秒；重复拼接为 21.02 秒音频，识别 18.8 秒。
上述短样本不能代表双人会议效果或长会议持续性能，短句有明显延迟。
兼容层 13 项自动测试通过；前端类型检查、静态构建、会后结果采用测试以及 Windows 原生调试版构建通过。
测试版仍使用代码已有的开发授权机制，正式发行需要有效设备许可证。
前端 admin 已初始化并通过 noVNC 登录；上一场中断会议的 53 条转写已恢复。

## 持续语音流验证

服务自带 `R2T2_CHUNK_SECONDS=2`，保留跨上传帧上下文。CPU 模式下采用较长解码间隔降低重复计算。
10.41 秒公开中文样本（同一声音两次发言、中间静音）输出两个停顿段、同一个标签；该次处理耗时 10.85 秒。
97.6 秒 Nemotron 官方多声演示输出 11 个语句段、6 个标签，整个会话完成；该次测试期间 Windows 前端也在编译，端到端耗时 212.36 秒。
这些结果验证接口、会话稳定性和多标签输出，不代表中文真实双人会议准确率，也不能保证 CPU 在长会议中保持实时。
原演示包含八种声音，连续抢话的语句使用主讲者标签；现成接口没有实时词级时间戳，抢话和同句换人仍需会后校准。
旧会议的逐字稿不因启动新版本自动改写。新的会议使用持续语音流，保留热词、原文、校对建议和修改历史。



## Low-latency live transcription

The compatibility service uses sherpa-onnx 1.13.8 with the Chinese streaming
Zipformer CTC INT8 model for immediate provisional text. Nemotron speaker inference
runs independently and updates the same utterance ID after a pause. The Windows
client sends 200ms PCM frames and upserts partial/final/speaker updates in place.

Place `model.int8.onnx` and `tokens.txt` in `models/streaming/live/` before starting.
Official model: https://huggingface.co/csukuangfj/sherpa-onnx-streaming-zipformer-ctc-zh-int8-2025-06-30
Pinned revision: f29132af655b91f258800df19c5b5d9b9281a5cd
ONNX SHA256: 24ffdc19ba9aaed5a6a9beaede1e087745217d82425cf4041bca0c696661801e
The inherited AsrServe image must contain its cached Nemotron model under
`models/asr/nemotron-3-diarization`; startup warms both models before health is ready.
Use `docker compose build bridge` once, then the existing offline compose override.

Measured on the target Intel VM: 24.73s Chinese replay, 111 token observations,
median 0.50s and P95 0.66s from model token time to HTTP response. A separate 40s
meeting sample returned three speaker labels and P95 0.66s; backend end flush was
0.54s. These measurements do not include screen rendering or establish an accuracy
score. Ambiguous speech and overlapping speakers still require review.
Raw PCM and every partial/label update are retained under each backend meeting ID.


## Intel 核显整句复核（2026-10-14）

Windows 原生 Python 3.12 独立环境：`D:\MeetingBox-Intel\asr-openvino`。
最终复核模型为 Qwen/Qwen3-ASR-1.7B，官方原始 revision：
`7278e1e70fe206f11671096ffdd38061171dd6e5`；通过官方权重 SHA-256 校验后本地转换为 INT8。
OpenVINO 2026.4.1、GenAI 2026.4.1.0、optimum-intel 2.2.0、qwen-asr 0.0.6。
模型和环境只位于目标机，不包含在源码压缩包中。部署脚本见 `services/qwen-openvino/`。
执行安装、下载、转换脚本后，将 start-service.ps1 放入目标独立环境目录，再注册任务。
服务在 Administrator 登录时启动，任务名 MeetingBox-Intel-ASR；监听 VMware NAT 地址
192.168.44.1:8060，仅允许 Ubuntu 192.168.44.50 调用，未向办公网开放。

快速 Zipformer 草稿先显示；停顿后完整句 PCM 交给 Windows Arc B390 核显上的
Qwen3-ASR-1.7B 复核；更新同一 utterance ID，同时保留最近的 Nemotron 标签。
原始 PCM、事件、draft_text 和最终文本留存。热词通过 Qwen 的 context 参数传入。
复核超时或服务暂不可用时保留草稿。结束时给剩余复核最多 15 秒收尾时间。

同一段 20 秒真实录音，0.6B 核显暖机 1.27 秒 / CPU 1.61 秒；
1.7B 核显暖机 1.53 秒 / CPU 3.20 秒。因此本机采用 1.7B 核显复核。
这是单段测量，不代表所有句子的延迟，也没有人工逐字参考文本来计算准确率。
瞬时草稿和最终复核稿的延迟分别验收，屏幕渲染延迟需桌面实测。


## 结束会议崩溃与短间隔换人修复

Windows Application 1000 在停止录音后报 0xc000001d；匹配调试符号定位到
Tao EventLoopRunner 的 Rc 引用计数路径。托盘菜单创建、设置、销毁统一派发到窗口主线程。
停止已完成音频、transcripts.json 写盘后才崩溃的会议可以从这些文件恢复。

说话人推理每完成约一秒音频就回写标签，避免积压时一直等追上所有音频。
Nemotron 的 10ms 活动时间线识别同句中的换人位置；达到 500ms 的稳定声段可以
分别识别，并用同一个原段 ID 与独立子段 ID 更新界面，不重复保留整段文字。
完整原音频时间区间会被保留，重叠/不确定声段保持待识别。
微小身份抖动不会立即创建新发言人；模型本身的误判仍需要人工校对。


## 2 秒段落、词级说话人归类与 8B 核显总结

同一发言人连续展示；实际停顿超过 2 秒、累计含间隔空格超过 200 字或换人时新起段落。
取消旧 30 秒强制换段限制。原始子句仍保留，支持逐句校对、定位录音。

短声段重新识别会丢上下文，本版保留整句 Qwen3-ASR 1.7B 文本，采用
Qwen3-ForcedAligner 0.6B 的 CPU 词级时间戳，把原文分配给 Nemotron 检出的声段。
拆分前后字符完全相同，不再对一两秒子段重新识别；对齐失败保留整句与待识别标签。
语音端点停顿设置 2 秒；草稿仍实时输出。前端用实际词时间计算停顿，去掉块末尾静音。

总结已从 Ubuntu Ollama Qwen3 4B Instruct CPU 切为 Windows 原生 Qwen3 8B INT4 核显。
HF 原始转换模型 OpenVINO/Qwen3-8B-int4-ov，revision
5c47abf4b8e12ebe8e99745bb0c1ec17e0c0abcc，权重 SHA256
f2e6960fde61d24a83c477c46f9ee7158748762089f2018fc2bb6a6d6c7f2a7e。
对齐模型 Qwen/Qwen3-ForcedAligner-0.6B，revision
c7cbfc2048c462b0d63a45797104fc9db3ad62b7，权重 SHA256
47831d0e82f96b20e9034dba01a075ee06436654719f6a68289e49f1b65ce0e7。
下载脚本已附，原始权重不混入源码包；目标环境下载后校验官方 SHA256。

总结任务 MeetingBox-Intel-Summary，在 Windows 登录时启动，192.168.44.1:8061，仅 VM 可调用。
关闭思考输出、串行生成，实际前端模型名改为 qwen3-8b，仍使用兼容层的 8040/v1 地址。
标准模板按实际信息量输出，疑似错词不扩展为业务判断，保留时间依据。
同份三分钟真实逐字稿、同要求实测：8B 核显 13.65 秒，4B CPU 113.59 秒；输出长度不同，
以上耗时仅适用于该样本，输出长度不同，无法据此比较固定长度吞吐或预测其他会议耗时。短样本 8B 约 6.4 秒。


最终整合实测选择：Qwen3-ASR 1.7B 改用 Windows CPU 4 线程，词级对齐 CPU 4 线程，
核显仅运行 Qwen3 8B 总结。核显 ASR 与其余服务整合时出现异常重复字；CPU 连续重放一致。
按音频时长限制 ASR 输出上限，并拒绝明显重复输出，失败时原草稿仍保留。
最终 40 秒多标签录音测试：草稿 HTTP 延迟 P95 0.67 秒，结束收尾 1.09 秒，
拆分前后文字完全一致，词时间边界通过验证。此测量不包含屏幕绘制，不能代替准确率标注。


2026-10-14 声纹回退：恢复整句主导说话人投票，关闭默认短声段/按词换人（MEETINGBOX_EXPERIMENTAL_SPEAKER_TURNS 默认 0）；后台端点恢复 0.6 秒。前端同人 2 秒/200 字合并保持不变。模型权重及 0.5 活动阈值未改。

开始会议前统一检查 ASR 8060、总结 8061 和实时桥接服务。缺失时通过现有任务启动本机模型，必要时启动 VMware，并以受限 SSH 固定命令拉起 Docker 服务。界面进度按真实已就绪服务数 0/3 至 3/3 显示；180 秒超时阻止录音并提示未就绪服务。服务启动私钥仅保存在目标电脑，源码包不包含密钥。

2026-10-14 Windows 界面随机崩溃修复：已符号化的栈定位到 Context -> DispatcherMainThreadContext -> EventLoopWindowTarget -> Rc<EventLoopRunner> 的跨线程克隆。固定 tauri-runtime-wry 2.11.1 到 vendor，本地主线程上下文改用 Arc 共享，实际界面访问仍受原主线程检查保护。保留 debug 内存安全检查。原运行时在 8 线程句柄克隆压力测试中立即崩溃，修复运行时完成 160000 次实际句柄克隆与释放。托盘中间态菜单也统一切回主线程；音频诊断计数器改为原子计数。MEETINGBOX_RUNTIME_CLONE_TEST=1 仅在 debug 中启用压力测试，正常启动关闭。


当前公开数据验收版本（2026-10-14 晚间）：前端同人 2 秒/200 字归段保留；实时端点改为 1.2 秒，最长识别片段 19 秒。实时阶段仍保守分配说话人，结束时由 word_speakers.py 按词级时间校准，完整保留文字。新身份需稳定活动证据，短标签岛保守处理。

AliMeeting 与 AISHELL-4 两场各 4 分钟抽样数据实时回放，校准与独立验证片段均有改善。独立 AliMeeting 段排除重叠的说话人 DER 从 15.67% 降至 3.41%；AISHELL-4 两段合并 CER 从 16.06% 降至 12.56%。这些属于片段测试，无法代表全语料成绩。多人重叠与极短发言仍有漏识别。

总结模型禁用磁盘 GPU 编译缓存，以规避已复现的 oneDNN/OpenCL 缓存加载失败。标准模板通过 grounded_report.py 验证有时间戳的原文行，再填入各节；建议、决策与任务类型分别校验。行文偏向原文引用。官方 4 分钟逐字稿两次调用输出一致，约 5–6 秒，生产接口复测完成。summary_ov_server.py 与 grounded_report.py 需一同部署。
