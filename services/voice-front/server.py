"""BossAI VoiceFront with short-command recognition through the existing RK ASR.

Audio remains in memory. No additional FunASR/PyTorch model is loaded.
"""
from __future__ import annotations

import asyncio
from contextlib import asynccontextmanager
from dataclasses import replace
import io
import os
from pathlib import Path
import sys
import wave

import httpx
import numpy as np
from fastapi import FastAPI, File, Form, HTTPException, Request, UploadFile
import uvicorn

sys.path.insert(0, str(Path(__file__).parent / "vendor"))
from voice_front.router import decode_chunk
from voice_front.service import VoiceFront, VoiceFrontError
from voice_front.settings import VoiceFrontSettings
from voice_front.kws import KwsEngine
import voice_front.service as voice_service


class MeetingboxKwsEngine(KwsEngine):
    def __init__(self, *args, **kwargs):
        threads = int(os.environ.get("MEETINGBOX_KWS_THREADS", "1"))
        if not 1 <= threads <= 4:
            raise ValueError("MEETINGBOX_KWS_THREADS must be between 1 and 4")
        kwargs["num_threads"] = threads
        super().__init__(*args, **kwargs)


# Keep the pinned vendor files intact; limit KWS workers in this adapter.
voice_service.KwsEngine = MeetingboxKwsEngine

ASR_URL = os.environ.get("MEETINGBOX_RK_ASR_URL", "http://127.0.0.1:8020").rstrip("/")
settings = replace(VoiceFrontSettings.from_env(), wake_timeout_s=20.0,
                   followup_window_s=20.0, max_utterance_s=10.0,
                   max_pending_audio_s=12.0, max_sessions=2)


def pcm_wav(samples: np.ndarray) -> bytes:
    output = io.BytesIO()
    with wave.open(output, "wb") as stream:
        stream.setnchannels(1)
        stream.setsampwidth(2)
        stream.setframerate(16000)
        stream.writeframes((np.clip(samples, -1, 1) * 32767).astype("<i2").tobytes())
    return output.getvalue()


async def recognize(samples: np.ndarray) -> str:
    async with httpx.AsyncClient(timeout=20.0, trust_env=False) as client:
        response = await client.post(
            ASR_URL + "/v1/asr/transcribe-upload",
            files={"audio": ("command.wav", pcm_wav(samples), "audio/wav")},
            data={"meeting_id": "0", "chunk_start_ms": "0", "skip_vad": "1"},
        )
        response.raise_for_status()
        payload = response.json()
    if payload.get("error") or not isinstance(payload.get("segments"), list):
        raise RuntimeError("command recognition failed")
    return "".join(str(segment.get("text", "")) for segment in payload["segments"])


front = VoiceFront(settings, recognize=recognize)


@asynccontextmanager
async def lifespan(app: FastAPI):
    await front.warm()
    yield
    for session in list(front._entries):
        front.reset(session)
    for task in list(front._tasks):
        task.cancel()
    await asyncio.gather(*front._tasks, return_exceptions=True)


app = FastAPI(lifespan=lifespan, docs_url=None, redoc_url=None, openapi_url=None)


@app.middleware("http")
async def limit_upload(request: Request, call_next):
    # Bound requests before multipart parsing. Native client always sends a length.
    if request.method == "POST":
        length = request.headers.get("content-length", "")
        if not length.isdecimal() or int(length) > 40000:
            from fastapi.responses import JSONResponse
            return JSONResponse({"detail": "bounded Content-Length required"}, status_code=413)
    return await call_next(request)


@app.get("/voice/health")
async def health():
    result = front.health()
    result["commandAsr"] = ASR_URL
    result["mode"] = "wake"
    result["maxUtteranceSeconds"] = settings.max_utterance_s
    return result


@app.post("/voice/feed")
async def feed(audio: UploadFile = File(...), session_id: str = Form(...),
               seq: int = Form(...), mode: str = Form("wake"),
               expect_followup: bool = Form(False), barge_active: bool = Form(False),
               suppress: bool = Form(False)):
    if not session_id.strip() or len(session_id) > 128 or seq < 0 or mode != "wake":
        raise HTTPException(400, "invalid wake feed")
    data = await audio.read(40001)
    # Validate the original container before decoding; never decode a truncated prefix.
    try:
        with wave.open(io.BytesIO(data), "rb") as stream:
            count = stream.getnframes()
            if (stream.getframerate(), stream.getnchannels(), stream.getsampwidth()) != (16000, 1, 2):
                raise ValueError("16 kHz mono PCM16 required")
            if not 0 < count <= 8000 or len(stream.readframes(count)) != count * 2:
                raise ValueError("invalid or truncated frame count")
    except (wave.Error, EOFError, RuntimeError, ValueError) as error:
        raise HTTPException(400, "invalid 16 kHz mono PCM16 WAV") from error
    pcm = decode_chunk(data, 0.5)
    try:
        return await front.feed(session_id=session_id.strip(), seq=seq, pcm=pcm,
                                mode="wake", expect_followup=expect_followup,
                                barge_active=barge_active, suppress=suppress)
    except VoiceFrontError as error:
        raise HTTPException(error.status, {"code": error.code, "message": str(error)}) from error


@app.post("/voice/reset")
async def reset(session_id: str = Form(...)):
    return {"ok": True, "existed": front.reset(session_id.strip())}


if __name__ == "__main__":
    uvicorn.run(app, host="127.0.0.1", port=int(os.environ.get("MEETINGBOX_VOICE_PORT", "8021")),
                access_log=False, limit_concurrency=8, timeout_keep_alive=5)
