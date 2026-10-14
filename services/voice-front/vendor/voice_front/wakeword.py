"""把转写开头的唤醒词去掉，只留指令正文。

移植自 peaknode-voice-demo 的 `peaknode/orchestrator/wakeword.py`，逻辑未改。

为什么需要：唤醒词不属于指令，带着它交给下游会干扰理解。语音识别还常把唤醒词
识别成同音字（小峰 → 小风 / 小锋），所以按**拼音**比对，不按字面。

只处理句首：前面允许有几个语气词或标点（「嗯，小峰小峰，帮我…」），其余位置不动。
"""
from __future__ import annotations

from typing import Iterable, Sequence

#: 句首可以跳过的语气词与标点
_LEAD = set("嗯呃啊哦喂哎诶欸那个你好 \t，,。.、！!？?～~…")
#: 唤醒词后面紧跟的语气词与标点（一并去掉）
_TAIL = set("啊呀呢吧嘛哈诶 \t，,。.、！!？?～~…：:")
#: 比对时忽略的标点（ASR 可能在叠词中间加逗号：「小峰，小峰，」）
_PUNCT = set(" \t，,。.、！!？?～~…：:")

#: 句首最多跳过几个字符去找唤醒词
MAX_LEAD = 4


def _pinyin(s: str) -> list[str]:
    """无声调拼音；没装 pypinyin 时退化为逐字比较（只能匹配字面相同的情况）。"""
    try:
        from pypinyin import lazy_pinyin
    except ImportError:  # pragma: no cover - 依赖缺失时的兜底
        return list(s)
    return [_fuzzy(p) for p in lazy_pinyin(s)]


def _fuzzy(p: str) -> str:
    """抹平 ASR 最常见的两类混淆：前后鼻音（feng/fen）、平翘舌（zh/z）。"""
    p = p.lower()
    for a, b in (("zh", "z"), ("ch", "c"), ("sh", "s")):
        if p.startswith(a):
            p = b + p[len(a):]
            break
    if p.endswith("ng"):
        p = p[:-1]
    return p


def _candidates(wake_words: Iterable[str]) -> list[tuple[str, bool]]:
    """[(词, 是否要求后面紧跟停顿)]，长的排前面。

    完整唤醒词直接匹配。叠词（小峰小峰）另加它的一半（小峰）当称呼用，
    但一半只有两个音节，同音词很多（小分队、小冯），所以要求后面紧跟标点、
    语气词或句尾才算称呼：「小峰，把文件发我」去掉，「小分队今天到」不动。
    """
    out: dict[str, bool] = {}
    for w in wake_words:
        w = "".join(ch for ch in str(w or "") if ch not in _PUNCT)
        if not w:
            continue
        out[w] = False
        half = len(w) // 2
        if len(w) % 2 == 0 and half >= 2 and w[:half] == w[half:]:
            out.setdefault(w[:half], True)
    return sorted(out.items(), key=lambda kv: len(kv[0]), reverse=True)


def _match_at(text: str, pos: int, target: Sequence[str]) -> int:
    """从 pos 起取与唤醒词等长的字（跳过标点）比拼音。匹配返回结束位置，否则 -1。"""
    j, got = pos, []
    while j < len(text) and len(got) < len(target):
        ch = text[j]
        j += 1
        if ch in _PUNCT:
            continue
        got.append(ch)
    if len(got) < len(target):
        return -1
    return j if _pinyin("".join(got)) == list(target) else -1


def strip_wake_prefix(text: str, wake_words: Iterable[str]) -> str:
    """去掉句首的唤醒词（含同音误识别），返回指令正文；没有唤醒词就原样返回。

    只含唤醒词时返回空串，由调用方决定怎么处理（编排器会继续等指令）。
    """
    t = (text or "").strip()
    cands = _candidates(wake_words)
    if not t or not cands:
        return t

    pos = 0
    while True:
        for w, need_pause in cands:
            end = _match_at(t, pos, _pinyin(w))
            if end < 0:
                continue
            rest = t[end:]
            if need_pause and rest and rest[0] not in _TAIL:
                continue
            while rest and rest[0] in _TAIL:
                rest = rest[1:]
            return rest.strip()
        if pos >= MAX_LEAD or pos >= len(t) or t[pos] not in _LEAD:
            return t
        pos += 1


#: 唤醒触发的句子里，最多往后找这么多个字符去定位唤醒词
MAX_WAKE_START = 12


def strip_wake_anywhere(text: str, wake_words: Iterable[str], *, require_wake: bool = False) -> str:
    """去掉**唤醒触发**的那一句里的唤醒词，以及它前面的内容。

    这是并入 sidecar 时新增的，demo 里没有。唤醒时会把之前 2 秒的音频一并交给切句
    （见 `session.py`），所以这一句的开头可能带着唤醒词之前别人说的半句话：
    「那就这样。小峰小峰，帮我查库存」。此时唤醒词不在句首，`strip_wake_prefix` 去不掉。

    既然已知这一句是被唤醒词触发的，就在开头一小段里找**第一处**完整的唤醒词
    （按拼音，容忍同音字和中间的标点），连同它前面的内容一起去掉。用第一处而不是
    最后一处，是因为指令里可能再次提到这个词：「小峰小峰，把小峰小峰这个名字改掉」。
    找不到时退回只处理句首的做法；require_wake 用于带预录音的临时稿，找不到就暂不输出。
    """
    t = (text or "").strip()
    wake_words = tuple(wake_words)
    full = [w for w, need_pause in _candidates(wake_words) if not need_pause]
    if not t or not full:
        return "" if require_wake else strip_wake_prefix(t, wake_words)
    for pos in range(0, min(len(t), MAX_WAKE_START) + 1):
        if pos < len(t) and t[pos] in _PUNCT:
            continue
        for w in full:
            end = _match_at(t, pos, _pinyin(w))
            if end < 0:
                continue
            rest = t[end:]
            while rest and rest[0] in _TAIL:
                rest = rest[1:]
            # 叠着说了两遍（「小峰小峰小峰小峰」）时，剩下的部分开头还是唤醒词
            return strip_wake_prefix(rest.strip(), wake_words)
    return "" if require_wake else strip_wake_prefix(t, wake_words)


def strip_wake_partial(text: str, wake_words: Iterable[str], *, has_wake: bool) -> str:
    """临时稿只放行已确认的正文；唤醒词未识别完整时继续等待。

    首句带着唤醒预录音，必须先找到完整唤醒词。只说唤醒词结句后再次开口的流
    不带该预录音，可以直接显示正文；两者都暂留句首尚未说完的唤醒词片段。
    """
    wake_words = tuple(wake_words)
    body = (strip_wake_anywhere(text, wake_words, require_wake=True) if has_wake
            else strip_wake_prefix(text, wake_words))
    targets = [_pinyin(w) for w, need_pause in _candidates(wake_words) if not need_pause]
    pos = 0
    while body:
        syllables = _pinyin("".join(ch for ch in body[pos:] if ch not in _PUNCT))
        if any(syllables and len(syllables) < len(target)
               and syllables == target[:len(syllables)] for target in targets):
            return ""
        if pos >= MAX_LEAD or pos >= len(body) or body[pos] not in _LEAD:
            break
        pos += 1
    return body


__all__ = ["strip_wake_prefix", "strip_wake_anywhere", "strip_wake_partial", "MAX_LEAD", "MAX_WAKE_START"]
