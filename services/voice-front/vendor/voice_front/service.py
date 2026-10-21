"""语音前段的运行时：模型的加载与健康状态、各路监听的登记、识别的后台调度。

分工：`session.py` 只管状态机，不碰模型也不等待；这里负责把它切出来的句子送去
识别（以及免唤醒判断），做完再交回去。识别在后台线程里跑，收音频的请求不等它——
结果随之后某一次 `feed` 的返回带回页面。

日志里不写识别出的文字，只记耗时、长度和状态。
"""
from __future__ import annotations

import asyncio
import importlib.util
import logging
import time
from typing import Any, Awaitable, Callable, Optional

import numpy as np

from .addressee import AddresseeClassifier, AddresseeUnavailable
from .kws import KwsEngine, KwsUnavailable
from .session import Mode, PendingUtterance, VoiceSession
from .settings import VoiceFrontSettings
from .streaming import WakeStreaming
from .vad import VadStream, VadUnavailable
from .wakeword_config import InvalidWakeWord, normalize_wake_word

log = logging.getLogger("bossai.voice_front")

__all__ = ["VoiceFront", "VoiceFrontError"]

Recognizer = Callable[[np.ndarray], Awaitable[str]]
_REQUIRED_MODULES = ("sherpa_onnx", "pypinyin", "sentencepiece")


class VoiceFrontError(RuntimeError):
    """可以直接回给调用方的错误。`code` 是稳定的机器可读标识。"""

    def __init__(self, code: str, message: str, status: int = 409) -> None:
        super().__init__(message)
        self.code = code
        self.status = status


class _Entry:
    def __init__(self, session: VoiceSession, wake_word: str | None = None) -> None:
        self.session = session
        self.wake_word = wake_word
        #: 同一路监听的句子按切出的先后顺序识别，免唤醒判断才能拿到正确的前文。
        self.order = asyncio.Lock()
        self.touched = time.monotonic()
        self.pending = 0
        self.streaming: WakeStreaming | None = None

    def close(self) -> None:
        self.session.cancel()
        if self.streaming is not None:
            self.streaming.close()


class VoiceFront:
    def __init__(self, settings: VoiceFrontSettings, *, recognize: Recognizer,
                 stream_engine: Any = None, clean_text: Callable[[str], str] = str.strip,
                 stream_encoder_look_back: int = 4, stream_decoder_look_back: int = 1) -> None:
        self.settings = settings
        self._recognize = recognize
        self._stream_engine = stream_engine
        self._clean_text = clean_text
        self._stream_encoder_look_back = stream_encoder_look_back
        self._stream_decoder_look_back = stream_decoder_look_back
        self._engine: Optional[KwsEngine] = None
        self._engine_lock = asyncio.Lock()
        self._engine_error: Optional[str] = None
        self._vad_ok = False
        self._addressee = AddresseeClassifier(settings.addressee_dir)
        self._addressee_error: Optional[str] = None
        self._entries: dict[str, _Entry] = {}
        self._tasks: set[asyncio.Task[None]] = set()
        #: 短句识别一次只跑一句，免得几路监听同时把 CPU 占满。
        self._asr_gate = asyncio.Semaphore(1)

    # ------------------------------------------------------------------
    # 可用性
    # ------------------------------------------------------------------
    def _missing_modules(self) -> list[str]:
        return [name for name in _REQUIRED_MODULES if importlib.util.find_spec(name) is None]

    def _unavailable_reason(self) -> Optional[str]:
        if not self.settings.enabled:
            return "disabled"
        if self._missing_modules():
            return "deps_missing"
        if not KwsEngine.files_present(self.settings.kws_dir) or not self.settings.vad_path.exists():
            return "models_missing"
        if self._engine_error is not None:
            # 文件都在却加载不起来（文件损坏、版本不配），对页面来说同样是“模型不可用”。
            return "models_missing"
        return None

    @property
    def addressee_ready(self) -> bool:
        return (self._addressee_error is None
                and AddresseeClassifier.files_present(self.settings.addressee_dir))

    def health(self) -> dict[str, Any]:
        reason = self._unavailable_reason()
        loaded = self._engine is not None
        payload: dict[str, Any] = {
            "available": reason is None,
            "kws": {
                "loaded": loaded,
                "wakeWords": list(self.settings.wake_words),
                "bargeWords": list(self.settings.barge_words),
            },
            "vad": {"loaded": loaded and self._vad_ok},
            "addressee": {"loaded": self._addressee.loaded, "available": self.addressee_ready},
        }
        if reason is not None:
            payload["reason"] = reason
        if self._addressee.threshold is not None:
            payload["addressee"]["threshold"] = self._addressee.threshold
        return payload

    async def warm(self) -> None:
        """提前把模型加载好。任何一步失败都只记状态，不抛。"""
        if self._unavailable_reason() is not None:
            return
        try:
            await self._ensure_engine()
        except VoiceFrontError:
            return
        if self.addressee_ready and not self._addressee.loaded:
            try:
                await asyncio.to_thread(self._addressee.load)
            except AddresseeUnavailable as exc:
                self._addressee_error = str(exc)
                log.warning("voice_front addressee unavailable: %s", exc)

    async def _ensure_engine(self) -> KwsEngine:
        if self._engine is not None:
            return self._engine
        reason = self._unavailable_reason()
        if reason is not None:
            raise VoiceFrontError(reason, f"语音前段不可用：{reason}", status=503)
        async with self._engine_lock:
            if self._engine is not None:
                return self._engine
            s = self.settings

            def load() -> KwsEngine:
                engine = KwsEngine(
                    s.kws_dir, wake_words=s.wake_words, barge_words=s.barge_words,
                    keywords_score=s.keywords_score, keywords_threshold=s.keywords_threshold,
                    num_trailing_blanks=s.num_trailing_blanks,
                )
                # 顺手验证切句模型能加载，之后每路监听各建各的。
                VadStream(s.vad_path, end_silence_s=s.wake_end_silence_s,
                          min_speech_s=s.min_speech_s, max_speech_s=s.max_utterance_s,
                          threshold=s.vad_threshold)
                return engine

            started = time.perf_counter()
            try:
                self._engine = await asyncio.to_thread(load)
            except (KwsUnavailable, VadUnavailable, ValueError) as exc:
                self._engine_error = str(exc)
                log.warning("voice_front engine failed to load: %s", exc)
                raise VoiceFrontError("models_missing", "语音前段模型加载失败", status=503) from exc
            self._vad_ok = True
            log.info("voice_front engine loaded in %.0f ms", (time.perf_counter() - started) * 1000)
            return self._engine

    # ------------------------------------------------------------------
    # 监听登记
    # ------------------------------------------------------------------
    def _purge(self) -> None:
        now = time.monotonic()
        stale = [key for key, entry in self._entries.items()
                 if now - entry.touched > self.settings.session_idle_ttl_s]
        for key in stale:
            self.reset(key)
        while len(self._entries) >= self.settings.max_sessions:
            oldest = min(self._entries, key=lambda key: self._entries[key].touched)
            self.reset(oldest)

    async def validate_wake_word(self, wake_word: str) -> str:
        """验证不创建监听、不修改默认词；feed 与验证接口共用这一入口。"""
        try:
            word = normalize_wake_word(wake_word)
            engine = await self._ensure_engine()
            return engine.validate_wake_word(word)
        except InvalidWakeWord as exc:
            raise VoiceFrontError("bad_request", str(exc), status=400) from exc

    def _entry(self, session_id: str, engine: KwsEngine, wake_word: str | None = None) -> _Entry:
        entry = self._entries.get(session_id)
        if entry is None or entry.wake_word != wake_word:
            # 先构造成功再替换；旧录音、outbox 和后台结果只归旧 entry 所有。
            session = VoiceSession(session_id, self.settings, engine,
                                   wake_words=None if wake_word is None else (wake_word,))
            if entry is None:
                self._purge()
            else:
                # 配置切换不能让旧序号重新生效。
                session.last_seq = entry.session.last_seq
                entry.close()
            entry = _Entry(session, wake_word)
            self._entries[session_id] = entry
            if self._stream_engine is not None:
                entry.streaming = WakeStreaming(
                    self._stream_engine,
                    valid=lambda key: (self._entries.get(session_id) is entry
                                       and session.accepts_partial(key)),
                    emit=session.partial, clean_text=self._clean_text,
                    encoder_look_back=self._stream_encoder_look_back,
                    decoder_look_back=self._stream_decoder_look_back,
                )
        entry.touched = time.monotonic()
        return entry

    def reset(self, session_id: str) -> bool:
        entry = self._entries.pop(session_id, None)
        if entry is not None:
            entry.close()
        return entry is not None

    # ------------------------------------------------------------------
    # 喂音频
    # ------------------------------------------------------------------
    async def feed(self, *, session_id: str, seq: int, pcm: np.ndarray, mode: Mode,
                   expect_followup: bool = False, barge_active: bool = False,
                   suppress: bool = False, wake_word: str | None = None) -> dict[str, Any]:
        word = None if wake_word is None else await self.validate_wake_word(wake_word)
        engine = await self._ensure_engine()
        if mode == "wake_free" and not suppress:
            if not self.addressee_ready:
                raise VoiceFrontError("addressee_unavailable", "免唤醒判断模型不可用，不能使用免唤醒模式")
            if not self._addressee.loaded:
                try:
                    await asyncio.to_thread(self._addressee.load)
                except AddresseeUnavailable as exc:
                    self._addressee_error = str(exc)
                    raise VoiceFrontError("addressee_unavailable", "免唤醒判断模型加载失败") from exc

        current = self._entries.get(session_id)
        if current is not None and seq <= current.session.last_seq:
            current.touched = time.monotonic()
            return self._response(seq, current, current.session.drain_events())
        entry = self._entry(session_id, engine, word)
        session = entry.session
        session.last_seq = seq

        events, utterances = session.feed(
            pcm, mode=mode, expect_followup=expect_followup,
            barge_active=barge_active, suppress=suppress,
        )
        audio = session.drain_stream_audio()
        if entry.streaming is not None:
            entry.streaming.reconcile()
            for item in audio:
                entry.streaming.submit(item)
            if entry.streaming.task is not None:
                task = entry.streaming.task
                if task not in self._tasks:
                    self._tasks.add(task)
                    task.add_done_callback(self._tasks.discard)
        for utterance in utterances:
            entry.pending += 1
            task = asyncio.create_task(self._process(entry, utterance))
            self._tasks.add(task)
            task.add_done_callback(self._tasks.discard)
        return self._response(seq, entry, session.drain_events() + events)

    @staticmethod
    def _response(seq: int, entry: _Entry, events: list[dict[str, Any]]) -> dict[str, Any]:
        # pending：这一路还有几句在识别。页面用不着；回放工具靠它知道什么时候该等一等
        # （回放比实时快得多，不等的话识别结果会落在音频“之后”）。
        return {"seq": seq, "state": entry.session.state, "events": events,
                "pending": entry.pending}

    async def _process(self, entry: _Entry, utterance: PendingUtterance) -> None:
        try:
            await self._process_in_order(entry, utterance)
        finally:
            entry.pending -= 1
            if entry.streaming is not None:
                entry.streaming.reconcile()

    async def _process_in_order(self, entry: _Entry, utterance: PendingUtterance) -> None:
        session = entry.session
        async with entry.order:
            if self._entries.get(session.session_id) is not entry or not session.owns(utterance):
                return
            if entry.streaming is not None:
                await entry.streaming.finish(utterance.id)
            started = time.perf_counter()
            try:
                async with self._asr_gate:
                    if self._entries.get(session.session_id) is not entry or not session.owns(utterance):
                        return
                    text = await self._recognize(utterance.wav)
            except Exception as exc:  # noqa: BLE001 - 一句识别失败不能拖垮监听
                log.warning("voice_front recognition failed: %s", type(exc).__name__)
                if self._entries.get(session.session_id) is entry:
                    session.complete(utterance, "", asr_ms=(time.perf_counter() - started) * 1000,
                                     failed=True)
                return
            if self._entries.get(session.session_id) is not entry or not session.owns(utterance):
                return
            asr_ms = (time.perf_counter() - started) * 1000
            probability: Optional[float] = None
            is_command: Optional[bool] = None
            if utterance.trigger == "free" and text.strip():
                try:
                    probability = await asyncio.to_thread(
                        self._addressee.score, text, session.context_for(utterance))
                    is_command = self._addressee.is_command(probability)
                except Exception as exc:  # noqa: BLE001 - 判断不了就按“不是指令”处理
                    log.warning("voice_front addressee failed: %s", type(exc).__name__)
                    probability, is_command = None, False
            if self._entries.get(session.session_id) is not entry or not session.owns(utterance):
                return
            session.complete(utterance, text, asr_ms=asr_ms,
                             p_command=probability, is_command=is_command)
            log.debug("voice_front utterance trigger=%s audio=%.2fs asr=%.0fms chars=%d",
                      utterance.trigger, utterance.duration_s, asr_ms, len(text))

    async def drain(self, timeout_s: float = 30.0) -> None:
        """等后台识别全部做完（测试和验收脚本用）。"""
        if self._tasks:
            await asyncio.wait(set(self._tasks), timeout=timeout_s)

    # ------------------------------------------------------------------
    # 只给验收用
    # ------------------------------------------------------------------
    async def addressee_score(self, text: str, context: str = "") -> dict[str, Any]:
        if not self.addressee_ready:
            raise VoiceFrontError("addressee_unavailable", "免唤醒判断模型不可用")
        try:
            await asyncio.to_thread(self._addressee.load)
            started = time.perf_counter()
            probability = await asyncio.to_thread(self._addressee.score, text, context)
            took_ms = (time.perf_counter() - started) * 1000
        except AddresseeUnavailable as exc:
            self._addressee_error = str(exc)
            raise VoiceFrontError("addressee_unavailable", "免唤醒判断模型加载失败") from exc
        return {
            "pCommand": probability,
            "threshold": self._addressee.threshold,
            "isCommand": self._addressee.is_command(probability),
            "ms": round(took_ms, 2),
        }
