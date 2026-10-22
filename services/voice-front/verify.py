"""Feed a real wake fixture without microphone capture or meeting operations."""
import argparse
import io
import json
from pathlib import Path
import time
import uuid
import wave

import httpx


def wav(data):
    output = io.BytesIO()
    with wave.open(output, "wb") as f:
        f.setnchannels(1)
        f.setsampwidth(2)
        f.setframerate(16000)
        f.writeframes(data)
    return output.getvalue()


def verify(endpoint, fixture):
    with wave.open(str(fixture), "rb") as f:
        if (f.getframerate(), f.getnchannels(), f.getsampwidth()) != (16000, 1, 2):
            raise ValueError("fixture must be 16 kHz mono PCM16")
        data = f.readframes(f.getnframes())
    session = "verification-" + uuid.uuid4().hex
    events = []
    with httpx.Client(base_url=endpoint, timeout=25, trust_env=False) as client:
        health = client.get("/voice/health").json()
        if not (health.get("available") and health.get("kws", {}).get("loaded")
                and health.get("vad", {}).get("loaded")):
            raise RuntimeError("wake models not ready")
        try:
            chunks = [data[i:i + 6400].ljust(6400, b"\0") for i in range(0, len(data), 6400)]
            chunks += [b"\0" * 6400] * 30
            for seq, chunk in enumerate(chunks):
                response = client.post("/voice/feed", files={"audio": ("frame.wav", wav(chunk), "audio/wav")},
                                       data={"session_id": session, "seq": str(seq), "mode": "wake"})
                response.raise_for_status()
                events.extend(response.json()["events"])
                time.sleep(0.02)
        finally:
            client.post("/voice/reset", data={"session_id": session}).raise_for_status()
    types = [event["type"] for event in events]
    return {"health": health, "fixture": Path(fixture).name, "events": types,
            "wake_detected": "wake" in types,
            "note": "Recorded fixture through real KWS/VAD; no microphone or desktop command acceptance."}


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("fixture", type=Path)
    parser.add_argument("--endpoint", default="http://127.0.0.1:8021")
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    result = verify(args.endpoint, args.fixture)
    if args.output:
        with args.output.open("x") as f:
            json.dump(result, f, ensure_ascii=False, indent=2)
            f.write("\n")
    print(json.dumps(result, ensure_ascii=False, indent=2))
    if not result["wake_detected"]:
        raise SystemExit(1)
