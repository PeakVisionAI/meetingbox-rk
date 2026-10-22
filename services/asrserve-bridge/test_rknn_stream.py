"""Tests for the RKNN streaming engine.

Runs without RK hardware: a stdlib HTTP stub stands in for the SenseVoice sidecar,
so the sliding-window, silence-boundary and failure behaviour are all exercised.
"""

import asyncio
import json
import os
import tempfile
import threading
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

os.environ.setdefault("MEETINGBOX_RK_STRIDE_MS", "200")
os.environ.setdefault("MEETINGBOX_RK_SILENCE_MS", "400")

import rknn_stream  # noqa: E402  (environment must be set first)

CHUNK_SAMPLES = 3200  # 200 ms at 16 kHz


class StubEngine(BaseHTTPRequestHandler):
    calls = []

    def log_message(self, *args):
        pass

    def do_GET(self):
        payload = json.dumps({"ready": True, "backend": "sensevoice-rknn"}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def do_POST(self):
        length = int(self.headers["Content-Length"])
        body = self.rfile.read(length)
        if b"multipart/form-data" not in self.headers["Content-Type"].encode():
            raise AssertionError("expected multipart body")
        assert b'name="meeting_id"' in body
        assert b'name="audio"; filename="window.wav"' in body
        assert body.startswith(b"--")
        index = len(StubEngine.calls) + 1
        StubEngine.calls.append(index)
        payload = json.dumps(
            {"segments": [{"text": f"第{index}段", "speaker": None}], "backend": "sensevoice-rknn"}
        ).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)


def speech_chunk(samples=CHUNK_SAMPLES, amplitude=3000):
    values = [amplitude if index % 2 else -amplitude for index in range(samples)]
    return b"".join(value.to_bytes(2, "little", signed=True) for value in values)


def silence_chunk(samples=CHUNK_SAMPLES):
    return b"\x00" * (samples * 2)


class RknnStreamTest(unittest.IsolatedAsyncioTestCase):
    @classmethod
    def setUpClass(cls):
        cls.server = ThreadingHTTPServer(("127.0.0.1", 0), StubEngine)
        cls.thread = threading.Thread(target=cls.server.serve_forever, daemon=True)
        cls.thread.start()
        os.environ["MEETINGBOX_RK_ASR_URL"] = f"http://127.0.0.1:{cls.server.server_port}"

    @classmethod
    def tearDownClass(cls):
        cls.server.shutdown()

    def setUp(self):
        StubEngine.calls = []
        self.folder = Path(tempfile.mkdtemp())

    async def test_partial_then_final_on_silence(self):
        stream = rknn_stream.RknnStream(7, self.folder)
        await stream.open("会议,预算")

        for _ in range(6):
            await stream.feed(speech_chunk())
        partials = stream.take()
        self.assertTrue(partials["segments"], "expected at least one partial hypothesis")
        self.assertTrue(any(segment["is_partial"] for segment in partials["segments"]))
        self.assertTrue(partials["upsert"])
        self.assertEqual(partials["diarization"], "unavailable")

        for _ in range(3):
            await stream.feed(silence_chunk())
        finals = stream.take()
        closed = [segment for segment in finals["segments"] if not segment["is_partial"]]
        self.assertTrue(closed, "silence should close an utterance")
        self.assertEqual(closed[-1]["utterance"], 0)
        self.assertEqual(stream.index, 1)

    async def test_finish_flushes_and_writes_snapshot(self):
        stream = rknn_stream.RknnStream(8, self.folder)
        await stream.open("")
        await stream.feed(speech_chunk())
        result = await stream.finish()
        self.assertTrue(stream.done.is_set())
        self.assertTrue(any(not segment["is_partial"] for segment in result["segments"]))
        snapshot = json.loads((self.folder / "live-final.json").read_text(encoding="utf8"))
        self.assertTrue(snapshot)
        self.assertEqual(snapshot[0]["utterance"], 0)
        self.assertFalse(result["upsert"])

    async def test_transport_failure_is_sticky(self):
        stream = rknn_stream.RknnStream(9, self.folder)
        await stream.open("")
        stream.meeting_id = 9
        os.environ["MEETINGBOX_RK_ASR_URL"] = "http://127.0.0.1:1"
        try:
            with self.assertRaises(RuntimeError):
                for _ in range(4):
                    await stream.feed(speech_chunk())
        finally:
            os.environ["MEETINGBOX_RK_ASR_URL"] = (
                f"http://127.0.0.1:{self.server.server_port}"
            )
        # First failure is recorded; the engine must not keep hammering the runtime.
        self.assertEqual(stream.failure is not None, True)
        self.assertIn("RK ASR unavailable", stream.failure)

    def test_record_schema_matches_bridge_contract(self):
        stream = rknn_stream.RknnStream(10, self.folder)
        stream.samples = 3200
        stream._publish("测试", True, 200)
        record = stream.records[0]
        for key in (
            "utterance",
            "start_ms",
            "end_ms",
            "text",
            "speaker",
            "is_partial",
            "generation",
            "token_times_ms",
        ):
            self.assertIn(key, record)
        self.assertEqual(record["utterance"], 0)
        self.assertEqual(stream.take()["segments"][0]["text"], "测试")


if __name__ == "__main__":
    unittest.main()
