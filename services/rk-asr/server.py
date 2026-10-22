#!/usr/bin/env python3
"""Loopback HTTP adapter for a separately provisioned SenseVoice RKNN runtime."""
import argparse
import array
import io
import json
import logging
import sys
import threading
import time
import wave
from email.parser import BytesParser
from email.policy import default
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

SAMPLE_RATE = 16000
MAX_SECONDS = 30
MAX_BODY = SAMPLE_RATE * 2 * MAX_SECONDS + 65536
LOG = logging.getLogger("meetingbox.rk_asr")


class RequestError(Exception):
    def __init__(self, status, message):
        super().__init__(message)
        self.status = status


def decode_wav(data):
    try:
        with wave.open(io.BytesIO(data), "rb") as audio:
            if (audio.getframerate(), audio.getnchannels(), audio.getsampwidth(), audio.getcomptype()) != (SAMPLE_RATE, 1, 2, "NONE"):
                raise RequestError(400, "audio must be 16kHz mono PCM16 WAV")
            frames = audio.getnframes()
            if not 1600 <= frames <= SAMPLE_RATE * MAX_SECONDS:
                raise RequestError(400, "audio duration must be 0.1–30 seconds")
            pcm = audio.readframes(frames)
            if len(pcm) != frames * 2:
                raise RequestError(400, "truncated WAV")
    except (wave.Error, EOFError, RuntimeError) as error:
        raise RequestError(400, "invalid WAV") from error
    samples = array.array("h", pcm)
    if sys.byteorder != "little":
        samples.byteswap()
    return [value / 32768.0 for value in samples]


def parse_upload(content_type, body):
    if "\r" in content_type or "\n" in content_type:
        raise RequestError(400, "invalid content type")
    message = BytesParser(policy=default).parsebytes(
        ("Content-Type: " + content_type + "\r\nMIME-Version: 1.0\r\n\r\n").encode("ascii") + body
    )
    if message.get_content_type() != "multipart/form-data" or not message.is_multipart() or message.defects:
        raise RequestError(400, "multipart/form-data required")
    parts = list(message.iter_parts())
    if len(parts) > 8:
        raise RequestError(400, "too many multipart fields")
    fields = {}
    for part in parts:
        name = part.get_param("name", header="content-disposition")
        if name not in ("audio", "meeting_id", "chunk_start_ms", "hotwords", "skip_vad", "language") or name in fields or part.is_multipart() or part.defects:
            raise RequestError(400, "invalid multipart field")
        value = part.get_payload(decode=True)
        if value is None:
            raise RequestError(400, "invalid multipart value")
        if name != "audio" and len(value) > 16384:
            raise RequestError(400, "multipart field too large")
        fields[name] = value
    if "audio" not in fields:
        raise RequestError(400, "audio is required")
    try:
        meeting_id = int(fields.get("meeting_id", b"0"))
        start_ms = int(fields.get("chunk_start_ms", b"0"))
        hotwords = json.loads(fields.get("hotwords", b"[]"))
        skip_vad = fields.get("skip_vad", b"0").decode("ascii")
        language = fields.get("language", b"auto").decode("ascii")
        if not 0 <= meeting_id < 2**63 or not 0 <= start_ms < 2**63 or skip_vad not in ("0", "1"):
            raise ValueError()
        if not isinstance(hotwords, list) or len(hotwords) > 256:
            raise ValueError()
        if any(not isinstance(entry, str) or len(entry) > 256 for entry in hotwords):
            raise ValueError()
        if language != "auto":
            raise ValueError()
    except (ValueError, UnicodeError, TypeError) as error:
        raise RequestError(400, "invalid ASR fields") from error
    return decode_wav(fields["audio"]), start_ms, language


class Service:
    def __init__(self, engine):
        self.engine = engine
        self.lock = threading.Lock()
        self.failed = False
        self.inference_count = 0

    def health(self):
        return {
            "ready": not self.failed and getattr(self.engine, "ready", True),
            "backend": "sensevoice-rknn",
            "device": "rk3588-npu",
            "model": self.engine.model_id,
            "runtime": self.engine.runtime_info,
            "capabilities": {"speaker_calibration": False, "vad": "client", "hotwords": "client-text-correction"},
            "inference_count": self.inference_count,
            "hardware_verified": False,
        }

    def transcribe(self, samples, start_ms, language):
        if self.failed:
            raise RequestError(503, "RKNN runtime requires restart")
        if not self.lock.acquire(blocking=False):
            raise RequestError(503, "RKNN inference busy; retry in a new task")
        started = time.monotonic()
        try:
            if self.failed:
                raise RequestError(503, "RKNN runtime requires restart")
            # No energy threshold: quiet speech must not be silently discarded.
            text = self.engine.recognize(samples, language) if any(samples) else ""
            if not isinstance(text, str):
                raise RuntimeError("invalid recognizer result")
            self.inference_count += 1
            elapsed = time.monotonic() - started
            duration = len(samples) / SAMPLE_RATE
            LOG.info("inference backend=sensevoice-rknn samples=%d elapsed=%.3f", len(samples), elapsed)
            return {
                "backend": "sensevoice-rknn", "device": "rk3588-npu", "model": self.engine.model_id,
                "segments": [{"text": text, "speaker": None, "speaker_id": None}] if text.strip() else [],
                "diarization": "unavailable", "elapsed_seconds": round(elapsed, 4), "rtf": round(elapsed / duration, 4),
            }
        except RequestError:
            raise
        except Exception as error:
            self.failed = True
            # Runtime errors can contain decoded text or local model paths.
            LOG.error("RKNN inference failed: %s", type(error).__name__)
            raise RequestError(503, "RKNN inference failed; restart required") from error
        finally:
            self.lock.release()


class Server(ThreadingHTTPServer):
    daemon_threads = True
    allow_reuse_address = True

    def __init__(self, address, service):
        self.service = service
        self.connections = threading.BoundedSemaphore(8)
        super().__init__(address, Handler)

    def process_request(self, request, client_address):
        if not self.connections.acquire(blocking=False):
            request.close()
            return
        try:
            super().process_request(request, client_address)
        except BaseException:
            self.connections.release()
            raise

    def process_request_thread(self, request, client_address):
        try:
            super().process_request_thread(request, client_address)
        finally:
            self.connections.release()


class Handler(BaseHTTPRequestHandler):
    def setup(self):
        super().setup()
        self.connection.settimeout(10)

    def log_message(self, format, *args):
        # Avoid logging uploaded text, caller-controlled URLs, or meeting IDs.
        pass

    def respond(self, status, payload):
        body = json.dumps(payload, ensure_ascii=False).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Connection", "close")
        self.end_headers()
        try:
            self.wfile.write(body)
        except (BrokenPipeError, ConnectionResetError):
            pass
        self.close_connection = True

    def do_GET(self):
        if self.path == "/healthz":
            self.respond(200, self.server.service.health())
        else:
            self.respond(404, {"error": "unknown endpoint"})

    def do_POST(self):
        try:
            if self.path not in ("/v1/asr/transcribe-upload", "/v1/asr/finalize-meeting"):
                raise RequestError(404, "unknown endpoint")
            if self.headers.get("Transfer-Encoding") or len(self.headers.get_all("Content-Length", [])) != 1:
                raise RequestError(400, "one Content-Length required")
            try:
                size = int(self.headers["Content-Length"])
            except ValueError as error:
                raise RequestError(400, "invalid Content-Length") from error
            limit = 16384 if self.path.endswith("finalize-meeting") else MAX_BODY
            if not 0 < size <= limit:
                raise RequestError(413, "request body exceeds limit")
            body = self.rfile.read(size)
            if len(body) != size:
                raise RequestError(400, "truncated request")
            if self.path.endswith("finalize-meeting"):
                payload = json.loads(body)
                if not isinstance(payload, dict) or type(payload.get("meeting_id")) is not int or payload["meeting_id"] <= 0:
                    raise RequestError(400, "positive meeting_id required")
                self.respond(200, {"finalized": False, "pending": False, "segments": [],
                                   "error": "Speaker calibration is unavailable; preserve the existing transcript."})
            else:
                samples, start_ms, language = parse_upload(self.headers.get("Content-Type", ""), body)
                self.respond(200, self.server.service.transcribe(samples, start_ms, language))
        except RequestError as error:
            self.respond(error.status, {"error": str(error)})
        except (ValueError, UnicodeError):
            self.respond(400, {"error": "invalid request"})
        except (TimeoutError, OSError):
            self.respond(408, {"error": "request read failed"})
        except Exception as error:
            LOG.error("request failed: %s", type(error).__name__)
            self.respond(500, {"error": "request failed"})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--assets", type=Path, required=True)
    parser.add_argument("--port", type=int, default=8020)
    args = parser.parse_args()
    if not 1 <= args.port <= 65535:
        parser.error("invalid port")
    logging.basicConfig(level=logging.INFO, format="%(asctime)s %(levelname)s %(message)s")
    from runtime import RknnEngine
    engine = RknnEngine(args.assets)
    server = Server(("127.0.0.1", args.port), Service(engine))
    LOG.info("RKNN runtime initialized; listening on 127.0.0.1:%d", args.port)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()
        engine.close()


if __name__ == "__main__":
    main()
