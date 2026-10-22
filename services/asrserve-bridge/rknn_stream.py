"""RK3588 SenseVoice engine for asrserve-bridge.

The Intel profile streams from a sherpa-onnx recognizer that keeps decoder state.
The RK service exposes a 30 s *batch* endpoint instead, so this engine recreates the
same streaming behaviour by re-decoding a sliding window and diffing the hypothesis:

* partial hypothesis  -> window re-decoded on a cadence, text diffed against last emit
* utterance boundary  -> measured trailing silence (the RK service has no endpointing)
* final text          -> one decode of the whole utterance buffer

Everything above this file (bridge protocol, recording, exports, refinement hooks)
is shared with the Intel profile, which is the point: only the model differs.

Endpoints (all overridable by environment):

    MEETINGBOX_RK_ASR_URL   default http://127.0.0.1:8020
    MEETINGBOX_RK_WINDOW_MS default 8000   re-decode window for partials
    MEETINGBOX_RK_STRIDE_MS default 600    minimum cadence between partial decodes
    MEETINGBOX_RK_SILENCE_MS default 700   trailing silence that closes an utterance
    MEETINGBOX_RK_MAX_UTTERANCE_MS default 20000  hard cut (model limit is 30 s)
"""

import asyncio
import io
import json
import os
import time
import urllib.request
import wave
from array import array

SAMPLE_RATE = 16000
BYTES_PER_MS = SAMPLE_RATE * 2 // 1000


def _env_int(name, default):
    try:
        value = int(os.getenv(name, ""))
    except ValueError:
        return default
    return value if value > 0 else default


def _endpoint():
    return os.getenv("MEETINGBOX_RK_ASR_URL", "http://127.0.0.1:8020").rstrip("/")


def _pcm_to_wav(pcm):
    output = io.BytesIO()
    with wave.open(output, "wb") as audio:
        audio.setnchannels(1)
        audio.setsampwidth(2)
        audio.setframerate(SAMPLE_RATE)
        audio.writeframes(pcm.tobytes())
    return output.getvalue()


def _multipart(fields, filename, payload, boundary):
    """Minimal multipart/form-data writer so the bridge needs no extra dependency."""
    chunks = []
    for name, value in fields.items():
        chunks.append(
            f'--{boundary}\r\nContent-Disposition: form-data; name="{name}"\r\n\r\n'.encode()
            + str(value).encode()
            + b"\r\n"
        )
    chunks.append(
        (
            f'--{boundary}\r\nContent-Disposition: form-data; name="audio"; filename="{filename}"\r\n'
            "Content-Type: audio/wav\r\n\r\n"
        ).encode()
        + payload
        + b"\r\n"
    )
    chunks.append(f"--{boundary}--\r\n".encode())
    return b"".join(chunks)


class RknnStream:
    """Drop-in replacement for FastStream backed by the RKNN sidecar."""

    def __init__(self, meeting_id, folder):
        self.meeting_id = meeting_id
        self.folder = folder
        self.lock = asyncio.Lock()
        self.done = asyncio.Event()

        self.samples = 0
        self.last_index = -1
        self.records = {}
        self.pending = {}
        self.index = 0
        self.start_ms = 0
        self.generation = 0
        self.last_text = ""

        self.hotwords = []
        self.buffer = array("h")            # current utterance, 16 kHz mono
        self.buffer_start_ms = 0            # absolute position of buffer[0]
        self.last_decode_ms = 0
        self.last_decode_cost_s = 0.0
        self.trailing_silence_ms = 0
        self.failure = None

        self.window_ms = _env_int("MEETINGBOX_RK_WINDOW_MS", 8000)
        self.stride_ms = _env_int("MEETINGBOX_RK_STRIDE_MS", 600)
        self.silence_ms = _env_int("MEETINGBOX_RK_SILENCE_MS", 700)
        self.max_utterance_ms = _env_int("MEETINGBOX_RK_MAX_UTTERANCE_MS", 20000)

    # -- lifecycle ---------------------------------------------------------

    async def open(self, context=""):
        self.hotwords = [word for word in context.split(",") if word][:256]
        self.buffer_start_ms = 0

    async def finish(self):
        async with self.lock:
            if not self.done.is_set():
                text = await asyncio.to_thread(self._decode, bytes(self.buffer.tobytes()), self.buffer_start_ms)
                if text.strip():
                    self._publish(text, True, self.buffer_start_ms + len(self.buffer) // 16)
                self.buffer = array("h")
                self.done.set()
                (self.folder / "live-final.json").write_text(
                    json.dumps(list(self.records.values()), ensure_ascii=False), encoding="utf8"
                )
            return self.take()

    # -- audio feed --------------------------------------------------------

    async def feed(self, pcm):
        if self.done.is_set():
            return
        if len(pcm) % 2:
            pcm = pcm[:-1]
        chunk = array("h")
        chunk.frombytes(pcm)
        if not chunk:
            return
        self.buffer.extend(chunk)
        self.samples += len(chunk)

        quiet = self._is_silent(chunk)
        self.trailing_silence_ms = self.trailing_silence_ms + 200 if quiet else 0

        position_ms = self.buffer_start_ms + len(self.buffer) // 16
        utterance_ms = position_ms - self.buffer_start_ms

        if self.trailing_silence_ms >= self.silence_ms and utterance_ms >= 800:
            await self._close_utterance(position_ms)
            return

        if utterance_ms >= self.max_utterance_ms:
            await self._close_utterance(position_ms)
            return

        if position_ms - self.last_decode_ms >= self._effective_stride():
            await self._emit_partial(position_ms)

    def _effective_stride(self):
        """Backpressure: never queue decodes faster than the NPU can retire them."""
        measured = max(0.0, self.last_decode_cost_s)
        return max(self.stride_ms, int(measured * 2000))

    @staticmethod
    def _is_silent(chunk):
        # 16-bit PCM; ~-46 dBFS. Speech windows from the app are already VAD-gated,
        # this only distinguishes "utterance ended" from "still talking".
        peak = max((abs(sample) for sample in chunk), default=0)
        return peak < 200

    # -- decoding ----------------------------------------------------------

    async def _emit_partial(self, position_ms):
        window_start = max(self.buffer_start_ms, position_ms - self.window_ms)
        offset = (window_start - self.buffer_start_ms) * 16
        pcm = self.buffer[offset:]
        if len(pcm) < SAMPLE_RATE // 2:
            return
        started = time.monotonic()
        text = await asyncio.to_thread(self._decode, bytes(pcm.tobytes()), window_start)
        self.last_decode_cost_s = time.monotonic() - started
        self.last_decode_ms = position_ms
        if text and text != self.last_text:
            self._publish(text, False, position_ms)

    async def _close_utterance(self, position_ms):
        pcm = bytes(self.buffer.tobytes())
        if len(pcm) >= 3200:
            text = await asyncio.to_thread(self._decode, pcm, self.buffer_start_ms)
            if text.strip():
                self._publish(text, True, position_ms)
        self.index += 1
        self.start_ms = position_ms
        self.buffer_start_ms = position_ms
        self.buffer = array("h")
        self.last_text = ""
        self.trailing_silence_ms = 0
        self.last_decode_ms = position_ms

    def _decode(self, pcm, start_ms):
        """POST one WAV window to the RK sidecar and return its text."""
        if self.failure:
            # Stay failed instead of hammering a broken runtime; bridge keeps the audio.
            raise RuntimeError(self.failure)
        if not pcm:
            return ""
        wav = _pcm_to_wav(array("h", pcm))
        boundary = f"----meetingbox{os.getpid()}{time.monotonic_ns()}"
        body = _multipart(
            {
                "meeting_id": self.meeting_id,
                "chunk_start_ms": start_ms,
                "hotwords": json.dumps(self.hotwords, ensure_ascii=False),
                "skip_vad": "1",
                "language": "auto",
            },
            "window.wav",
            wav,
            boundary,
        )
        request = urllib.request.Request(
            _endpoint() + "/v1/asr/transcribe-upload",
            data=body,
            headers={"Content-Type": f"multipart/form-data; boundary={boundary}"},
        )
        try:
            with urllib.request.urlopen(request, timeout=60) as response:
                payload = json.load(response)
        except Exception as error:
            self.failure = f"RK ASR unavailable: {error}"
            raise RuntimeError(self.failure) from error
        segments = payload.get("segments") or []
        return "".join(str(segment.get("text", "")).strip() for segment in segments).strip()

    # -- output ------------------------------------------------------------

    def _publish(self, text, final, end_ms):
        text = text.strip()
        if not text:
            return
        key = self.index * 100
        record = {
            "utterance": key,
            "start_ms": self.start_ms,
            "end_ms": end_ms,
            "text": text,
            "speaker": None,
            "is_partial": not final,
            "text_ready_audio_ms": self.samples // 16,
            "generation": self.generation,
            "token_times_ms": [],
            "emitted_wall_time": time.time(),
            "speech_start_ms": self.start_ms,
            "speech_end_ms": end_ms,
        }
        self.generation += 1
        self.records[key] = record
        self.pending[key] = dict(record)
        with (self.folder / "stream.events.jsonl").open("a", encoding="utf8") as out:
            out.write(json.dumps(record, ensure_ascii=False) + "\n")
        self.last_text = text

    def take(self):
        records = sorted(self.pending.values(), key=lambda record: record["utterance"])
        self.pending.clear()
        return {
            "segments": records,
            "continuous": True,
            "upsert": any(record["is_partial"] for record in records),
            "diarization": "unavailable",
        }
