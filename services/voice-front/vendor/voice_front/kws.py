"""唤醒词与打断词：sherpa-onnx Zipformer KWS 的流式封装。

移植自 peaknode-voice-demo 的 `peaknode/speech/kws.py`，改动只有两处：
模型（`KwsEngine`）与每路监听各自的流（`KwsStream`）拆开，多个监听共用一份模型；
不再依赖 demo 的配置和契约类型。

下面几条都是实测踩出来的，改动前先读：

1. 中文关键词要用 `text2token` 转成拼音 token，`ppinyin` 模式同时需要
   `sentencepiece` 和 `pypinyin`。
2. `text2token` 遇到查不到表的字只往 stderr 打一句日志，然后**少返回一条**，不抛异常。
   不校验条数的话，那个词会从此静默失效。
3. keywords.txt 必须是无 BOM 的 UTF-8、LF 换行。带 BOM 时 sherpa-onnx 会静默产出
   空的关键词表；CRLF 会让 `@原文` 末尾多一个 `\\r`。
4. 行语法 `<token…> [:boost] [#threshold] @原文`，冒号、井号与数字之间不能有空格。
5. `get_result()` 命中后必须 `reset_stream()`，否则同一个词会反复命中。
6. 离线回放一段音频时要在结尾补一点静音再抽干，否则结尾处的词检不出来（`flush()`）。
"""
from __future__ import annotations

import hashlib
import os
import tempfile
from dataclasses import dataclass
from functools import lru_cache
from pathlib import Path
from typing import Any, Iterable, Literal, Optional, Sequence

import numpy as np

from .settings import SAMPLE_RATE
from .wakeword_config import InvalidWakeWord, check_barge_conflicts, normalize_wake_word

__all__ = ["KwsEngine", "KwsStream", "KwsHit", "KwsUnavailable", "encode_keywords"]

KwsKind = Literal["wake", "barge"]
TAIL_PADDING_S = 0.66
_WEIGHT_STEM = "epoch-12-avg-2-chunk-16-left-64"


class KwsUnavailable(RuntimeError):
    """sherpa-onnx 不可用，或 KWS 模型文件缺失。"""


@dataclass(frozen=True)
class KwsHit:
    keyword: str
    kind: KwsKind
    #: 命中时刻：这路流已喂入的音频秒数。
    t: float


@dataclass(frozen=True)
class _Keyword:
    text: str
    kind: KwsKind

    def line(self, tokens: Sequence[str]) -> str:
        return " ".join([*tokens, f"@{self.text}"])


def _import_sherpa() -> Any:
    try:
        import sherpa_onnx  # type: ignore

        return sherpa_onnx
    except Exception as exc:  # noqa: BLE001 - 缺依赖是预期内的一种状态
        raise KwsUnavailable(f"sherpa-onnx 不可用：{exc}") from exc


def encode_keywords(words: Sequence[str], tokens_path: str | Path) -> list[list[str]]:
    """中文关键词 → 拼音 token。任何一个词编码不出来都抛 `KwsUnavailable`（坑 2）。"""
    sherpa = _import_sherpa()
    words = [str(w).strip() for w in words]
    if not words:
        return []
    try:
        encoded = list(sherpa.text2token(list(words), tokens=str(tokens_path), tokens_type="ppinyin") or [])
    except Exception as exc:  # noqa: BLE001
        raise KwsUnavailable(f"关键词转拼音失败：{exc}") from exc
    if len(encoded) != len(words) or any(not toks for toks in encoded):
        raise KwsUnavailable(
            f"关键词里有词表之外的字，只编码出 {len(encoded)}/{len(words)} 个：{words}",
        )
    return [[str(t) for t in toks] for toks in encoded]


def _writable_cache_dir(preferred: Path | None) -> Path:
    candidates = [preferred] if preferred is not None else []
    candidates.append(Path(tempfile.gettempdir()) / "bossai-voice-front")
    for directory in candidates:
        if directory is None:
            continue
        try:
            directory.mkdir(parents=True, exist_ok=True)
            probe = directory / f".probe-{os.getpid()}"
            probe.write_text("x", encoding="utf-8")
            probe.unlink()
            return directory
        except OSError:
            continue
    raise KwsUnavailable("找不到可写的目录来存放关键词表")


def _write_keywords_file(keywords: Sequence[_Keyword], encoded: Sequence[Sequence[str]],
                         out_dir: Path) -> Path:
    body = "\n".join(k.line(toks) for k, toks in zip(keywords, encoded)) + "\n"
    digest = hashlib.sha1(body.encode("utf-8")).hexdigest()[:12]
    path = out_dir / f"keywords-{digest}.txt"
    if not path.exists() or path.read_text(encoding="utf-8") != body:
        # 坑 3：无 BOM、强制 LF
        with path.open("w", encoding="utf-8", newline="\n") as f:
            f.write(body)
    return path


class KwsEngine:
    """一份 KWS 模型 + 关键词表；每路监听用 `create_stream()` 拿自己的流。"""

    def __init__(self, model_dir: str | Path, *,
                 wake_words: Sequence[str], barge_words: Sequence[str] = (),
                 keywords_score: float = 1.0, keywords_threshold: float = 0.25,
                 num_trailing_blanks: int = 0, num_threads: int = 2,
                 cache_dir: str | Path | None = None) -> None:
        self.model_dir = Path(model_dir)
        self.wake_words = [str(w) for w in wake_words]
        self.barge_words = [str(w) for w in barge_words]
        if not self.wake_words and not self.barge_words:
            raise ValueError("至少需要一个唤醒词或打断词")

        missing = [name for name in self._required_files() if not (self.model_dir / name).exists()]
        if missing:
            raise KwsUnavailable(f"KWS 模型文件缺失：{missing}")

        self._keywords = ([_Keyword(w, "wake") for w in self.wake_words]
                          + [_Keyword(w, "barge") for w in self.barge_words])
        tokens = self.model_dir / "tokens.txt"
        encoded = encode_keywords([k.text for k in self._keywords], tokens)
        self._by_tokens = {" ".join(toks): k for k, toks in zip(self._keywords, encoded)}
        self._vocabulary = frozenset(line.split()[0] for line in tokens.read_text(encoding="utf-8").splitlines()
                                     if line.strip())
        self._custom_keyword = lru_cache(maxsize=32)(self._encode_custom_keyword)
        self.keywords_file = _write_keywords_file(
            self._keywords, encoded,
            _writable_cache_dir(Path(cache_dir) if cache_dir is not None else None),
        )

        sherpa = _import_sherpa()
        try:
            self._spotter = sherpa.KeywordSpotter(
                tokens=str(tokens),
                encoder=str(self.model_dir / f"encoder-{_WEIGHT_STEM}.int8.onnx"),
                decoder=str(self.model_dir / f"decoder-{_WEIGHT_STEM}.int8.onnx"),
                joiner=str(self.model_dir / f"joiner-{_WEIGHT_STEM}.int8.onnx"),
                keywords_file=str(self.keywords_file),
                num_threads=int(num_threads),
                keywords_score=float(keywords_score),
                keywords_threshold=float(keywords_threshold),
                num_trailing_blanks=int(num_trailing_blanks),
                provider="cpu",
            )
        except Exception as exc:  # noqa: BLE001
            raise KwsUnavailable(f"KWS 模型加载失败：{exc}") from exc

    @staticmethod
    def _required_files() -> list[str]:
        return ["tokens.txt", *(f"{role}-{_WEIGHT_STEM}.int8.onnx"
                                for role in ("encoder", "decoder", "joiner"))]

    @classmethod
    def files_present(cls, model_dir: str | Path) -> bool:
        base = Path(model_dir)
        return all((base / name).exists() for name in cls._required_files())

    def lookup(self, raw: str) -> Optional[_Keyword]:
        """把 `get_result()` 的返回值映射回关键词（正常是 `@原文`，兜底认 token 串）。"""
        raw = raw.strip()
        for keyword in self._keywords:
            if raw == keyword.text:
                return keyword
        if raw in self._by_tokens:
            return self._by_tokens[raw]
        compact = raw.replace(" ", "")
        for keyword in self._keywords:
            if compact == keyword.text or (keyword.text and keyword.text in raw):
                return keyword
        return None

    def _encode_custom_keyword(self, word: str) -> tuple[str, ...]:
        check_barge_conflicts(word, self.barge_words)
        try:
            encoded = encode_keywords([word], self.model_dir / "tokens.txt")
        except KwsUnavailable as exc:
            raise InvalidWakeWord("唤醒词无法用当前词表编码") from exc
        if (len(encoded) != 1 or not encoded[0]
                or any(not token or token not in self._vocabulary for token in encoded[0])):
            raise InvalidWakeWord("唤醒词无法用当前词表编码")
        tokens = tuple(encoded[0])
        for raw, keyword in self._by_tokens.items():
            if keyword.kind != "barge":
                continue
            shorter, longer = sorted((tokens, tuple(raw.split())), key=len)
            if any(longer[i:i + len(shorter)] == shorter
                   for i in range(len(longer) - len(shorter) + 1)):
                raise InvalidWakeWord("唤醒词不能与打断词同音或相互包含")
        return tokens

    def validate_wake_word(self, word: str) -> str:
        word = normalize_wake_word(word)
        self._custom_keyword(word)
        return word

    def create_stream(self, wake_words: Sequence[str] | None = None) -> "KwsStream":
        if wake_words is None:
            return KwsStream(self)
        words = [self.validate_wake_word(word) for word in wake_words]
        keywords = [_Keyword(word, "wake") for word in words]
        encoded = [self._custom_keyword(word) for word in words]
        lookup = {keyword.text: keyword for keyword in self._keywords if keyword.kind == "barge"}
        lookup.update({raw: keyword for raw, keyword in self._by_tokens.items() if keyword.kind == "barge"})
        lookup.update({keyword.text: keyword for keyword in keywords})
        custom_by_tokens = {" ".join(tokens): keyword for keyword, tokens in zip(keywords, encoded)}
        lookup.update(custom_by_tokens)
        # sherpa 的默认词后加入，同音路径可能返回默认标签；只映射完全相同的 token。
        for raw, default in self._by_tokens.items():
            if default.kind == "wake" and raw in custom_by_tokens:
                lookup[default.text] = custom_by_tokens[raw]
        # sherpa 追加这些词，模型的默认词仍在；每流的 lookup/active_words 再严格过滤。
        extra = " / ".join(keyword.line(tokens) for keyword, tokens in zip(keywords, encoded))
        return KwsStream(self, wake_words=words, lookup=lookup, extra_keywords=extra)


class KwsStream:
    """一路监听的 KWS 状态。只能在同一个线程里顺序调用。"""

    def __init__(self, engine: KwsEngine, *, wake_words: Sequence[str] | None = None,
                 lookup: dict[str, _Keyword] | None = None, extra_keywords: str = "") -> None:
        self._engine = engine
        self._spotter = engine._spotter
        self._stream: Any = None
        self._active = frozenset(engine.wake_words if wake_words is None else wake_words)
        self._known = self._active | frozenset(engine.barge_words)
        self._lookup = lookup
        self._extra_keywords = extra_keywords
        self._t = 0.0

    @property
    def t(self) -> float:
        return self._t

    @property
    def active_words(self) -> frozenset[str]:
        return self._active

    def set_active_words(self, words: Iterable[str]) -> None:
        """只让给出的这些词参与命中；其余的词即使模型检出也丢弃。

        词集变了就重建流：否则上一种状态下已经解码到一半的词，会在新状态下冒出来。
        未登记的词被忽略。空集合表示什么都不听（省算力，时间轴照常推进）。
        """
        wanted = frozenset(str(w) for w in words) & self._known
        if wanted != self._active:
            self._active = wanted
            self._stream = None

    def feed(self, pcm: np.ndarray) -> list[KwsHit]:
        """喂一块音频，返回这一块里的全部命中（按出现顺序）。"""
        pcm = np.asarray(pcm, dtype=np.float32)
        if not self._active:
            self._stream = None
            self._t += pcm.size / SAMPLE_RATE
            return []
        if self._stream is None:
            self._stream = (self._spotter.create_stream(keywords=self._extra_keywords)
                            if self._extra_keywords else self._spotter.create_stream())
        self._stream.accept_waveform(SAMPLE_RATE, pcm)
        self._t += pcm.size / SAMPLE_RATE
        return self._drain()

    def _drain(self) -> list[KwsHit]:
        hits: list[KwsHit] = []
        while self._spotter.is_ready(self._stream):
            self._spotter.decode_stream(self._stream)
            raw = self._spotter.get_result(self._stream)
            if raw == "":
                continue
            self._spotter.reset_stream(self._stream)  # 坑 5
            keyword = (self._engine.lookup(raw) if self._lookup is None
                       else self._lookup.get(raw.strip()))
            if keyword is None or keyword.text not in self._active:
                continue
            hits.append(KwsHit(keyword=keyword.text, kind=keyword.kind, t=round(self._t, 3)))
        return hits

    def flush(self) -> list[KwsHit]:
        """离线回放收尾：补一点静音再抽干（坑 6）。实时监听不需要调用。"""
        if self._stream is None or not self._active:
            return []
        padding = np.zeros(int(TAIL_PADDING_S * SAMPLE_RATE), dtype=np.float32)
        self._stream.accept_waveform(SAMPLE_RATE, padding)
        self._t += padding.size / SAMPLE_RATE
        return self._drain()

    def reset(self) -> None:
        self._stream = None
        self._t = 0.0
