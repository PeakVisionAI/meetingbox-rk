"""一路监听的状态机：喂一块音频，吐出事件和待识别的句子。

状态只有三个：

    idle       待机。唤醒模式下只听唤醒词；免唤醒模式下同时在后台切句。
    listening  已唤醒（或小峰在等回答），等用户开口。
    recording  用户正在说。

时间一律用“这路监听已经喂入的音频秒数”，不用墙上时钟：回放一段录音和真人实时说话
走的是同一套时限，测试也因此可以确定地复现。

本模块不做识别，也不做免唤醒判断——那两步耗时，由 `service.py` 放到后台线程，
做完再调用 `complete()` 把结果交回来。所以这里的所有方法都必须在同一个线程
（sidecar 的事件循环）里调用。

几个容易想错的地方：

1. **唤醒时把之前 2 秒的音频一并交给切句。** 唤醒词的命中比它说完要晚几百毫秒，
   “小峰小峰帮我查库存”连着说时，不回补的话指令开头会被吃掉。回补的音频里带着
   唤醒词，识别出来以后再按拼音从文字里去掉。
2. **只说了唤醒词不算一句指令。** 这一段识别出来去掉唤醒词后是空的，此时继续等，
   并重新给一个完整的等待时限。但这要等识别结果回来才知道，所以唤醒触发的句子
   交出去以后，状态先留在 listening，由 `complete()` 决定回不回待机。
3. **打断词会连带丢掉正在切的这一句。** 否则免唤醒模式下“取消”两个字自己会被当成
   一句话送去判断。
"""
from __future__ import annotations

import uuid
from collections import deque
from dataclasses import dataclass
from typing import Any, Callable, Deque, Literal, Optional, Sequence

import numpy as np

from .kws import KwsEngine, KwsStream
from .settings import SAMPLE_RATE, VoiceFrontSettings
from .streaming import CHUNK_SAMPLES, StreamAudio
from .vad import VadSegment, VadStream
from .wakeword import strip_wake_anywhere, strip_wake_partial, strip_wake_prefix

__all__ = ["VoiceSession", "PendingUtterance", "Mode", "Trigger", "State"]

#: 切句器说“没人在说话了”之后，再等这么久仍没有句子，就判定这一段被丢掉了。
_TOO_SHORT_GRACE_S = 0.3

Mode = Literal["wake", "wake_free"]
State = Literal["idle", "listening", "recording"]
Trigger = Literal["wake", "free", "followup"]
Event = dict[str, Any]


@dataclass
class PendingUtterance:
    """切出来、等识别的一句。音频只在内存里，识别完就丢。"""

    id: str
    trigger: Trigger
    wav: np.ndarray
    t_start: float
    t_end: float

    @property
    def duration_s(self) -> float:
        return self.wav.size / SAMPLE_RATE


class VoiceSession:
    def __init__(self, session_id: str, settings: VoiceFrontSettings, kws_engine: KwsEngine,
                 *, vad_factory: Optional[Callable[[float], VadStream]] = None,
                 wake_words: Sequence[str] | None = None) -> None:
        self.session_id = session_id
        self.settings = settings
        self._kws: KwsStream = (kws_engine.create_stream() if wake_words is None
                               else kws_engine.create_stream(wake_words=wake_words))
        self._wake_words = list(kws_engine.wake_words if wake_words is None else wake_words)
        self._barge_words = list(kws_engine.barge_words)
        self._vad_factory = vad_factory or self._default_vad_factory

        self.state: State = "idle"
        self.mode: Mode = "wake"
        self.t = 0.0
        self.last_seq = -1

        self._vad: Optional[VadStream] = None
        #: 当前切句器的第 0 个采样对应的监听时间。
        self._vad_t0 = 0.0
        #: 当前切句器切出的句子算什么触发方式。
        self._vad_trigger: Trigger = "free"
        self._deadline: Optional[float] = None
        self._rec_start: Optional[float] = None
        #: 切句器最近一次判定“有人在说话”的监听时间。
        self._last_speech_t: Optional[float] = None
        self._followup_seen = False
        #: 已交出去、还没出识别结果的唤醒/追问句子数。
        self._awaiting = 0
        self._pending_audio_s = 0.0

        self._preroll: Deque[np.ndarray] = deque()
        self._preroll_samples = 0
        #: 最近听到的话（监听时间, 原话），给免唤醒判断当前文。
        self._heard: Deque[tuple[float, str]] = deque(maxlen=8)
        self._outbox: list[Event] = []
        self._pending: dict[str, PendingUtterance] = {}
        self._stream_id: str | None = None
        self._stream_has_wake = False
        self._stream_start = 0.0
        self._stream_end = 0.0
        self._stream_buffer = np.zeros(0, dtype=np.float32)
        self._stream_audio: list[StreamAudio] = []
        self._partial_text: dict[str, str] = {}

    def owns(self, utterance: PendingUtterance) -> bool:
        return self._pending.get(utterance.id) is utterance

    def accepts_partial(self, key: str) -> bool:
        return (self.state in {"listening", "recording"}
                and (self._stream_id == key or key in self._pending))

    def partial(self, item: StreamAudio, text: str) -> None:
        if not self.accepts_partial(item.id):
            return
        body = strip_wake_partial(text, self._wake_words, has_wake=item.has_wake)
        if not body or self._partial_text.get(item.id) == body:
            return
        self._partial_text[item.id] = body
        # 同一句只保留最新临时稿，长时间不轮询也不会累积整句的所有版本。
        self._outbox = [event for event in self._outbox
                        if not (event["type"] == "partial" and event["id"] == item.id)]
        self._outbox.append({"type": "partial", "id": item.id, "trigger": "wake",
                             "text": body, "tStart": round(item.t_start, 3),
                             "tEnd": round(item.t_end, 3)})

    def drain_stream_audio(self) -> list[StreamAudio]:
        audio, self._stream_audio = self._stream_audio, []
        return audio

    def _start_stream(self, at: float, *, has_wake: bool = False) -> None:
        self._stream_id = uuid.uuid4().hex[:12]
        self._stream_has_wake = has_wake
        self._stream_start = self._stream_end = at
        self._stream_buffer = np.zeros(0, dtype=np.float32)

    def _stream_feed(self, pcm: np.ndarray, *, final: bool = False) -> None:
        if self._stream_id is None:
            return
        self._stream_buffer = np.concatenate([self._stream_buffer, pcm])
        while self._stream_buffer.size >= CHUNK_SAMPLES:
            part, self._stream_buffer = (self._stream_buffer[:CHUNK_SAMPLES],
                                         self._stream_buffer[CHUNK_SAMPLES:])
            self._stream_end += part.size / SAMPLE_RATE
            self._stream_audio.append(StreamAudio(self._stream_id, part, self._stream_start,
                                                  self._stream_end, has_wake=self._stream_has_wake))
        if final:
            self._stream_end += self._stream_buffer.size / SAMPLE_RATE
            self._stream_audio.append(StreamAudio(self._stream_id, self._stream_buffer,
                                                  self._stream_start, self._stream_end, True,
                                                  has_wake=self._stream_has_wake))
            self._stream_buffer = np.zeros(0, dtype=np.float32)
            self._stream_id = None

    def _discard_stream(self) -> None:
        if self._stream_id is not None:
            self._partial_text.pop(self._stream_id, None)
            self._outbox = [event for event in self._outbox
                            if not (event["type"] == "partial" and event["id"] == self._stream_id)]
            self._stream_audio = [item for item in self._stream_audio if item.id != self._stream_id]
        self._stream_id = None
        self._stream_buffer = np.zeros(0, dtype=np.float32)

    def cancel(self) -> None:
        """当前 entry 的所有在途句子失效；已经启动的线程只能丢结果。"""
        self._pending.clear()
        self._awaiting = 0
        self._pending_audio_s = 0.0
        self._outbox.clear()
        self._partial_text.clear()
        self._stream_audio.clear()
        self._discard_stream()

    # ------------------------------------------------------------------
    # 切句器
    # ------------------------------------------------------------------
    def _default_vad_factory(self, end_silence_s: float) -> VadStream:
        s = self.settings
        return VadStream(s.vad_path, end_silence_s=end_silence_s, min_speech_s=s.min_speech_s,
                         max_speech_s=s.max_utterance_s, threshold=s.vad_threshold)

    def _open_vad(self, trigger: Trigger, *, at: Optional[float] = None,
                  replay: Optional[np.ndarray] = None) -> None:
        """换一个新的切句器，从监听时间 `at` 开始听。

        `replay` 是要先回补进去的音频（唤醒前的那 2 秒）。回补不会切出完整的句子：
        唤醒档要停顿 2 秒才结句，而回补的音频总共只有 2 秒。
        """
        silence = (self.settings.free_end_silence_s if trigger == "free"
                   else self.settings.wake_end_silence_s)
        self._vad = self._vad_factory(silence)
        self._vad_trigger = trigger
        self._last_speech_t = None
        start = self.t if at is None else at
        replayed = 0 if replay is None else int(replay.size)
        self._vad_t0 = start - replayed / SAMPLE_RATE
        self._discard_stream()
        if trigger == "wake":
            self._start_stream(self._vad_t0, has_wake=True)
            if replayed:
                self._stream_feed(replay)
        if replayed:
            self._vad.feed(replay)

    def _close_vad(self) -> None:
        self._vad = None
        self._discard_stream()

    def _to_idle(self) -> None:
        """回到待机，丢掉正在切的半句。免唤醒模式下立刻重新开始后台切句。"""
        self.state = "idle"
        self._deadline = None
        self._rec_start = None
        self._last_speech_t = None
        if self.mode == "wake_free":
            self._open_vad("free")
        else:
            self._close_vad()

    # ------------------------------------------------------------------
    # 回补缓冲
    # ------------------------------------------------------------------
    def _remember(self, pcm: np.ndarray) -> None:
        self._preroll.append(pcm)
        self._preroll_samples += pcm.size
        limit = int(self.settings.preroll_s * SAMPLE_RATE)
        while self._preroll and self._preroll_samples - self._preroll[0].size >= limit:
            self._preroll_samples -= self._preroll.popleft().size

    def _take_preroll(self) -> np.ndarray:
        if not self._preroll:
            return np.zeros(0, dtype=np.float32)
        audio = np.concatenate(list(self._preroll))
        limit = int(self.settings.preroll_s * SAMPLE_RATE)
        return audio[-limit:] if audio.size > limit else audio

    # ------------------------------------------------------------------
    # 喂音频
    # ------------------------------------------------------------------
    def feed(self, pcm: np.ndarray, *, mode: Mode = "wake", expect_followup: bool = False,
             barge_active: bool = False, suppress: bool = False,
             ) -> tuple[list[Event], list[PendingUtterance]]:
        """喂一块音频。返回（立刻产生的事件, 需要送去识别的句子）。

        识别完成后产生的事件不在这里返回，而是进 `drain_events()`。
        """
        pcm = np.asarray(pcm, dtype=np.float32)
        events: list[Event] = []
        utterances: list[PendingUtterance] = []

        if suppress:
            self.cancel()
            # 手动听写进行中：这块音频不参与任何判断。
            if self.state != "idle" or self._vad is not None:
                self.state = "idle"
                self._deadline = None
                self._rec_start = None
                self._close_vad()
            self._preroll.clear()
            self._preroll_samples = 0
            self._kws.set_active_words(())
            self._kws.feed(pcm)
            self.t += pcm.size / SAMPLE_RATE
            self._followup_seen = expect_followup
            return events, utterances

        if mode != self.mode or (mode == "wake_free" and self.state == "idle" and self._vad is None):
            self.mode = mode
            if self.state == "idle":
                self._to_idle()

        # 追问窗口：只在“小峰开始等回答”的那一刻开一次。
        if expect_followup and not self._followup_seen and self.state == "idle":
            self.state = "listening"
            self._deadline = self.t + self.settings.followup_window_s
            self._open_vad("followup")
            self._rec_start = None
        elif (not expect_followup and self.state == "listening"
              and self._vad_trigger == "followup" and self._awaiting == 0):
            # 用户改用别的方式回答了，不再等这一句。
            self._to_idle()
        self._followup_seen = expect_followup

        # 1) 唤醒词与打断词
        self._kws.set_active_words(self._active_words(barge_active))
        hits = self._kws.feed(pcm)
        chunk_start = self.t
        self.t += pcm.size / SAMPLE_RATE
        for hit in hits:
            if hit.kind == "wake":
                if self.state != "idle":
                    continue  # 正在听的时候再说唤醒词，不重开
                events.append({"type": "wake", "keyword": hit.keyword, "t": round(self.t, 3)})
                self.state = "listening"
                self._deadline = self.t + self.settings.wake_timeout_s
                self._rec_start = None
                # 回补的是这一块之前的音频；这一块本身在下面第 3 步正常喂进去。
                self._open_vad("wake", at=chunk_start, replay=self._take_preroll())
            else:
                if self.state == "idle" and not barge_active:
                    continue
                events.append({"type": "barge", "keyword": hit.keyword, "t": round(self.t, 3)})
                self.cancel()
                if self.state != "idle" or self._vad is not None:
                    self._to_idle()  # 连同正在切的这一句一起丢掉

        self._remember(pcm)

        # 2) 时限
        if self.state == "listening" and self._deadline is not None and self.t >= self._deadline:
            if self._awaiting == 0:
                events.append({"type": "dropped", "reason": "wake_timeout"})
                self._to_idle()
            else:
                # 还有句子在识别，结果回来之前不下结论。
                self._deadline = None
        elif (self.state == "recording" and self._rec_start is not None
              and self.t - self._rec_start >= self.settings.max_utterance_s and self._vad is not None):
            self._collect(self._vad.flush(), events, utterances, forced=True)

        # 3) 切句
        if self._vad is not None:
            segments = self._vad.feed(pcm)
            if self._vad_trigger == "wake":
                if self._stream_id is None and (self._vad.in_speech or segments):
                    self._start_stream(chunk_start)
                self._stream_feed(pcm)
            self._collect(segments, events, utterances)
        if self._vad is not None and self._vad_trigger != "free":
            speaking = self._vad.in_speech
            if speaking:
                self._last_speech_t = self.t
            if self.state == "listening" and speaking:
                self.state = "recording"
                self._deadline = None
                self._rec_start = self.t
            elif (self.state == "recording" and not speaking and self._last_speech_t is not None
                  and self.t - self._last_speech_t > _TOO_SHORT_GRACE_S):
                # 切句器已经结束了这一段，却没有交出句子：声音太短，被当成杂音丢掉了。
                # （切出了句子的话，上面的 _collect 已经把状态改回 listening，走不到这里。）
                events.append({"type": "dropped", "reason": "too_short"})
                self._discard_stream()
                self.state = "listening"
                self._rec_start = None
                self._deadline = self.t + self.settings.wake_timeout_s
        return events, utterances

    def _active_words(self, barge_active: bool) -> list[str]:
        if self.state != "idle":
            return self._barge_words
        return self._wake_words + (self._barge_words if barge_active else [])

    def _collect(self, segments: list[VadSegment], events: list[Event],
                 utterances: list[PendingUtterance], *, forced: bool = False) -> None:
        for segment in segments:
            trigger = self._vad_trigger
            start = self._vad_t0 + segment.start_s
            key = self._stream_id if trigger == "wake" else None
            if self._pending_audio_s + segment.duration_s > self.settings.max_pending_audio_s:
                events.append({"type": "dropped", "reason": "overflow",
                               "seconds": round(segment.duration_s, 2)})
                self._discard_stream()
            else:
                self._pending_audio_s += segment.duration_s
                if trigger != "free":
                    self._awaiting += 1
                utterance = PendingUtterance(
                    id=key or uuid.uuid4().hex[:12], trigger=trigger, wav=segment.wav,
                    t_start=round(start, 3), t_end=round(start + segment.duration_s, 3),
                )
                self._pending[utterance.id] = utterance
                utterances.append(utterance)
                if trigger == "wake":
                    self._stream_feed(np.zeros(0, dtype=np.float32), final=True)
                    events.append({"type": "speech_end", "id": utterance.id, "trigger": "wake",
                                   "tEnd": utterance.t_end,
                                   "silenceMs": max(0, round((self.t - utterance.t_end) * 1000))})
            if trigger != "free":
                # 这一句说完了。要不要回待机，等识别结果回来再定（见模块说明第 2 条）。
                self.state = "listening"
                self._rec_start = None
                self._deadline = self.t + self.settings.wake_timeout_s
        if forced and self.state == "recording":
            self.state = "listening"
            self._rec_start = None
            self._deadline = self.t + self.settings.wake_timeout_s

    # ------------------------------------------------------------------
    # 识别结果回来
    # ------------------------------------------------------------------
    def context_for(self, utterance: PendingUtterance) -> str:
        """免唤醒判断的前文：这一句之前、最近 30 秒内听到的上一句。"""
        for heard_at, text in reversed(self._heard):
            if 0.0 <= utterance.t_start - heard_at <= self.settings.context_window_s:
                return text
            if heard_at < utterance.t_start:
                break
        return ""

    def complete(self, utterance: PendingUtterance, text: str, *, asr_ms: float,
                 p_command: Optional[float] = None, is_command: Optional[bool] = None,
                 failed: bool = False) -> None:
        """一句识别完了。`text` 是识别出的原话（没去唤醒词）。"""
        if not self.owns(utterance):
            return
        self._pending.pop(utterance.id)
        self._partial_text.pop(utterance.id, None)
        self._outbox = [event for event in self._outbox
                        if not (event["type"] == "partial" and event["id"] == utterance.id)]
        self._pending_audio_s = max(0.0, self._pending_audio_s - utterance.duration_s)
        triggered = utterance.trigger != "free"
        if triggered:
            self._awaiting = max(0, self._awaiting - 1)
        heard = (text or "").strip()

        if failed or not heard:
            if triggered:
                self._outbox.append({"type": "dropped", "reason": "empty"})
                self._settle_after_trigger(got_command=False)
            return

        self._heard.append((utterance.t_end, heard))

        if utterance.trigger == "free":
            if not is_command:
                self._outbox.append({"type": "dropped", "reason": "not_command"})
                return
            body = strip_wake_prefix(heard, self._wake_words)
            if not body:
                return
            self._outbox.append(self._utterance_event(utterance, body, asr_ms, p_command))
            return

        body = (strip_wake_anywhere(heard, self._wake_words) if utterance.trigger == "wake"
                else strip_wake_prefix(heard, self._wake_words))
        if not body:
            # 只听到唤醒词：不算一句，继续等。
            self._settle_after_trigger(got_command=False)
            return
        self._outbox.append(self._utterance_event(utterance, body, asr_ms, None))
        self._settle_after_trigger(got_command=True)

    def _settle_after_trigger(self, *, got_command: bool) -> None:
        if self.state != "listening" or self._awaiting > 0:
            return  # 用户又开口了，或者还有句子没识别完
        if got_command:
            self._to_idle()
        else:
            self._deadline = self.t + self.settings.wake_timeout_s

    @staticmethod
    def _utterance_event(utterance: PendingUtterance, text: str, asr_ms: float,
                         p_command: Optional[float]) -> Event:
        event: Event = {
            "type": "utterance", "id": utterance.id, "trigger": utterance.trigger, "text": text,
            "tStart": utterance.t_start, "tEnd": utterance.t_end, "asrMs": round(asr_ms),
        }
        if p_command is not None:
            event["pCommand"] = round(float(p_command), 6)
        return event

    def drain_events(self) -> list[Event]:
        events, self._outbox = self._outbox, []
        return events
