"""`/voice/*` 接口。`mount()` 是 `server.py` 唯一需要调用的入口。"""
from __future__ import annotations

import asyncio
import io
import logging
from typing import Any, Callable, Optional

import numpy as np
import soundfile as sf
from fastapi import APIRouter, FastAPI, File, Form, HTTPException, Request, UploadFile

from .service import VoiceFront, VoiceFrontError
from .settings import SAMPLE_RATE, VoiceFrontSettings

log = logging.getLogger("bossai.voice_front")

__all__ = ["mount", "decode_chunk"]

_MODES = {"wake", "wake_free"}


def decode_chunk(data: bytes, max_seconds: float) -> np.ndarray:
    """把上传的一块 WAV 解成 16kHz 单声道 float32。音频只在内存里，不写临时文件。"""
    if not data:
        raise HTTPException(status_code=400, detail={"code": "bad_audio", "message": "音频为空"})
    try:
        pcm, sample_rate = sf.read(io.BytesIO(data), dtype="float32", always_2d=False)
    except Exception as exc:  # noqa: BLE001
        raise HTTPException(status_code=400,
                            detail={"code": "bad_audio", "message": "音频不是有效的 WAV"}) from exc
    if sample_rate != SAMPLE_RATE:
        raise HTTPException(status_code=400,
                            detail={"code": "bad_audio", "message": "音频必须是 16kHz"})
    if pcm.ndim > 1:
        pcm = pcm.mean(axis=1)
    if pcm.size == 0 or pcm.size > int(max_seconds * SAMPLE_RATE):
        raise HTTPException(status_code=400,
                            detail={"code": "bad_audio", "message": "单块音频长度不合法"})
    return np.ascontiguousarray(pcm, dtype=np.float32)


def _raise(error: VoiceFrontError) -> None:
    raise HTTPException(status_code=error.status,
                        detail={"code": error.code, "message": str(error)})


def mount(app: FastAPI, *, file_engine: Any, clean_text: Callable[[str], str],
          settings: Optional[VoiceFrontSettings] = None, stream_engine: Any = None,
          stream_encoder_look_back: int = 4, stream_decoder_look_back: int = 1) -> VoiceFront:
    """把语音前段挂到 sidecar 上。

    `file_engine` 是 sidecar 已有的文件转写引擎：短句识别直接用它已经加载好的
    SenseVoice，只跑主模型，不经过它的切句和说话人模型，也不另占一份内存。
    """
    settings = settings or VoiceFrontSettings.from_env()

    async def recognize(wav: np.ndarray) -> str:
        model = await file_engine.model()

        def run() -> str:
            # 传一份浅拷贝：inference 会把本次参数写回它拿到的 kwargs，
            # 直接传 model.kwargs 会把 language、use_itn 泄漏给同时在跑的文件转写。
            result = model.inference(wav, model=model.model, kwargs=dict(model.kwargs),
                                     language=settings.language, use_itn=settings.use_itn,
                                     disable_pbar=True)
            return str(result[0].get("text", "")) if result else ""

        return clean_text(await asyncio.to_thread(run))

    front = VoiceFront(settings, recognize=recognize, stream_engine=stream_engine,
                       clean_text=clean_text, stream_encoder_look_back=stream_encoder_look_back,
                       stream_decoder_look_back=stream_decoder_look_back)
    router = APIRouter(prefix="/voice")

    @router.get("/health")
    async def health() -> dict[str, Any]:
        return front.health()

    @router.post("/wake-word/validate")
    async def validate_wake_word(wake_word: str = Form("")) -> dict[str, str]:
        try:
            return {"wakeWord": await front.validate_wake_word(wake_word)}
        except VoiceFrontError as error:
            _raise(error)
            raise  # pragma: no cover

    @router.post("/feed")
    async def feed(
        request: Request,
        audio: UploadFile = File(...),
        session_id: str = Form(...),
        seq: int = Form(...),
        mode: str = Form("wake"),
        expect_followup: bool = Form(False),
        barge_active: bool = Form(False),
        suppress: bool = Form(False),
        wake_word: Optional[str] = Form(None),
    ) -> dict[str, Any]:
        session_key = session_id.strip()
        if not session_key or len(session_key) > 128 or seq < 0 or mode not in _MODES:
            raise HTTPException(status_code=400,
                                detail={"code": "bad_request", "message": "参数不合法"})
        # FastAPI 把空表单字符串视为默认值；显式空词仍应走 400 校验，不能退回默认词。
        if wake_word is None and "wake_word" in await request.form():
            wake_word = ""
        pcm = decode_chunk(await audio.read(), settings.max_chunk_s)
        try:
            return await front.feed(
                session_id=session_key, seq=seq, pcm=pcm, mode=mode,  # type: ignore[arg-type]
                expect_followup=expect_followup, barge_active=barge_active, suppress=suppress,
                wake_word=wake_word,
            )
        except VoiceFrontError as error:
            _raise(error)
            raise  # pragma: no cover

    @router.post("/reset")
    async def reset(session_id: str = Form(...)) -> dict[str, Any]:
        return {"ok": True, "existed": front.reset(session_id.strip())}

    @router.post("/addressee")
    async def addressee(text: str = Form(...), context: str = Form("")) -> dict[str, Any]:
        """只给验收用：文字进，分数出。页面不调用它。"""
        try:
            return await front.addressee_score(text, context)
        except VoiceFrontError as error:
            _raise(error)
            raise  # pragma: no cover

    app.include_router(router)

    @app.on_event("startup")
    async def _warm_voice_front() -> None:
        # 后台预热，不挡住 sidecar 自己的启动和健康检查。
        task = asyncio.create_task(front.warm())
        front._tasks.add(task)  # noqa: SLF001
        task.add_done_callback(front._tasks.discard)  # noqa: SLF001

    return front
