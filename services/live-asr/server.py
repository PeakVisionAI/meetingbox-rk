#!/usr/bin/env python3
"""Serve only the pinned BossAI streaming engine, never its file model."""
import asyncio
from contextlib import asynccontextmanager
import os
from pathlib import Path
import sys
import time

import uvicorn
from fastapi import File, Form, HTTPException, UploadFile

source = Path(os.environ['BOSSAI_ASR_SERVER_DIR']).resolve()
sys.path.insert(0, str(source))
import server as upstream

if upstream.ASR_MODE != 'streaming' or upstream.PRELOAD_FILE_ASR:
    raise RuntimeError('Live ASR requires streaming mode and file preload disabled')
# The pinned upstream omits ncpu. Supply it at the existing AutoModel boundary
# so both loading and subsequent inference use the same explicit CPU budget.
import funasr

cpu_threads = max(1, int(os.environ.get('BOSSAI_LOCAL_CPU_THREADS', '1')))
original_auto_model = funasr.AutoModel


def streaming_auto_model(*args, **kwargs):
    kwargs.setdefault('ncpu', cpu_threads)
    kwargs.setdefault('disable_update', True)
    kwargs.setdefault('disable_pbar', True)
    model = original_auto_model(*args, **kwargs)
    if os.environ.get('BOSSAI_STREAM_INT8', '0') == '1':
        import torch

        # Keep decoder/predictor precision: quantizing them changes preview words.
        torch.backends.quantized.engine = 'qnnpack'
        model.model.encoder = torch.ao.quantization.quantize_dynamic(
            model.model.encoder, {torch.nn.Linear}, dtype=torch.qint8, inplace=True)
    return model


funasr.AutoModel = streaming_auto_model
upstream.app.router.routes = [route for route in upstream.app.router.routes
                              if getattr(route, 'path', None) in ('/health', '/warmup')]
inference_lock = asyncio.Lock()


def expire_idle_sessions(now):
    upstream.cleanup_stream_caches(now)
    sessions = upstream.STREAM_SESSIONS.sessions
    for key, session in list(sessions.items()):
        if session.users == 0 and now - session.accessed_at > upstream.STREAM_CACHE_TTL_SECONDS:
            sessions.pop(key, None)


async def expire_periodically():
    while True:
        await asyncio.sleep(60)
        async with inference_lock:
            expire_idle_sessions(time.monotonic())


upstream_lifespan = upstream.app.router.lifespan_context


@asynccontextmanager
async def lifespan(app):
    async with upstream_lifespan(app):
        cleanup = asyncio.create_task(expire_periodically())
        try:
            yield
        finally:
            cleanup.cancel()
            await asyncio.gather(cleanup, return_exceptions=True)


upstream.app.router.lifespan_context = lifespan


@upstream.app.post('/asr/transcribe')
async def transcribe(audio: UploadFile = File(...), language: str = Form('auto'),
                     chunk_index: int = Form(...), min_rms: float = Form(0.003),
                     session_id: str = Form(...), is_final: bool = Form(False)):
    async with inference_lock:
        # A disconnected request must not release model ownership while its
        # inference thread still runs.
        task = asyncio.create_task(upstream.transcribe(
            audio, language, chunk_index, min_rms, session_id, is_final))
        try:
            return await asyncio.shield(task)
        except asyncio.CancelledError:
            try:
                await task
            finally:
                raise


@upstream.app.post('/asr/transcribe-file')
async def file_disabled():
    raise HTTPException(409, 'This worker exposes only streaming transcription')


if __name__ == '__main__':
    uvicorn.run(upstream.app, host='127.0.0.1', port=upstream.PORT)
