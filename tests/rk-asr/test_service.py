import io
import json
import sys
import struct
import threading
import unittest
import urllib.error
import urllib.request
import wave
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "services" / "rk-asr"))
from server import MAX_BODY, RequestError, Server, Service, decode_wav, parse_upload


def wav(samples=16000, channels=1):
    target = io.BytesIO()
    with wave.open(target, "wb") as audio:
        audio.setnchannels(channels)
        audio.setsampwidth(2)
        audio.setframerate(16000)
        audio.writeframes(b"\x00\x10" * samples * channels)
    return target.getvalue()


def multipart(audio, fields=None):
    parts = []
    for name, value in (fields or {"meeting_id": "1", "chunk_start_ms": "2300", "hotwords": '["峰瞰"]', "skip_vad": "0"}).items():
        parts.append(f'--test\r\nContent-Disposition: form-data; name="{name}"\r\n\r\n{value}\r\n'.encode())
    parts.append(b'--test\r\nContent-Disposition: form-data; name="audio"; filename="chunk.wav"\r\nContent-Type: audio/wav\r\n\r\n' + audio + b'\r\n--test--\r\n')
    return b"".join(parts)


class Engine:
    model_id = "test-only"
    runtime_info = {"test_double": True}

    def recognize(self, samples, language):
        return "你好"


class Tests(unittest.TestCase):
    def setUp(self):
        self.service = Service(Engine())
        self.server = Server(("127.0.0.1", 0), self.service)
        self.thread = threading.Thread(target=self.server.serve_forever)
        self.thread.start()
        self.url = "http://127.0.0.1:" + str(self.server.server_port)

    def tearDown(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join()

    def request(self, path, body=None, content_type="multipart/form-data; boundary=test"):
        request = urllib.request.Request(self.url + path, data=body, headers={"Content-Type": content_type})
        try:
            response = urllib.request.urlopen(request, timeout=3)
        except urllib.error.HTTPError as response_error:
            response = response_error
        with response:
            return response.status, json.load(response)

    def test_client_wire_format_and_timeline(self):
        status, result = self.request("/v1/asr/transcribe-upload", multipart(wav()))
        self.assertEqual(status, 200)
        self.assertEqual(result["backend"], "sensevoice-rknn")
        self.assertEqual(result["segments"], [{"text": "你好", "speaker": None, "speaker_id": None}])
        self.assertEqual(parse_upload("multipart/form-data; boundary=test", multipart(wav()))[1], 2300)
        self.assertEqual(self.request("/healthz")[1]["inference_count"], 1)

    def test_silence_is_valid(self):
        status, result = self.request("/v1/asr/transcribe-upload", multipart(wav()[:44] + b"\x00\x00" * 16000))
        self.assertEqual(status, 200)
        self.assertEqual(result["segments"], [])

    def test_failure_marks_health_unavailable(self):
        def fail(*args):
            raise RuntimeError("private transcript and path")
        self.service.engine.recognize = fail
        status, result = self.request("/v1/asr/transcribe-upload", multipart(wav()))
        self.assertEqual(status, 503)
        self.assertNotIn("private", json.dumps(result))
        self.assertFalse(self.request("/healthz")[1]["ready"])

    def test_no_parallel_runtime_access(self):
        self.service.lock.acquire()
        try:
            self.assertEqual(self.request("/v1/asr/transcribe-upload", multipart(wav()))[0], 503)
        finally:
            self.service.lock.release()
        self.assertTrue(self.service.health()["ready"])

    def test_finalize_preserves_existing_transcripts(self):
        status, result = self.request("/v1/asr/finalize-meeting", b'{"meeting_id":1,"segments":[]}', "application/json")
        self.assertEqual(status, 200)
        self.assertFalse(result["finalized"])
        self.assertFalse(result["pending"])
        self.assertEqual(result["segments"], [])
        self.assertIn("unavailable", result["error"])

    def test_invalid_audio_and_fields(self):
        for audio in (b"not WAV", b"RIFF" + struct.pack("<I", 12) + b"WAVEJUNK" + struct.pack("<I", 1000) + b"x",
                      wav(channels=2), wav(samples=1599), wav(samples=480001), wav()[:-2]):
            with self.subTest(size=len(audio)):
                self.assertEqual(self.request("/v1/asr/transcribe-upload", multipart(audio))[0], 400)
        for fields in ({"skip_vad": "yes"}, {"hotwords": "{}"}, {"chunk_start_ms": "-1"}, {"language": "bad"}):
            self.assertEqual(self.request("/v1/asr/transcribe-upload", multipart(wav(), fields))[0], 400)

    def test_missing_duplicate_and_oversized_uploads(self):
        self.assertEqual(self.request("/v1/asr/transcribe-upload", b"--test--\r\n")[0], 400)
        body = multipart(wav()).replace(b"--test--\r\n", b'--test\r\nContent-Disposition: form-data; name="audio"\r\n\r\nx\r\n--test--\r\n')
        self.assertEqual(self.request("/v1/asr/transcribe-upload", body)[0], 400)
        self.assertEqual(self.request("/v1/asr/transcribe-upload", b"x" * (MAX_BODY + 1))[0], 413)


if __name__ == "__main__":
    unittest.main()
