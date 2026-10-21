#!/usr/bin/env python3
"""Supervise a single local streaming model and verify its warmup contract."""
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import time
import urllib.error
import urllib.request


def main():
    source = Path(os.environ['BOSSAI_ASR_SERVER_DIR']).resolve()
    model = Path(os.environ['BOSSAI_STREAM_MODEL']).resolve()
    if not (source / 'server.py').is_file() or not (model / 'model.pt').is_file():
        raise RuntimeError('Pinned ASR source or streaming model is missing')
    port = int(os.environ.get('ASR_PORT', '8002'))
    threads = str(max(1, int(os.environ.get('BOSSAI_LOCAL_CPU_THREADS', '1'))))
    env = {**os.environ, 'PYTHONNOUSERSITE': '1', 'PYTHONUNBUFFERED': '1',
           'FUNASR_MODE': 'streaming', 'FUNASR_MODEL': str(model),
           'FUNASR_DEVICE': 'cpu', 'FUNASR_HUB': 'ms',
           'FUNASR_ENABLE_SPEAKER': '0', 'BOSSAI_ASR_PRELOAD': '1',
           'BOSSAI_ASR_PRELOAD_FILE': '0', 'BOSSAI_VOICE_FRONT': '0',
           'BOSSAI_VOICE_TTS': '0', 'ASR_PORT': str(port),
           'OMP_NUM_THREADS': threads, 'MKL_NUM_THREADS': threads,
           'NUMEXPR_NUM_THREADS': threads, 'BOSSAI_LOCAL_CPU_THREADS': threads,
           'TOKENIZERS_PARALLELISM': 'false'}
    child = subprocess.Popen([sys.executable, str(Path(__file__).with_name('server.py'))],
                             cwd=source, env=env)
    stopping = False

    def terminate(signum, frame):
        nonlocal stopping
        stopping = True
        if child.poll() is None:
            child.terminate()

    signal.signal(signal.SIGTERM, terminate)
    signal.signal(signal.SIGINT, terminate)
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    ready = False
    started = time.monotonic()
    try:
        while child.poll() is None and not stopping and time.monotonic() - started < 120:
            try:
                with opener.open(f'http://127.0.0.1:{port}/health', timeout=3) as response:
                    health = json.load(response)
                if health.get('fileLoaded'):
                    raise RuntimeError('Unexpected file model loaded in streaming worker')
                if health.get('warmupError'):
                    raise RuntimeError('Streaming model warmup failed')
                if (health.get('ok') and health.get('backend') == 'funasr'
                        and health.get('mode') == 'streaming' and health.get('loaded')
                        and health.get('ready')):
                    ready = True
                    break
            except (urllib.error.URLError, TimeoutError, ConnectionError):
                pass
            time.sleep(.5)
        if not ready:
            if stopping:
                return 0
            raise RuntimeError('Streaming model was not ready within 120 seconds')
        print('live-asr: streaming ready; file engine disabled', flush=True)
        result = child.wait()
        return 0 if stopping else (result if result else 1)
    finally:
        if child.poll() is None:
            child.terminate()
            try:
                child.wait(timeout=20)
            except subprocess.TimeoutExpired:
                child.kill()
                child.wait()


if __name__ == '__main__':
    raise SystemExit(main())
