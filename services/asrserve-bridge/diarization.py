"""Utterance-level speaker diarization for the RK bridge.

Intel's build streams speaker activity from a torch model that lives outside this
repo. RK has no torch, so we use the sherpa-onnx speaker-embedding API instead —
the same runtime that already serves SenseVoice on the device. One embedding per
closed utterance, then online clustering.

Everything that can be unit-tested is kept free of model dependencies:
`OnlineSpeakerCluster` is pure Python so the assignment policy is verified without
audio, and `SpeakerEmbedder` degrades to "unavailable" when the model or runtime
is missing (the transcript keeps flowing, it just has no speaker labels).

Environment:
    MEETINGBOX_SPEAKER_MODEL      path to a sherpa-onnx speaker embedding model
    MEETINGBOX_SPEAKER_THRESHOLD  cosine threshold for "same speaker" (default 0.62)
    MEETINGBOX_SPEAKER_MAX        maximum distinct speakers (default 8)
"""

import math
import os
from array import array

SAMPLE_RATE = 16000


def _env_float(name, default):
    try:
        value = float(os.getenv(name, ""))
    except ValueError:
        return default
    return value if 0.0 < value <= 1.0 else default


def _env_int(name, default):
    try:
        value = int(os.getenv(name, ""))
    except ValueError:
        return default
    return value if value > 0 else default


def _normalize(vector):
    norm = math.sqrt(sum(value * value for value in vector))
    if norm <= 0:
        return []
    return [value / norm for value in vector]


def cosine(left, right):
    """Cosine similarity of two equal-length vectors; 0.0 when degenerate."""
    if not left or len(left) != len(right):
        return 0.0
    dot = sum(a * b for a, b in zip(left, right))
    left_norm = math.sqrt(sum(value * value for value in left))
    right_norm = math.sqrt(sum(value * value for value in right))
    if left_norm <= 0 or right_norm <= 0:
        return 0.0
    return dot / (left_norm * right_norm)


class OnlineSpeakerCluster:
    """Greedy online clustering: the meeting is short, so no re-clustering pass."""

    def __init__(self, threshold=None, max_speakers=None, min_utterance_ms=800):
        self.threshold = threshold if threshold is not None else _env_float("MEETINGBOX_SPEAKER_THRESHOLD", 0.62)
        self.max_speakers = max_speakers or _env_int("MEETINGBOX_SPEAKER_MAX", 8)
        self.min_utterance_ms = min_utterance_ms
        self.centroids = []
        self.counts = []

    def assign(self, embedding, duration_ms):
        """Return a zero-based speaker index, or None when the sample is too short."""
        if duration_ms < self.min_utterance_ms:
            return None
        vector = _normalize(embedding or [])
        if not vector:
            return None

        best_index, best_score = None, -1.0
        for index, centroid in enumerate(self.centroids):
            score = cosine(vector, centroid)
            if score > best_score:
                best_index, best_score = index, score

        if best_index is not None and best_score >= self.threshold:
            self._update(best_index, vector)
            return best_index
        if len(self.centroids) < self.max_speakers:
            self.centroids.append(vector)
            self.counts.append(1)
            return len(self.centroids) - 1
        # Session is at capacity: keep the best guess instead of growing forever.
        if best_index is not None:
            self._update(best_index, vector)
        return best_index

    def _update(self, index, vector):
        count = self.counts[index]
        blended = [
            (old * count + new) / (count + 1)
            for old, new in zip(self.centroids[index], vector)
        ]
        self.centroids[index] = _normalize(blended)
        self.counts[index] = count + 1

    def reset(self):
        self.centroids = []
        self.counts = []

    @staticmethod
    def label(index):
        return None if index is None else f"发言人{index + 1}"


class SpeakerEmbedder:
    """Thin wrapper over sherpa-onnx speaker embedding, imported lazily."""

    def __init__(self, model_path=None, num_threads=2):
        self.model_path = model_path or os.getenv("MEETINGBOX_SPEAKER_MODEL", "").strip()
        self.num_threads = num_threads
        self._extractor = None
        self._unavailable_reason = None
        self._loaded = False

    @property
    def available(self):
        self._load()
        return self._extractor is not None

    @property
    def reason(self):
        self._load()
        return self._unavailable_reason

    def _load(self):
        if self._loaded:
            return
        self._loaded = True
        if not self.model_path:
            self._unavailable_reason = "MEETINGBOX_SPEAKER_MODEL not set"
            return
        if not os.path.exists(self.model_path):
            self._unavailable_reason = f"speaker model missing: {self.model_path}"
            return
        try:
            import sherpa_onnx  # noqa: PLC0415 - optional, device-only dependency

            config = sherpa_onnx.SpeakerEmbeddingExtractorConfig(
                model=self.model_path, num_threads=self.num_threads
            )
            if not config.validate():
                self._unavailable_reason = "speaker model config rejected"
                return
            self._extractor = sherpa_onnx.SpeakerEmbeddingExtractor(config)
        except Exception as error:  # pragma: no cover - depends on device runtime
            self._unavailable_reason = f"sherpa-onnx unavailable: {type(error).__name__}"
            self._extractor = None

    def embed(self, pcm_bytes):
        """Return an embedding for 16 kHz mono PCM16, or None."""
        if not self.available:
            return None
        if len(pcm_bytes) < SAMPLE_RATE:  # under 0.5 s is not worth embedding
            return None
        samples = array("h")
        samples.frombytes(pcm_bytes)
        waveform = [sample / 32768.0 for sample in samples]
        try:
            stream = self._extractor.create_stream()
            stream.accept_waveform(SAMPLE_RATE, waveform)
            stream.input_finished()
            if not self._extractor.is_ready(stream):
                return None
            return list(self._extractor.compute(stream))
        except Exception:  # pragma: no cover - device runtime behaviour
            return None
