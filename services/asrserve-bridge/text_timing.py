"""Punctuation and word-level timing for RK transcripts.

SenseVoice already emits punctuation, but it does not emit token timestamps and
its punctuation can be uneven after sliding-window decoding. Intel's build solves
both with OpenVINO models (CT-Transformer punctuation, Qwen3 forced alignment),
which do not exist on aarch64. This module provides the RK equivalents:

* punctuation — optional sherpa-onnx CT-Transformer refinement, no new runtime;
* timing      — deterministic clause/character distribution inside each utterance.

The timing is an approximation, not a forced alignment: within one utterance the
characters are assumed to be spoken at a uniform rate. That is enough for karaoke
highlighting, export timestamps and "jump to this sentence", and it needs no model.
If a real aligner is added later, only `utterance_timing` has to change — callers
consume `clauses` and `token_times_ms` either way.

Environment:
    MEETINGBOX_PUNCT_MODEL  path to a sherpa-onnx CT-Transformer punctuation model
"""

import os

SENTENCE_END = "。！？!?…"
CLAUSE_END = SENTENCE_END + "，,；;：:、"
PUNCTUATION = CLAUSE_END + "“”‘’（）()《》<>\"'—-· "
CLAUSE_WEIGHT = 0.35   # punctuation takes time but carries no speech
MIN_CHAR_WEIGHT = 1.0


def _char_weight(char):
    return CLAUSE_WEIGHT if char in PUNCTUATION else MIN_CHAR_WEIGHT


def split_clauses(text):
    """Split into display clauses, keeping the trailing delimiter on each clause."""
    clauses = []
    current = []
    for char in text:
        current.append(char)
        if char in CLAUSE_END:
            clause = "".join(current).strip()
            if clause:
                clauses.append(clause)
            current = []
    tail = "".join(current).strip()
    if tail:
        clauses.append(tail)
    return clauses or ([text.strip()] if text.strip() else [])


def ensure_ending(text):
    """Guarantee a sentence-final mark so the UI does not show a dangling clause."""
    text = text.strip()
    if not text:
        return text
    if text[-1] in SENTENCE_END:
        return text
    if text[-1] in CLAUSE_END:
        return text[:-1] + "。"
    return text + "。"


def utterance_timing(text, start_ms, end_ms):
    """Distribute [start_ms, end_ms] over clauses and characters.

    Returns `{"clauses": [{"text", "start_ms", "end_ms"}], "token_times_ms": [...]}`.
    """
    clauses = split_clauses(text)
    if not clauses or end_ms <= start_ms:
        return {"clauses": [], "token_times_ms": []}

    weights = [max(1.0, sum(_char_weight(char) for char in clause)) for clause in clauses]
    total = sum(weights)
    span = end_ms - start_ms
    cursor = start_ms
    result = []
    for clause, weight in zip(clauses, weights):
        duration = span * weight / total
        clause_end = min(end_ms, cursor + duration)
        result.append(
            {"text": clause, "start_ms": int(round(cursor)), "end_ms": int(round(clause_end))}
        )
        cursor = clause_end
    result[-1]["end_ms"] = end_ms

    # Character-level times for the whole utterance, punctuation included.
    text_length = max(1, len(text))
    step = span / text_length
    token_times = [int(round(start_ms + step * index)) for index in range(text_length)]
    return {"clauses": result, "token_times_ms": token_times}


class Punctuator:
    """Optional sherpa-onnx CT-Transformer punctuation; silent no-op when absent."""

    def __init__(self, model_path=None, num_threads=2):
        self.model_path = model_path or os.getenv("MEETINGBOX_PUNCT_MODEL", "").strip()
        self.num_threads = num_threads
        self._impl = None
        self._loaded = False
        self._reason = None

    @property
    def available(self):
        self._load()
        return self._impl is not None

    @property
    def reason(self):
        self._load()
        return self._reason

    def _load(self):
        if self._loaded:
            return
        self._loaded = True
        if not self.model_path:
            self._reason = "MEETINGBOX_PUNCT_MODEL not set"
            return
        if not os.path.exists(self.model_path):
            self._reason = f"punctuation model missing: {self.model_path}"
            return
        try:
            import sherpa_onnx  # noqa: PLC0415 - optional, device-only dependency

            config = sherpa_onnx.OfflinePunctuationConfig(
                model=sherpa_onnx.OfflinePunctuationModelConfig(
                    ct_transformer=self.model_path, num_threads=self.num_threads
                )
            )
            self._impl = sherpa_onnx.OfflinePunctuation(config)
        except Exception as error:  # pragma: no cover - depends on device runtime
            self._reason = f"sherpa-onnx punctuation unavailable: {type(error).__name__}"
            self._impl = None

    def process(self, text):
        """Add punctuation when the model is present; always guarantee an ending."""
        text = (text or "").strip()
        if not text:
            return text
        if self.available:
            try:
                refined = self._impl.add_punct(text).strip()
                if refined:
                    return ensure_ending(refined)
            except Exception:  # pragma: no cover - device runtime behaviour
                pass
        return ensure_ending(text)
