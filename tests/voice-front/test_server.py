"""VoiceFront HTTP/ASR regressions, without models or external service calls.

Run with the service's installed dependencies (no dependency stubs)::

    python -m unittest discover -s tests/voice-front -v

MEETINGBOX_VOICE_TEST_ROOT may point to a copied repository layout on a test
host, containing services/voice-front/server.py and its real vendor directory.
Only the front instance and ASR HTTP transport are replaced. Production module
imports, multipart parsing, WAV decoding, and response serialization stay real.
"""
from __future__ import annotations

import importlib.util
import io
import json
import os
from pathlib import Path
import struct
import unittest
from unittest.mock import patch
import wave

import httpx
import numpy as np
from fastapi.testclient import TestClient


ROOT = Path(os.environ.get("MEETINGBOX_VOICE_TEST_ROOT", Path(__file__).resolve().parents[2]))
SERVICE_PATH = ROOT / "services" / "voice-front" / "server.py"
spec = importlib.util.spec_from_file_location("meetingbox_voice_front_server_tests", SERVICE_PATH)
server = importlib.util.module_from_spec(spec)
spec.loader.exec_module(server)


def wav(frames=3200, *, rate=16000, channels=1, width=2):
    """Build independent wire fixtures, without the production WAV encoder."""
    output = io.BytesIO()
    with wave.open(output, "wb") as stream:
        stream.setnchannels(channels)
        stream.setsampwidth(width)
        stream.setframerate(rate)
        # Distinct channels make the non-mono fixture easy to inspect.
        frame = (b"\x00\x20" + b"\x00\x60" * (channels - 1)) if width == 2 else b"\0" * width * channels
        stream.writeframes(frame * frames)
    return output.getvalue()


class FakeFront:
    def __init__(self):
        self._entries = {}
        self._tasks = set()
        self.calls = []
        self.resets = []
        self.warmed = False
        self.error = None
        self.reset_result = True
        self.health_result = {
            "available": True, "kws": {"loaded": True, "wakeWords": ["小峰小峰"]},
            "vad": {"loaded": True}, "addressee": {"available": False, "loaded": False},
        }
        self.feed_result = {"seq": 7, "state": "listening", "events": [{"type": "wake"}], "pending": 0}

    async def warm(self):
        self.warmed = True

    def health(self):
        return dict(self.health_result)

    async def feed(self, **kwargs):
        self.calls.append(kwargs)
        if self.error is not None:
            raise self.error
        return self.feed_result

    def reset(self, session_id):
        self.resets.append(session_id)
        self._entries.pop(session_id, None)
        return self.reset_result


class HttpTests(unittest.TestCase):
    def setUp(self):
        self.front = FakeFront()
        self.front_patch = patch.object(server, "front", self.front)
        self.front_patch.start()
        self.addCleanup(self.front_patch.stop)
        self.client = self.enterContext(TestClient(server.app, raise_server_exceptions=False))

    def feed(self, audio=None, **fields):
        data = {"session_id": " desktop ", "seq": "7"}
        data.update(fields)
        return self.client.post("/voice/feed", files={"audio": ("frame.wav", wav() if audio is None else audio, "audio/wav")}, data=data)

    def test_imports_use_the_real_vendored_package(self):
        import voice_front.router
        import voice_front.service
        vendor = (SERVICE_PATH.parent / "vendor").resolve()
        for module in (voice_front.router, voice_front.service):
            self.assertTrue(Path(module.__file__).resolve().is_relative_to(vendor))
        self.assertIs(server.decode_chunk, voice_front.router.decode_chunk)
        self.assertIs(server.VoiceFrontError, voice_front.service.VoiceFrontError)

    def test_lifespan_warms_front_and_resets_remaining_sessions(self):
        self.assertTrue(self.front.warmed)
        # Use a separate context so shutdown can be inspected before tearDown.
        self.front._entries.update({"one": object(), "two": object()})
        with TestClient(server.app):
            pass
        self.assertEqual(self.front.resets, ["one", "two"])
        self.assertEqual(self.front._entries, {})

    def test_health_preserves_model_state_and_reports_command_limits(self):
        self.front.health_result.update(available=False, reason="models_missing")
        with patch.object(server, "ASR_URL", "http://asr.invalid:8020"):
            response = self.client.get("/voice/health")
        self.assertEqual(response.status_code, 200)
        self.assertEqual(response.json(), {
            **self.front.health_result, "commandAsr": "http://asr.invalid:8020",
            "mode": "wake", "maxUtteranceSeconds": 10.0,
        })
        self.assertEqual(server.settings.max_sessions, 2)
        self.assertEqual(server.settings.max_pending_audio_s, 12.0)
        # Matches the native 20 s wake window; vendor defaults are 5 s / 10 s.
        self.assertEqual(server.settings.wake_timeout_s, 20.0)
        self.assertEqual(server.settings.followup_window_s, 20.0)

    def test_feed_wire_fields_defaults_and_real_pcm_decoding(self):
        response = self.feed()
        self.assertEqual(response.status_code, 200)
        self.assertEqual(response.json(), self.front.feed_result)
        call = self.front.calls[0]
        self.assertEqual({k: v for k, v in call.items() if k != "pcm"}, {
            "session_id": "desktop", "seq": 7, "mode": "wake",
            "expect_followup": False, "barge_active": False, "suppress": False,
        })
        self.assertEqual(call["pcm"].dtype, np.float32)
        self.assertEqual(call["pcm"].shape, (3200,))
        self.assertTrue(call["pcm"].flags.c_contiguous)
        np.testing.assert_array_equal(call["pcm"], np.full(3200, 0.25, dtype=np.float32))

    def test_feed_parses_boolean_flags(self):
        for wire, expected in (("true", True), ("1", True), ("false", False), ("0", False)):
            with self.subTest(wire=wire):
                response = self.feed(expect_followup=wire, barge_active=wire, suppress=wire)
                self.assertEqual(response.status_code, 200)
                for field in ("expect_followup", "barge_active", "suppress"):
                    self.assertIs(self.front.calls[-1][field], expected)

    def test_rejects_free_and_unknown_modes_without_feeding(self):
        for mode in ("wake_free", "free", "WAKE", "unknown"):
            with self.subTest(mode=mode):
                response = self.feed(mode=mode)
                self.assertEqual(response.status_code, 400)
                self.assertEqual(response.json(), {"detail": "invalid wake feed"})
        self.assertEqual(self.front.calls, [])

    def test_session_and_sequence_bounds(self):
        for fields in ({"session_id": " "}, {"session_id": "x" * 129}, {"seq": "-1"}):
            with self.subTest(fields=fields):
                self.assertEqual(self.feed(**fields).status_code, 400)
        self.assertEqual(self.front.calls, [])
        self.assertEqual(self.feed(session_id="x" * 128, seq="0").status_code, 200)
        self.assertEqual(self.front.calls[-1]["seq"], 0)

    def test_missing_and_unparseable_form_fields(self):
        for field in ("audio", "session_id", "seq"):
            with self.subTest(missing=field):
                data = {"session_id": "desktop", "seq": "0"}
                data.pop(field, None)
                files = {} if field == "audio" else {"audio": ("frame.wav", wav(), "audio/wav")}
                self.assertEqual(self.client.post("/voice/feed", files=files, data=data).status_code, 422)
        for fields in ({"seq": "nope"}, {"suppress": "nope"}, {"barge_active": "nope"}, {"expect_followup": "nope"}):
            with self.subTest(fields=fields):
                self.assertEqual(self.feed(**fields).status_code, 422)
        self.assertEqual(self.front.calls, [])

    def test_front_errors_keep_status_code_and_message(self):
        for status, code in ((409, "busy"), (503, "models_missing")):
            with self.subTest(status=status):
                self.front.error = server.VoiceFrontError(code, "front unavailable", status=status)
                response = self.feed()
                self.assertEqual(response.status_code, status)
                self.assertEqual(response.json(), {"detail": {"code": code, "message": "front unavailable"}})

    def test_audio_duration_boundary(self):
        self.assertEqual(self.feed(wav(8000)).status_code, 200)
        self.assertEqual(self.front.calls[-1]["pcm"].shape, (8000,))
        self.front.calls.clear()
        response = self.feed(wav(8001))
        self.assertEqual(response.status_code, 400)
        self.assertEqual(response.json(), {"detail": "invalid 16 kHz mono PCM16 WAV"})
        self.assertEqual(self.front.calls, [])

    def test_overlong_pcm32_cannot_be_shortened_by_the_upload_read_cap(self):
        # 8001 PCM32 frames fit under 40 KB, but exceed the 500 ms limit.
        # Reading only 32045 bytes leaves 8000 complete frames for libsndfile,
        # so checking just the decoded frame count can hide the excess audio.
        response = self.feed(wav(8001, width=4))
        self.assertEqual(response.status_code, 400, "the read cap must not turn an overlong upload into a valid chunk")
        self.assertEqual(self.front.calls, [])

    def test_bad_empty_and_wrong_rate_audio(self):
        for audio in (b"", b"not a WAV", wav(0), wav(rate=8000), wav(rate=48000)):
            with self.subTest(size=len(audio)):
                response = self.feed(audio)
                self.assertEqual(response.status_code, 400)
                self.assertEqual(response.json(), {"detail": "invalid 16 kHz mono PCM16 WAV"})
        self.assertEqual(self.front.calls, [])

    def test_rejects_stereo_and_non_pcm16_before_feeding(self):
        for audio in (wav(channels=2), wav(width=1), wav(width=3), wav(width=4)):
            with self.subTest(size=len(audio)):
                response = self.feed(audio)
                self.assertEqual(response.status_code, 400)
                self.assertEqual(response.json(), {"detail": "invalid 16 kHz mono PCM16 WAV"})
        self.assertEqual(self.front.calls, [])

    def test_truncated_wav_is_rejected_before_feeding(self):
        # The header advertises 3200 frames, but the last frame is missing.
        response = self.feed(wav()[:-2])
        self.assertEqual(response.status_code, 400, "truncated WAV must not be accepted as a shorter valid chunk")
        self.assertEqual(self.front.calls, [])

    def test_non_wav_audio_is_rejected_before_feeding(self):
        import soundfile
        output = io.BytesIO()
        soundfile.write(output, np.full(3200, 0.25, dtype=np.float32), 16000, format="FLAC")
        response = self.feed(output.getvalue())
        self.assertEqual(response.status_code, 400, "feed requires WAV, even when libsndfile can decode another format")
        self.assertEqual(self.front.calls, [])

    def test_post_requires_bounded_decimal_content_length(self):
        for length in (None, "", "-1", "abc", "1.0", "40001"):
            with self.subTest(length=length):
                request = self.client.build_request("POST", "/voice/reset", data={"session_id": "desktop"})
                if length is None:
                    del request.headers["content-length"]
                else:
                    request.headers["content-length"] = length
                response = self.client.send(request)
                self.assertEqual(response.status_code, 413)
                self.assertEqual(response.json(), {"detail": "bounded Content-Length required"})
        self.assertEqual(self.front.resets, [])

    def test_content_length_cap_accepts_exactly_40000_and_rejects_next_byte(self):
        prefix = b"session_id=desktop&padding="
        for size, status in ((40000, 200), (40001, 413)):
            with self.subTest(size=size):
                response = self.client.post("/voice/reset", content=prefix + b"x" * (size - len(prefix)),
                                            headers={"content-type": "application/x-www-form-urlencoded"})
                self.assertEqual(response.status_code, status)
        self.assertEqual(self.front.resets, ["desktop"])

    def test_large_audio_rejected_without_calling_front(self):
        self.assertEqual(self.feed(wav(20000)).status_code, 413)
        self.assertEqual(self.front.calls, [])

    def test_reset_trims_session_and_reports_existence(self):
        for existed in (True, False):
            with self.subTest(existed=existed):
                self.front.reset_result = existed
                response = self.client.post("/voice/reset", data={"session_id": " desktop "})
                self.assertEqual(response.status_code, 200)
                self.assertEqual(response.json(), {"ok": True, "existed": existed})
        self.assertEqual(self.front.resets, ["desktop", "desktop"])


class KwsThreadTests(unittest.TestCase):
    def test_default_and_override_reach_native_engine_without_changing_keywords(self):
        for threads, expected in ((None, 1), ("2", 2)):
            with self.subTest(threads=threads), patch.dict(os.environ, {}, clear=True):
                if threads is not None:
                    os.environ["MEETINGBOX_KWS_THREADS"] = threads
                with patch.object(server.KwsEngine, "__init__", return_value=None) as constructor:
                    server.MeetingboxKwsEngine("model", wake_words=["小峰小峰"], keywords_threshold=0.25)
                constructor.assert_called_once_with(
                    "model", wake_words=["小峰小峰"], keywords_threshold=0.25,
                    num_threads=expected,
                )

    def test_invalid_thread_count_does_not_construct_engine(self):
        for value in ("0", "5", "invalid"):
            with self.subTest(value=value), patch.dict(os.environ, {"MEETINGBOX_KWS_THREADS": value}):
                with patch.object(server.KwsEngine, "__init__", return_value=None) as constructor:
                    with self.assertRaises(ValueError):
                        server.MeetingboxKwsEngine("model", wake_words=["小峰小峰"])
                constructor.assert_not_called()


class PcmWavTests(unittest.TestCase):
    def test_pcm16_mono_16khz_clipping_and_little_endian_samples(self):
        samples = np.array([-2, -1, -0.5, 0, 0.5, 1, 2], dtype=np.float32)
        original = samples.copy()
        encoded = server.pcm_wav(samples)
        self.assertTrue(encoded.startswith(b"RIFF"))
        self.assertEqual(encoded[8:12], b"WAVE")
        with wave.open(io.BytesIO(encoded), "rb") as stream:
            self.assertEqual((stream.getnchannels(), stream.getsampwidth(), stream.getframerate(), stream.getnframes()),
                             (1, 2, 16000, 7))
            self.assertEqual(stream.getcomptype(), "NONE")
            self.assertEqual(struct.unpack("<7h", stream.readframes(7)), (-32767, -32767, -16383, 0, 16383, 32767, 32767))
        np.testing.assert_array_equal(samples, original)

    def test_command_wav_duration_and_payload_size(self):
        encoded = server.pcm_wav(np.zeros(16000, dtype=np.float32))
        with wave.open(io.BytesIO(encoded), "rb") as stream:
            self.assertEqual(stream.getnframes() / stream.getframerate(), 1.0)
            self.assertEqual(stream.readframes(16000), b"\0\0" * 16000)
        self.assertEqual(len(encoded), 44 + 32000)


class RecognizeTests(unittest.IsolatedAsyncioTestCase):
    async def recognize_with(self, handler):
        # Exercise real httpx request/multipart/response behavior; no sockets.
        real_client = httpx.AsyncClient
        options = []

        def client_factory(**kwargs):
            options.append(kwargs)
            return real_client(transport=httpx.MockTransport(handler), **kwargs)

        with patch.object(server.httpx, "AsyncClient", side_effect=client_factory), patch.object(server, "ASR_URL", "http://asr.invalid:8020"):
            result = await server.recognize(np.array([0.25, -0.25], dtype=np.float32))
        self.assertEqual(options, [{"timeout": 20.0, "trust_env": False}])
        return result

    async def test_asr_multipart_fields_filename_and_wav(self):
        from email import policy
        from email.parser import BytesParser

        def handler(request):
            self.assertEqual(request.method, "POST")
            self.assertEqual(str(request.url), "http://asr.invalid:8020/v1/asr/transcribe-upload")
            message = BytesParser(policy=policy.default).parsebytes(
                b"Content-Type: " + request.headers["content-type"].encode() + b"\r\nMIME-Version: 1.0\r\n\r\n" + request.read())
            parts = {part.get_param("name", header="content-disposition"): part for part in message.iter_parts()}
            self.assertEqual(set(parts), {"audio", "meeting_id", "chunk_start_ms", "skip_vad"})
            for name, expected in (("meeting_id", b"0"), ("chunk_start_ms", b"0"), ("skip_vad", b"1")):
                self.assertEqual(parts[name].get_payload(decode=True), expected)
            self.assertEqual(parts["audio"].get_filename(), "command.wav")
            self.assertEqual(parts["audio"].get_content_type(), "audio/wav")
            with wave.open(io.BytesIO(parts["audio"].get_payload(decode=True)), "rb") as stream:
                self.assertEqual((stream.getnchannels(), stream.getsampwidth(), stream.getframerate(), stream.getnframes()), (1, 2, 16000, 2))
                self.assertEqual(struct.unpack("<2h", stream.readframes(2)), (8191, -8191))
            return httpx.Response(200, json={"segments": [{"text": "开始"}, {"text": "录音"}]})

        self.assertEqual(await self.recognize_with(handler), "开始录音")

    async def test_joins_segments_in_order_including_empty_segments(self):
        result = await self.recognize_with(lambda request: httpx.Response(200, json={
            "segments": [{"text": " 小峰"}, {}, {"text": "，开始录音。 "}], "error": None,
        }))
        self.assertEqual(result, " 小峰，开始录音。 ")

    async def test_empty_segments_are_valid_silence(self):
        self.assertEqual(await self.recognize_with(lambda request: httpx.Response(200, json={"segments": []})), "")

    async def test_error_payload_and_missing_or_nonlist_segments_fail(self):
        for payload in ({"error": "ASR unavailable", "segments": []}, {}, {"segments": None}, {"segments": {}}, {"segments": "text"}):
            with self.subTest(payload=payload):
                with self.assertRaisesRegex(RuntimeError, "command recognition failed"):
                    await self.recognize_with(lambda request: httpx.Response(200, json=payload))

    async def test_http_failures_propagate(self):
        for status in (400, 503):
            with self.subTest(status=status):
                with self.assertRaises(httpx.HTTPStatusError) as error:
                    await self.recognize_with(lambda request: httpx.Response(status, json={"segments": [{"text": "must not be returned"}]}))
                self.assertEqual(error.exception.response.status_code, status)

    async def test_timeout_propagates(self):
        def timeout(request):
            raise httpx.ReadTimeout("ASR timed out", request=request)
        with self.assertRaises(httpx.ReadTimeout):
            await self.recognize_with(timeout)

    async def test_invalid_json_propagates(self):
        with self.assertRaises(json.JSONDecodeError):
            await self.recognize_with(lambda request: httpx.Response(200, content=b"not JSON"))


if __name__ == "__main__":
    unittest.main()
