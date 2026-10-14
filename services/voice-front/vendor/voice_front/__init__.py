"""语音前段：唤醒词、打断词、切句、免唤醒判断。

这是语音识别 sidecar 里的**可选**子模块：页面把麦克风音频按小块送到 `/voice/feed`，
取回“听到了什么”的事件。它只负责听，不执行任何指令。

导入本包不会加载 sherpa-onnx、torch 或任何模型：缺依赖、缺模型时
`/voice/health` 返回不可用及原因，sidecar 现有的 `/health`、听写、文件转写不受影响。

唤醒、切句、去唤醒词、免唤醒判断四个模块移植自 PeakNode 语音模块 demo
（peaknode-voice-demo，提交 8789978），接口与事件格式见
`packages/contracts/src/voiceFront.ts`。
"""
from __future__ import annotations

from .router import mount

__all__ = ["mount"]
