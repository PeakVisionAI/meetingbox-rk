#!/usr/bin/env python3
"""Check external RK ASR assets and protocol; optionally run a supplied WAV."""
import argparse
import hashlib
import json
import platform
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid
import wave
from pathlib import Path


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def request_json(url, data=None, content_type=None, timeout=5):
    headers = {"Content-Type": content_type} if content_type else {}
    request = urllib.request.Request(url, data=data, headers=headers)
    with urllib.request.build_opener(NoRedirect()).open(request, timeout=timeout) as response:
        body = response.read(1024 * 1024 + 1)
        if len(body) > 1024 * 1024:
            raise ValueError("response exceeds 1 MiB")
        result = json.loads(body)
        if not isinstance(result, dict):
            raise ValueError("response must be a JSON object")
        return result


def reports_npu(payload):
    return payload.get("backend") == "sensevoice-rknn" and payload.get("device") in ("npu", "rknn", "rk3588-npu")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--url", default="http://127.0.0.1:8020")
    parser.add_argument("--audio", type=Path, help="Optional 16kHz mono PCM16 WAV; sends audio to the specified service")
    parser.add_argument("--model", type=Path, help="Optional RKNN file to inventory and hash")
    parser.add_argument("--sha256", help="Expected model SHA-256")
    parser.add_argument("--timeout", type=int, default=90)
    args = parser.parse_args()
    parsed = urllib.parse.urlsplit(args.url)
    if parsed.scheme not in ("http", "https") or not parsed.hostname or parsed.username or parsed.password or parsed.query or parsed.fragment:
        parser.error("URL must be HTTP(S) without credentials, query or fragment")
    if not 1 <= args.timeout <= 180:
        parser.error("timeout must be between 1 and 180 seconds")
    if args.sha256 and not args.model:
        parser.error("--sha256 requires --model")
    report = {"host": {"system": platform.system(), "machine": platform.machine()}, "hardware_verified": False}
    try:
        if args.model:
            digest = hashlib.sha256()
            with args.model.open("rb") as file:
                for block in iter(lambda: file.read(1024 * 1024), b""):
                    digest.update(block)
            sha = digest.hexdigest()
            report["model_asset"] = {"bytes": args.model.stat().st_size, "sha256": sha}
            if args.sha256 and sha.lower() != args.sha256.lower():
                raise ValueError("model checksum mismatch")
        endpoint = args.url.rstrip("/")
        health = request_json(endpoint + "/healthz")
        report["health"] = {key: health.get(key) for key in ("ready", "backend", "model", "device")}
        if health.get("ready") is not True or not reports_npu(health):
            raise ValueError("service is not ready or does not report SenseVoice RKNN")
        if args.audio:
            with wave.open(str(args.audio), "rb") as audio:
                if (audio.getframerate(), audio.getnchannels(), audio.getsampwidth(), audio.getcomptype()) != (16000, 1, 2, "NONE"):
                    raise ValueError("audio must be 16kHz mono uncompressed PCM16 WAV")
                duration = audio.getnframes() / 16000
                if not 1 <= duration <= 25:
                    raise ValueError("use a test WAV between 1 and 25 seconds")
            boundary = "meetingbox-" + uuid.uuid4().hex
            fields = {"meeting_id": "0", "chunk_start_ms": "0", "hotwords": "[]", "skip_vad": "0"}
            body = bytearray()
            for key, value in fields.items():
                body.extend(f'--{boundary}\r\nContent-Disposition: form-data; name="{key}"\r\n\r\n{value}\r\n'.encode())
            body.extend(f'--{boundary}\r\nContent-Disposition: form-data; name="audio"; filename="probe.wav"\r\nContent-Type: audio/wav\r\n\r\n'.encode())
            body.extend(args.audio.read_bytes())
            body.extend(f"\r\n--{boundary}--\r\n".encode())
            started = time.monotonic()
            result = request_json(endpoint + "/v1/asr/transcribe-upload", bytes(body), f"multipart/form-data; boundary={boundary}", args.timeout)
            elapsed = time.monotonic() - started
            report["inference"] = {"elapsed_seconds": round(elapsed, 3), "duration_seconds": duration,
                                   "rtf": round(elapsed / duration, 3), "backend": result.get("backend"),
                                   "device": result.get("device"), "model": result.get("model"),
                                   "segment_count": len(result["segments"]) if isinstance(result.get("segments"), list) else None}
            if not reports_npu(result):
                raise ValueError("inference did not report SenseVoice RKNN")
            segments = result.get("segments")
            if not isinstance(segments, list) or result.get("error") is not None or any(
                not isinstance(segment, dict) or not isinstance(segment.get("text"), str) for segment in segments
            ):
                raise ValueError("inference response has invalid segments or reports failure")
        report["protocol_passed"] = True
        report["note"] = "Service metadata is not proof of NPU execution; record target driver/runtime and NPU activity separately."
        print(json.dumps(report, ensure_ascii=False, indent=2))
        return 0
    except (OSError, ValueError, urllib.error.URLError, wave.Error) as error:
        # Do not print service response bodies, URLs with credentials, or transcripts.
        report["protocol_passed"] = False
        report["error"] = str(error) if isinstance(error, ValueError) else type(error).__name__
        print(json.dumps(report, ensure_ascii=False, indent=2))
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
