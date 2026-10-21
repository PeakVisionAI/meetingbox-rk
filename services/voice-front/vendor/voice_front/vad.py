"""切句：Silero VAD（由 sherpa-onnx 运行）把连续音频切成一句一句。

移植自 peaknode-voice-demo 的 `peaknode/speech/vad.py`。每路监听持有自己的
`VadStream`；构造一个只要几毫秒，但不要每块音频都重建。

实测要点：

1. 音频结束时必须 `flush()`，否则最后一句一直挂在内部缓冲里。
2. `SpeechSegment.start` 是这个检测器创建以来的采样计数，要自己换算成时间。
3. `SpeechSegment.samples` 是 Python list，要转成 ndarray。
4. `max_speech_duration` 只是软上限（设 2.0 秒实测能吐出 2.9 秒的段），
   所以这里再做一次硬切，保证每句不超过上限。
5. **不能一次喂进去一大段。** 一次 `accept_waveform` 给 2 秒音频时，检测器把这句话的
   起点定在这 2 秒的末尾附近，前面的内容全丢（实测：同一段音频，按 200ms 喂起点在
   1.06 秒，先喂 2 秒一整块起点变成 1.67 秒）。唤醒时回补的那 2 秒正好踩中这个坑，
   所以 `feed()` 内部一律拆成 100ms 的小块。
6. **检测器给出的起点偏晚，会切掉句子的第一个辅音。** 实测 aishell 200 句：整句识别
   字错率 3.89%，按检测器切出的段识别是 4.54%（“村书记”识别成“孙书记”，
   “我国”识别成“国”），往前多带 0.3 秒后是 3.68%。所以这里自己留一份最近的音频，
   每句都往前补 `LEFT_PAD_S`。
"""
from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
from typing import Any

import numpy as np

from .settings import SAMPLE_RATE

__all__ = ["VadStream", "VadSegment", "VadUnavailable"]

_FEED_SAMPLES = SAMPLE_RATE // 10
#: 每句往前多带的音频（要点 6）。
LEFT_PAD_S = 0.3


class VadUnavailable(RuntimeError):
    """sherpa-onnx 不可用，或 VAD 模型文件缺失。"""


@dataclass(frozen=True)
class VadSegment:
    #: 相对这个检测器创建时刻的起点（秒）。
    start_s: float
    wav: np.ndarray

    @property
    def duration_s(self) -> float:
        return self.wav.size / SAMPLE_RATE

    @property
    def end_s(self) -> float:
        return self.start_s + self.duration_s


class VadStream:
    def __init__(self, model_path: str | Path, *, end_silence_s: float,
                 min_speech_s: float = 0.25, max_speech_s: float = 60.0,
                 threshold: float = 0.5, num_threads: int = 1) -> None:
        self.model_path = Path(model_path)
        if not self.model_path.exists():
            raise VadUnavailable(f"VAD 模型不存在：{self.model_path.name}")
        try:
            import sherpa_onnx  # type: ignore
        except Exception as exc:  # noqa: BLE001
            raise VadUnavailable(f"sherpa-onnx 不可用：{exc}") from exc

        self.end_silence_s = float(end_silence_s)
        self.min_speech_s = float(min_speech_s)
        self.max_speech_s = float(max_speech_s)

        config = sherpa_onnx.VadModelConfig()
        config.silero_vad.model = str(self.model_path)
        config.silero_vad.threshold = float(threshold)
        config.silero_vad.min_silence_duration = self.end_silence_s
        config.silero_vad.min_speech_duration = self.min_speech_s
        config.silero_vad.max_speech_duration = self.max_speech_s
        config.sample_rate = SAMPLE_RATE
        config.num_threads = int(num_threads)
        config.provider = "cpu"
        try:
            self._vad: Any = sherpa_onnx.VoiceActivityDetector(
                config, buffer_size_in_seconds=max(self.max_speech_s * 1.5, 90.0),
            )
        except Exception as exc:  # noqa: BLE001
            raise VadUnavailable(f"VAD 模型加载失败：{exc}") from exc
        self.segments_dropped = 0
        # 自己留的一份最近音频，用来给每句往前补一小段（要点 6）。
        self._left_pad = int(LEFT_PAD_S * SAMPLE_RATE)
        self._history: list[np.ndarray] = []
        self._history_start = 0     # _history 第一个采样在整条流里的序号
        self._fed = 0               # 已喂入的采样数
        self._history_keep = int((self.max_speech_s + self.end_silence_s + 5.0) * SAMPLE_RATE)

    @property
    def in_speech(self) -> bool:
        return bool(self._vad.is_speech_detected())

    def feed(self, pcm: np.ndarray) -> list[VadSegment]:
        """喂一段音频。长音频在内部按 100ms 一小块喂（要点 5）。"""
        pcm = np.asarray(pcm, dtype=np.float32)
        out: list[VadSegment] = []
        for offset in range(0, pcm.size, _FEED_SAMPLES):
            piece = pcm[offset:offset + _FEED_SAMPLES]
            self._history.append(piece)
            self._fed += piece.size
            self._vad.accept_waveform(piece)
            out.extend(self._collect())
        self._trim_history()
        return out

    def _trim_history(self) -> None:
        while self._history and (self._fed - self._history_start - self._history[0].size
                                 >= self._history_keep):
            self._history_start += self._history.pop(0).size

    def _before(self, start: int) -> np.ndarray:
        """整条流里第 `start` 个采样之前的那一小段音频（最多 `LEFT_PAD_S`）。"""
        begin = max(self._history_start, start - self._left_pad)
        if begin >= start:
            return np.zeros(0, dtype=np.float32)
        pieces: list[np.ndarray] = []
        cursor = self._history_start
        for piece in self._history:
            end = cursor + piece.size
            if end > begin and cursor < start:
                pieces.append(piece[max(0, begin - cursor):min(piece.size, start - cursor)])
            cursor = end
            if cursor >= start:
                break
        return np.concatenate(pieces) if pieces else np.zeros(0, dtype=np.float32)

    def flush(self) -> list[VadSegment]:
        """强制结句（要点 1）。之后可以继续 `feed()`。"""
        self._vad.flush()
        return self._collect()

    def _collect(self) -> list[VadSegment]:
        out: list[VadSegment] = []
        while not self._vad.empty():
            segment = self._vad.front
            start = int(segment.start)
            wav = np.asarray(segment.samples, dtype=np.float32)
            self._vad.pop()
            if wav.size == 0:
                continue
            if wav.size / SAMPLE_RATE < self.min_speech_s:
                self.segments_dropped += 1
                continue
            lead = self._before(start)
            if lead.size:
                wav = np.concatenate([lead, wav])
                start -= lead.size
            out.extend(self._split(start / SAMPLE_RATE, wav))
        return out

    def _split(self, start_s: float, wav: np.ndarray) -> list[VadSegment]:
        """超过上限的段均分成几块（要点 4）。

        均分而不是“切满再留尾”：每块一定不超过上限，也不会切出零点零几秒的碎尾。
        """
        limit = int(round(self.max_speech_s * SAMPLE_RATE))
        if limit <= 0 or wav.size <= limit:
            return [VadSegment(round(start_s, 3), wav)]
        min_samples = max(1, int(round(self.min_speech_s * SAMPLE_RATE)))
        parts = -(-wav.size // limit)
        while parts > 2 and wav.size // parts < min_samples:
            parts -= 1
        step = -(-wav.size // parts)
        return [
            VadSegment(round(start_s + offset / SAMPLE_RATE, 3), wav[offset:offset + step])
            for offset in range(0, wav.size, step)
        ]
