"""唤醒句子的增量识别：复用已加载的 Paraformer，每路有界队列、每句独立 cache。"""
from __future__ import annotations

import asyncio
import logging
from collections import deque
from dataclasses import dataclass, field
from typing import Any, Callable

import numpy as np

from .settings import SAMPLE_RATE

log = logging.getLogger("bossai.voice_front")
# 与 sidecar 听写的 Paraformer 配置一致；600ms 一个推理步。
CHUNK_SIZE = [0, 10, 5]
CHUNK_SAMPLES = CHUNK_SIZE[1] * 960
MAX_QUEUED_SAMPLES = SAMPLE_RATE * 4
MAX_QUEUED_CHUNKS = 32


@dataclass
class StreamAudio:
    id: str
    pcm: np.ndarray
    t_start: float
    t_end: float
    final: bool = False
    has_wake: bool = False


@dataclass
class _Sentence:
    cache: dict[str, Any] = field(default_factory=dict)
    text: str = ""
    cancelled: bool = False
    done: asyncio.Event = field(default_factory=asyncio.Event)


class WakeStreaming:
    """方法由事件循环调用；线程只访问正在处理的句子 cache，取消后线程退出再清理。"""

    def __init__(self, engine: Any, *, valid: Callable[[str], bool],
                 emit: Callable[[StreamAudio, str], None],
                 clean_text: Callable[[str], str], encoder_look_back: int = 4,
                 decoder_look_back: int = 1) -> None:
        self.engine = engine
        self.valid = valid
        self.emit = emit
        self.clean_text = clean_text
        self.encoder_look_back = encoder_look_back
        self.decoder_look_back = decoder_look_back
        self.queue: deque[StreamAudio] = deque()
        self.queued_samples = 0
        self.sentences: dict[str, _Sentence] = {}
        self.disabled: set[str] = set()
        self.task: asyncio.Task[None] | None = None
        self._inflight: str | None = None

    def reconcile(self) -> None:
        for key in list(self.sentences):
            if not self.valid(key):
                self.cancel(key)
        self.disabled = {key for key in self.disabled if self.valid(key)}

    def cancel(self, key: str) -> None:
        sentence = self.sentences.get(key)
        if sentence is not None:
            sentence.cancelled = True
            sentence.done.set()
            if self._inflight != key:
                sentence.cache.clear()
                self.sentences.pop(key, None)
        self.queue = deque(item for item in self.queue if item.id != key)
        self.queued_samples = sum(item.pcm.size for item in self.queue)

    async def finish(self, key: str) -> None:
        """结句先 flush；过慢的临时通道最多额外占一个推理步，不拖住 final。"""
        sentence = self.sentences.get(key)
        if sentence is not None:
            try:
                await asyncio.wait_for(sentence.done.wait(), timeout=0.6)
            except asyncio.TimeoutError:
                self.cancel(key)

    def close(self) -> None:
        for key in list(self.sentences):
            self.cancel(key)
        self.disabled.clear()

    def submit(self, item: StreamAudio) -> None:
        if item.id in self.disabled or not self.valid(item.id):
            return
        if (len(self.queue) >= MAX_QUEUED_CHUNKS
                or self.queued_samples + item.pcm.size > MAX_QUEUED_SAMPLES):
            # 丢任意中间音频会破坏 cache 连续性：整句停用 partial，离线 final 继续。
            self.disabled.add(item.id)
            self.cancel(item.id)
            return
        self.sentences.setdefault(item.id, _Sentence())
        self.queue.append(item)
        self.queued_samples += item.pcm.size
        if self.task is None or self.task.done():
            self.task = asyncio.create_task(self._run())

    async def _run(self) -> None:
        while self.queue:
            item = self.queue.popleft()
            self.queued_samples -= item.pcm.size
            sentence = self.sentences.get(item.id)
            if sentence is None or sentence.cancelled or not self.valid(item.id):
                self.cancel(item.id)
                continue
            self._inflight = item.id
            try:
                # 不在这里加载模型，也不调用 generate（它会改 AutoModel 的共享 kwargs）。
                if not self.engine.loaded or self.engine.mode != "streaming":
                    raise RuntimeError("streaming model unavailable")
                model = await self.engine.model()
                if sentence.cancelled or not self.valid(item.id):
                    continue

                def run() -> str:
                    options = dict(model.kwargs)
                    options.pop("cache", None)
                    result = model.inference(
                        item.pcm, model=model.model, kwargs=options, cache=sentence.cache,
                        is_final=item.final, chunk_size=list(CHUNK_SIZE),
                        encoder_chunk_look_back=self.encoder_look_back,
                        decoder_chunk_look_back=self.decoder_look_back,
                        disable_pbar=True,
                    )
                    rows = result if isinstance(result, list) else [result]
                    return "".join(str(row.get("text", "")) for row in rows if isinstance(row, dict))

                delta = await asyncio.to_thread(run)
                if not sentence.cancelled and self.valid(item.id):
                    # Paraformer 返回本块的增量；重复字也必须保留，不做相邻文本去重。
                    sentence.text += delta
                    text = self.clean_text(sentence.text)
                    if text:
                        self.emit(item, text)
            except Exception as exc:  # noqa: BLE001 - partial 失败不得影响离线 final
                log.warning("voice_front streaming failed: %s", type(exc).__name__)
                self.disabled.add(item.id)
                self.cancel(item.id)
            finally:
                self._inflight = None
                if item.final or sentence.cancelled:
                    sentence.cache.clear()
                    sentence.done.set()
                    self.sentences.pop(item.id, None)
