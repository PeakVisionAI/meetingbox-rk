"""自定义唤醒词的输入与打断词冲突校验，不修改全局设置。"""
from __future__ import annotations

import re
from typing import Sequence


class InvalidWakeWord(ValueError):
    """用户提供的词不合法或无法用于当前关键词词表。"""


def normalize_wake_word(word: str) -> str:
    normalized = word.strip()
    if re.fullmatch(r"[㐀-䶿一-鿿]{2,4}", normalized) is None:
        raise InvalidWakeWord("唤醒词必须为 2–4 个汉字")
    return normalized


def check_barge_conflicts(word: str, barge_words: Sequence[str]) -> None:
    from pypinyin import lazy_pinyin

    syllables = tuple(lazy_pinyin(word))
    for barge in barge_words:
        other = tuple(lazy_pinyin(barge))
        if not other:
            continue
        shorter, longer = sorted((syllables, other), key=len)
        if (word in barge or barge in word
                or any(longer[i:i + len(shorter)] == shorter
                       for i in range(len(longer) - len(shorter) + 1))):
            raise InvalidWakeWord("唤醒词不能与打断词同音或相互包含")
