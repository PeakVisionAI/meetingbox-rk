#!/usr/bin/env python3
"""On-device smoke test for the RK bridge.

Drives the bridge exactly the way the desktop app does: 200 ms PCM windows on
`/v1/asr/stream/{meeting}`, then `/end`. No dependencies beyond the standard
library, so it runs on the RK image as-is.

    # 1) both services up?
    python3 smoke_rk.py --health-only

    # 2) synthetic tone: proves transport + plumbing, not recognition quality
    python3 smoke_rk.py --tone 3

    # 3) real recording (16 kHz mono 16-bit WAV)
    python3 smoke_rk.py --wav sample.wav

Exit code 0 means every expectation passed; 1 means a check failed (details are
printed). This is what "能测" looks like before the Tauri app is even built.
"""

import argparse
import base64
import json
import math
import struct
import sys
import time
import urllib.error
import urllib.request
import wave

SAMPLE_RATE = 16000
CHUNK_MS = 200
CHUNK_BYTES = SAMPLE_RATE * 2 * CHUNK_MS // 1000
# A fresh id per run: the bridge keeps per-meeting stream state for one hour and
# rejects replayed chunk indexes, so a fixed id would fail on the second run.
def default_meeting_id():
    return int(time.time()) % 900_000 + 1_000


def http_get(url, timeout=5):
    with urllib.request.urlopen(url, timeout=timeout) as response:
        return response.status, json.load(response)


def http_post(url, body, headers, timeout=60):
    request = urllib.request.Request(url, data=body, headers=headers, method="POST")
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            return response.status, json.load(response)
    except urllib.error.HTTPError as error:
        detail = error.read().decode("utf-8", "replace")[:200]
        return error.code, {"error": detail}


def check_health(bridge, asr):
    ok = True
    print(f"[1/4] bridge  {bridge}/healthz")
    try:
        _, payload = http_get(bridge + "/healthz")
        print(f"      ready={payload.get('ready')} backend={payload.get('backend')} "
              f"engine={payload.get('engine')}")
        if payload.get("speakers") is not None:
            print(f"      speakers={payload.get('speakers')}")
            print(f"      punctuation={payload.get('punctuation')} "
                  f"alignment={payload.get('alignment')}")
        ok = bool(payload.get("ready"))
    except Exception as error:
        print(f"      FAIL {type(error).__name__}: {error}")
        ok = False

    print(f"[2/4] rk-asr  {asr}/healthz")
    try:
        _, payload = http_get(asr + "/healthz")
        print(f"      ready={payload.get('ready')} model={payload.get('model')} "
              f"npu={payload.get('device')}")
        ok = ok and payload.get("ready") is True
    except Exception as error:
        print(f"      FAIL {type(error).__name__}: {error}")
        ok = False
    return ok


def read_wav(path):
    with wave.open(path, "rb") as audio:
        if (audio.getframerate(), audio.getnchannels(), audio.getsampwidth()) != (SAMPLE_RATE, 1, 2):
            raise SystemExit(
                f"{path}: need 16 kHz mono 16-bit, got {audio.getframerate()} Hz / "
                f"{audio.getnchannels()} ch / {audio.getsampwidth() * 8} bit.\n"
                f"  convert: ffmpeg -i in.wav -ar 16000 -ac 1 -c:a pcm_s16le out.wav"
            )
        return audio.readframes(audio.getnframes())


def tone(seconds):
    frames = int(SAMPLE_RATE * seconds)
    samples = bytearray()
    for index in range(frames):
        value = int(6000 * math.sin(2 * math.pi * 220 * index / SAMPLE_RATE))
        samples += struct.pack("<h", value)
    return bytes(samples)


def stream(bridge, pcm, meeting_id):
    stream_url = f"{bridge}/v1/asr/stream/{meeting_id}"
    segments, finals, partials = {}, 0, 0
    started = time.monotonic()
    total = 0

    for index, offset in enumerate(range(0, len(pcm), CHUNK_BYTES)):
        chunk = pcm[offset:offset + CHUNK_BYTES]
        if not chunk:
            continue
        total += 1
        status, payload = http_post(
            f"{stream_url}?chunk_index={index}",
            chunk,
            {
                "Content-Type": "application/octet-stream",
                "X-Meeting-Hotwords": base64.b64encode(b"[]").decode(),
            },
        )
        if status != 200:
            print(f"      chunk {index} -> HTTP {status}: {payload}")
            return segments, finals, partials, total, time.monotonic() - started
        for segment in payload.get("segments", []):
            segments[segment.get("utterance")] = segment
            if segment.get("is_partial"):
                partials += 1
            else:
                finals += 1

    status, payload = http_post(f"{stream_url}/end", b"", {})
    if status == 200:
        for segment in payload.get("segments", []):
            segments[segment.get("utterance")] = segment
            if not segment.get("is_partial"):
                finals += 1
    else:
        print(f"      end -> HTTP {status}: {payload}")
    return segments, finals, partials, total, time.monotonic() - started


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--bridge", default="http://127.0.0.1:8130")
    parser.add_argument("--asr", default="http://127.0.0.1:8020")
    parser.add_argument("--wav")
    parser.add_argument("--tone", type=float, help="generated sine seconds (plumbing only)")
    parser.add_argument("--health-only", action="store_true")
    parser.add_argument("--meeting-id", type=int, default=None)
    args = parser.parse_args()

    args.bridge = args.bridge.rstrip("/")
    args.asr = args.asr.rstrip("/")

    healthy = check_health(args.bridge, args.asr)
    if args.health_only or not healthy:
        print("\nRESULT:", "PASS" if healthy else "FAIL")
        return 0 if healthy else 1

    if args.wav:
        pcm = read_wav(args.wav)
    elif args.tone:
        pcm = tone(args.tone)
    else:
        print("[3/4] skip audio (pass --wav or --tone)")
        print("\nRESULT: PASS (health only)")
        return 0

    print(f"[3/4] streaming {len(pcm) / (SAMPLE_RATE * 2):.1f}s in {CHUNK_MS}ms windows")
    meeting_id = args.meeting_id or default_meeting_id()
    print(f"      meeting_id={meeting_id}")
    segments, finals, partials, chunks, elapsed = stream(args.bridge, pcm, meeting_id)

    print(f"[4/4] chunks={chunks} finals={finals} partials={partials} "
          f"elapsed={elapsed:.2f}s")
    speakers = sorted({s.get("speaker") for s in segments.values() if s.get("speaker")})
    print(f"      speakers={speakers or 'none (no speaker model)'}")
    for segment in list(sorted(segments.values(), key=lambda s: s.get("start_ms") or 0))[:5]:
        text = (segment.get("text") or "").strip()
        print(f"      [{segment.get('start_ms')}ms] {segment.get('speaker') or '-'}: {text[:60]}")

    failed = []
    if finals == 0:
        failed.append("no final segment (check model output / silence threshold)")
    if elapsed > len(pcm) / (SAMPLE_RATE * 2) * 3:
        failed.append("slower than 3x realtime; raise MEETINGBOX_RK_STRIDE_MS")
    print("\nRESULT:", "FAIL: " + "; ".join(failed) if failed else "PASS")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
