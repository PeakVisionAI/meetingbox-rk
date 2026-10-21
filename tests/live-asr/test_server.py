"""Verify wrapper ownership without loading a speech model."""
import asyncio
from contextlib import asynccontextmanager
import importlib.util
import os
from pathlib import Path
import sys
import types
import unittest
from unittest.mock import patch

SERVER = Path(__file__).resolve().parents[2] / 'services/live-asr/server.py'


class WrapperTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self):
        self.calls = []
        self.started = asyncio.Event()
        self.release = asyncio.Event()
        self.running = 0
        self.maximum_running = 0
        self.routes = []

        class App:
            router = types.SimpleNamespace(routes=[
                types.SimpleNamespace(path=path) for path in
                ('/health', '/warmup', '/asr/transcribe', '/asr/transcribe-file', '/voice/feed')
            ])

            def post(inner, path):
                self.routes.append(path)
                return lambda function: function

        async def transcribe(*args):
            self.calls.append(args)
            self.running += 1
            self.maximum_running = max(self.maximum_running, self.running)
            self.started.set()
            try:
                await self.release.wait()
                return {'text': 'preview', 'final': args[-1]}
            finally:
                self.running -= 1

        @asynccontextmanager
        async def original_lifespan(app):
            yield

        App.router.lifespan_context = original_lifespan
        self.upstream = types.SimpleNamespace(
            ASR_MODE='streaming', PRELOAD_FILE_ASR=False, PORT=8002,
            app=App(), transcribe=transcribe, STREAM_CACHE_TTL_SECONDS=600,
            STREAM_SESSIONS=types.SimpleNamespace(sessions={}), cleanup_stream_caches=lambda now: None,
        )
        self.model_calls = []
        def model(*args, **kwargs):
            self.model_calls.append((args, kwargs))
            return 'loaded'
        self.funasr = types.SimpleNamespace(AutoModel=model)
        self.patch = patch.dict(sys.modules, {'server': self.upstream, 'funasr': self.funasr})
        self.patch.start()
        self.env = patch.dict(os.environ, {'BOSSAI_ASR_SERVER_DIR': '/tmp/pinned-asr',
                                          'BOSSAI_LOCAL_CPU_THREADS': '2'})
        self.env.start()
        self.saved_path = list(sys.path)
        spec = importlib.util.spec_from_file_location('meetingbox_live_wrapper_test', SERVER)
        self.module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(self.module)

    async def asyncTearDown(self):
        self.release.set()
        self.env.stop()
        self.patch.stop()
        sys.path[:] = self.saved_path

    async def test_disconnect_keeps_global_inference_ownership_until_completion(self):
        first = asyncio.create_task(self.module.transcribe('audio-1', 'auto', 0, .003, 'one', False))
        await self.started.wait()
        first.cancel()
        second = asyncio.create_task(self.module.transcribe('audio-2', 'auto', 0, .003, 'two', False))
        await asyncio.sleep(0)
        await asyncio.sleep(0)
        self.assertEqual(len(self.calls), 1)
        self.assertFalse(first.done())
        self.release.set()
        with self.assertRaises(asyncio.CancelledError):
            await first
        self.assertEqual((await second)['text'], 'preview')
        self.assertEqual(self.maximum_running, 1)
        self.assertEqual(self.calls[1], ('audio-2', 'auto', 0, .003, 'two', False))

    async def test_file_route_cannot_load_a_second_model(self):
        from fastapi import HTTPException
        self.assertEqual([r.path for r in self.upstream.app.router.routes], ['/health', '/warmup'])
        self.assertEqual(self.routes, ['/asr/transcribe', '/asr/transcribe-file'])
        with self.assertRaises(HTTPException) as error:
            await self.module.file_disabled()
        self.assertEqual(error.exception.status_code, 409)
        self.assertEqual(self.model_calls, [])

    async def test_cpu_budget_is_passed_to_model_constructor(self):
        result = self.funasr.AutoModel(model='/local/model')
        self.assertEqual(result, 'loaded')
        self.assertEqual(self.model_calls, [((), {'model': '/local/model', 'ncpu': 2, 'disable_update': True, 'disable_pbar': True})])

    async def test_idle_expiry_keeps_active_and_recent_sessions(self):
        called = []
        self.upstream.cleanup_stream_caches = called.append
        sessions = self.upstream.STREAM_SESSIONS.sessions
        sessions.update({
            'stale': types.SimpleNamespace(users=0, accessed_at=0),
            'active': types.SimpleNamespace(users=1, accessed_at=0),
            'recent': types.SimpleNamespace(users=0, accessed_at=900),
        })
        self.module.expire_idle_sessions(1000)
        self.assertEqual(called, [1000])
        self.assertEqual(set(sessions), {'active', 'recent'})

    async def test_periodic_cleanup_task_is_joined_on_shutdown(self):
        async with self.upstream.app.router.lifespan_context(self.upstream.app):
            self.assertTrue(any(t.get_coro().__name__ == 'expire_periodically'
                                for t in asyncio.all_tasks()))
        self.assertFalse(any(t.get_coro().__name__ == 'expire_periodically'
                             for t in asyncio.all_tasks()))

    async def test_int8_changes_encoder_only_and_preserves_decoder(self):
        encoder, decoder, quantized = object(), object(), object()
        wrapped = types.SimpleNamespace(model=types.SimpleNamespace(encoder=encoder, decoder=decoder))
        calls = []
        def quantize(model, layers, **kwargs):
            calls.append((model, layers, kwargs))
            return quantized
        linear, qint8 = object(), object()
        engine = types.SimpleNamespace(engine='default')
        torch = types.SimpleNamespace(
            backends=types.SimpleNamespace(quantized=engine), nn=types.SimpleNamespace(Linear=linear),
            qint8=qint8, ao=types.SimpleNamespace(quantization=types.SimpleNamespace(quantize_dynamic=quantize)),
        )
        self.module.original_auto_model = lambda **kwargs: wrapped
        with patch.dict(os.environ, {'BOSSAI_STREAM_INT8': '1'}), patch.dict(sys.modules, {'torch': torch}):
            self.assertIs(self.funasr.AutoModel(model='/local/model'), wrapped)
        self.assertIs(wrapped.model.encoder, quantized)
        self.assertIs(wrapped.model.decoder, decoder)
        self.assertEqual(calls, [(encoder, {linear}, {'dtype': qint8, 'inplace': True})])
        self.assertEqual(engine.engine, 'qnnpack')


if __name__ == '__main__':
    unittest.main()
