"""MeetingBox's existing RK HTTP contract backed by unmodified AsrServe.

Chunk speakers are deliberately provisional. Full-meeting inference establishes
recording-local speaker identities. Raw and corrected text are persisted separately.
"""
import asyncio
import base64
import io
import json
import os
import re
import uuid
import wave
import time
from pathlib import Path

import httpx
from fastapi import FastAPI, File, Form, HTTPException, UploadFile, Request
from pydantic import BaseModel, Field
from summary_memory import memory_messages,validate_state

app = FastAPI(title="MeetingBox AsrServe compatibility service")
ASR_URL = os.getenv("ASRSERVE_URL", "http://127.0.0.1:17003").rstrip("/")
LLM_URL = os.getenv("MEETINGBOX_LLM_URL", "http://127.0.0.1:11434/v1").rstrip("/")
LLM_MODEL = os.getenv("MEETINGBOX_LLM_MODEL", "qwen3:4b-instruct")
DATA = Path(os.getenv("MEETINGBOX_BRIDGE_DATA", "./meeting-data"))
MAX_SECONDS = 4 * 3600
MAX_UPLOAD = 32 * 1024 * 1024
jobs: dict[str, dict] = {}
tasks: set[asyncio.Task] = set()
final_summary_until = 0.0
knowledge_owner = None
knowledge_until = 0.0

@app.get('/runtime/model-resources')
async def model_resources():
    return {'final_summary_active': time.monotonic() < final_summary_until, 'active_streams':sum(not s.done.is_set() for s in streams.values())}

@app.post('/runtime/final-summary/{action}')
async def final_summary_runtime(action: str, request: Request):
    global final_summary_until
    if request.client.host not in ('192.168.44.1', '127.0.0.1'):
        raise HTTPException(403, 'Native summary adapter only')
    if knowledge_owner and time.monotonic() < knowledge_until:
        raise HTTPException(409, 'Knowledge center owns model resources')
    if action == 'release':
        final_summary_until = 0.0
        return {'released': True}
    if action != 'acquire': raise HTTPException(400, 'Invalid action')
    active = sum(not s.done.is_set() for s in streams.values())
    pending = sum(j.get('status') == 'running' for j in jobs.values())
    if active or pending or inference_slot.locked():
        raise HTTPException(409, 'Speech finalization is still active')
    final_summary_until = time.monotonic() + 3600
    return {'active_streams': active, 'pending_finalizations': pending, 'exclusive': True}

@app.post('/runtime/knowledge/{action}')
async def knowledge_runtime(action: str, request: Request):
    global knowledge_owner, knowledge_until, final_summary_until, fast_ready
    if request.client.host not in ('192.168.44.1', '127.0.0.1'):
        raise HTTPException(403, 'Native knowledge service only')
    data = await request.json()
    owner = data.get('owner', '')
    if not isinstance(owner, str) or len(owner) < 20:
        raise HTTPException(400, 'Invalid resource owner')
    now = time.monotonic()
    if action == 'release':
        if knowledge_owner == owner:
            knowledge_owner = None; knowledge_until = 0.0; final_summary_until = 0.0
        return {'released': True}
    if action != 'acquire': raise HTTPException(400, 'Invalid action')
    if knowledge_owner and now < knowledge_until and knowledge_owner != owner:
        raise HTTPException(409, 'Knowledge center is already active')
    if now < final_summary_until and (not knowledge_owner or now >= knowledge_until):
        raise HTTPException(409, 'Final summary is still running')
    active = sum(not s.done.is_set() for s in streams.values())
    pending = sum(j.get('status') == 'running' for j in jobs.values())
    if active or pending or inference_slot.locked():
        raise HTTPException(409, 'Speech processing must finish first')
    first_entry = knowledge_owner != owner or now >= knowledge_until
    knowledge_owner = owner; knowledge_until = now + 90; final_summary_until = knowledge_until
    if first_entry:
        import gc
        import fast_stream
        from app.utils.speaker_diarizer import close_speaker_diarizer
        for stream in streams.values():
            speaker = getattr(stream, 'speaker', None)
            if speaker is not None:
                speaker.model = speaker.processor = speaker.cache = None
        fast_stream.recognizer = fast_stream.speaker_parts = None
        close_speaker_diarizer()
        fast_ready = False
        gc.collect()
    return {'exclusive': True, 'active_streams': 0}

locks: dict[int, asyncio.Lock] = {}
inference_slot = asyncio.Lock()
streams: dict[int, "LiveStream"] = {}
FAST_ENABLED = os.getenv("MEETINGBOX_FAST_ASR", "0") == "1"
fast_ready = False


@app.on_event("startup")
async def warm_fast_asr():
    global fast_ready
    if FAST_ENABLED:
        from fast_stream import warmup
        await asyncio.to_thread(warmup)
        fast_ready = True



class LiveStream:
    """One continuous upstream connection per meeting, including silent audio."""
    def __init__(self, meeting_id):
        self.meeting_id = meeting_id
        self.socket = None
        self.reader = None
        self.lock = asyncio.Lock()
        self.done = asyncio.Event()
        self.parts = {}
        self.closed = {}
        self.labels = {}
        self.sent = set()
        self.pending = []
        self.start_ms = 0
        self.samples = 0
        self.error = None
        self.last_index = -1
        self.audio_ms = 0
        self.lookahead_ms = 4000
        self.advanced = asyncio.Event()

    async def open(self, context=""):
        from websockets.asyncio.client import connect
        url = ASR_URL.replace("http://", "ws://").replace("https://", "wss://") + "/v1/stream"
        self.socket = await connect(url, additional_headers=headers(), open_timeout=30)
        await self.socket.send(json.dumps({"context": context}))
        ready = json.loads(await asyncio.wait_for(self.socket.recv(), 60))
        if not ready.get("ready") or not ready.get("speaker_diarization"):
            await self.socket.close()
            raise RuntimeError("continuous ASR with speaker diarization is unavailable")
        self.lookahead_ms = round(float(ready["chunk_seconds"]) * 2000)
        self.reader = asyncio.create_task(self.read())

    def accept(self, event):
        index = event.get("utterance")
        if index is None:
            return
        if "delta" in event:
            self.parts[index] = self.parts.get(index, "") + event.get("delta", "")
            if event.get("utterance_end"):
                end = int(event["audio_ms"])
                self.closed[index] = {"start_ms": self.start_ms, "end_ms": end,
                                      "text": self.parts.get(index, "").strip()}
                self.start_ms = end
        if "speaker" in event:
            label = event["speaker"]
            self.labels[index] = label.replace("说话人", "发言人", 1) if isinstance(label, str) else None
        for index in sorted(self.closed):
            if index in self.sent or index not in self.labels:
                continue
            segment = self.closed[index]
            if segment["text"]:
                label = self.labels[index]
                self.pending.append({**segment, "speaker": label,
                                     "utterance": index})
            self.sent.add(index)

    async def read(self):
        try:
            async for message in self.socket:
                event = json.loads(message)
                with (folder(self.meeting_id) / "stream.events.jsonl").open("a", encoding="utf-8") as output:
                    output.write(json.dumps(event, ensure_ascii=False) + "\n")
                if event.get("error"):
                    raise RuntimeError(event.get("code", "stream_failed"))
                self.audio_ms = max(self.audio_ms, int(event.get("audio_ms", 0)))
                self.advanced.set()
                self.accept(event)
                if event.get("done"):
                    break
        except Exception as error:
            self.error = type(error).__name__ + ": " + str(error)[:160]
        finally:
            self.done.set()
            self.advanced.set()

    def take(self):
        if self.error:
            raise HTTPException(502, "continuous ASR failed; raw audio retained: " + self.error)
        result, self.pending = self.pending, []
        return {"segments": result, "diarization": "live", "continuous": True}

    async def finish(self):
        async with self.lock:
            if not self.done.is_set():
                await self.socket.send("end")
                await asyncio.wait_for(self.done.wait(), 300)
            await self.socket.close()
            return self.take()


@app.post("/v1/asr/stream/{meeting_id}")
async def stream_audio(meeting_id: int, request: Request, chunk_index: int):
    if time.monotonic() < final_summary_until:
        raise HTTPException(409, 'Final minutes are using model resources; retry after completion')
    pcm = await request.body()
    if not pcm or len(pcm) % 2 or len(pcm) > 64000:
        raise HTTPException(400, "expected at most 2 seconds of 16 kHz mono PCM16")
    p = folder(meeting_id)
    async with locks.setdefault(meeting_id, asyncio.Lock()):
        if time.monotonic() < final_summary_until:
            raise HTTPException(409, 'Final summary owns model resources')
        if meeting_id not in streams:
            if (p / "stream.pcm").exists():
                raise HTTPException(409, "stream interrupted; finalize retained audio before a new meeting")
            if FAST_ENABLED:
                from fast_stream import FastStream
                if os.getenv('MEETINGBOX_PRIMARY_ASR','')=='fire_red':
                    from fire_red_stream import FireRedStream
                    stream=FireRedStream(meeting_id,p)
                else:stream=FastStream(meeting_id,p)
            else:
                stream = LiveStream(meeting_id)
            try:
                encoded = request.headers.get("X-Meeting-Hotwords", "W10=")
                if len(encoded) > 16384:
                    raise ValueError("oversized hotwords")
                words = json.loads(base64.b64decode(encoded, validate=True).decode("utf-8"))
                if not isinstance(words, list) or any(not isinstance(word, str) for word in words):
                    raise ValueError("invalid hotwords")
            except (ValueError, UnicodeError):
                raise HTTPException(400, "invalid hotwords header")
            await stream.open(",".join(words)[:2048])
            write_json(p / "hotwords.json", words)
            streams[meeting_id] = stream
        stream = streams[meeting_id]
        async with stream.lock:
            if chunk_index <= stream.last_index or stream.done.is_set():
                raise HTTPException(409, "out-of-order or finished stream")
            if stream.samples + len(pcm) // 2 > 3600 * 16000:
                raise HTTPException(413, "continuous session limit is one hour; end this meeting first")
            with (p / "stream.pcm").open("ab") as output:
                output.write(pcm)
            stream.samples += len(pcm) // 2
            stream.last_index = chunk_index
            if FAST_ENABLED:
                await stream.feed(pcm)
                return stream.take()
            for offset in range(0, len(pcm), 20480):
                await asyncio.wait_for(stream.socket.send(pcm[offset:offset + 20480]), 30)
            target = max(0, stream.samples // 16 - stream.lookahead_ms)
            while stream.audio_ms < target and not stream.done.is_set():
                stream.advanced.clear()
                if stream.audio_ms >= target:
                    break
                await asyncio.wait_for(stream.advanced.wait(), 120)
            await asyncio.sleep(0.03)
            return stream.take()


@app.post("/v1/asr/stream/{meeting_id}/end")
async def end_stream(meeting_id: int):
    if meeting_id not in streams:
        return {"segments": [], "continuous": True}
    return await streams[meeting_id].finish()


@app.get('/v1/asr/stream/{meeting_id}/status')
async def stream_status(meeting_id: int):
    stream=streams.get(meeting_id)
    if stream is None:raise HTTPException(404,'unknown stream')
    from transcript_cleanup import display_record
    p=folder(meeting_id);audit=p/'overlap-audit.jsonl'
    return {'segments':[display_record(r) for r in getattr(stream,'records',{}).values()],
            'done':stream.done.is_set(),'audio_ms':stream.samples//16,
            'overlap_audit':[json.loads(line) for line in audit.read_text(encoding='utf8').splitlines()] if audit.exists() else [],
            'errors':(p/'refinement-errors.log').read_text(encoding='utf8').splitlines()[-8:] if (p/'refinement-errors.log').exists() else []}


@app.get('/v1/asr/stream/{meeting_id}/audio/{name}')
async def stream_audio_track(meeting_id: int,name: str):
    if not re.fullmatch(r'overlap-\d+-\d+-[01]\.wav',name):raise HTTPException(400,'invalid track')
    p=folder(meeting_id)/name
    if not p.is_file():raise HTTPException(404,'track unavailable')
    from fastapi.responses import FileResponse
    return FileResponse(p,media_type='audio/wav')


def folder(meeting_id: int) -> Path:
    if meeting_id <= 0:
        raise HTTPException(400, "meeting_id must be positive")
    p = DATA / str(meeting_id)
    p.mkdir(parents=True, exist_ok=True)
    return p


def write_json(path: Path, value):
    temporary = path.with_suffix(".tmp")
    temporary.write_text(json.dumps(value, ensure_ascii=False), encoding="utf-8")
    temporary.replace(path)


def pcm_bytes(blob: bytes) -> bytes:
    try:
        with wave.open(io.BytesIO(blob), "rb") as wav:
            if (wav.getframerate(), wav.getnchannels(), wav.getsampwidth()) != (16000, 1, 2):
                raise ValueError("expected 16 kHz mono PCM16 WAV")
            pcm = wav.readframes(wav.getnframes())
    except (ValueError, wave.Error, EOFError) as error:
        raise HTTPException(400, str(error)) from error
    if not pcm:
        raise HTTPException(400, "empty audio")
    return pcm


def wav_bytes(pcm: bytes) -> bytes:
    stream = io.BytesIO()
    with wave.open(stream, "wb") as wav:
        wav.setnchannels(1)
        wav.setsampwidth(2)
        wav.setframerate(16000)
        wav.writeframes(pcm)
    return stream.getvalue()


def headers():
    key = os.getenv("ASRSERVE_API_KEY")
    return {"Authorization": f"Bearer {key}"} if key else {}


async def recognize(blob: bytes, hotwords: str, diarize: bool):
    async with inference_slot:
        if time.monotonic() < final_summary_until:
            raise HTTPException(409, 'Final summary owns model resources')
        async with httpx.AsyncClient(timeout=1800) as client:
            response = await client.post(
                ASR_URL + "/v1/audio/transcriptions",
                headers=headers(),
                files={"file": ("meeting.wav", blob, "audio/wav")},
                data={"model": "confucius4-r2t2", "language": "zh",
                      "response_format": "verbose_json",
                      "enable_speaker_diarization": str(diarize).lower(),
                      "word_timestamps": "false", "hotwords": hotwords[:2048]},
            )
            response.raise_for_status()
            return response.json()


def normalize_segments(payload: dict) -> list[dict]:
    labels: dict[str, int] = {}
    result = []
    for segment in payload.get("segments", []):
        text = str(segment.get("text", "")).strip()
        if not text:
            continue
        start = max(0, round(float(segment.get("start", 0)) * 1000))
        end = round(float(segment.get("end", 0)) * 1000)
        if end <= start:
            continue
        speaker = segment.get("speaker")
        number = None
        if speaker is not None:
            number = labels.setdefault(str(speaker), len(labels) + 1)
        result.append({"start_ms": start, "end_ms": end, "text": text,
                       "speaker_id": number,
                       "speaker": f"发言人{number}" if number else None})
    if not result and payload.get("text"):
        raise ValueError("AsrServe returned text without valid segment timestamps")
    return result


@app.get("/healthz")
async def health():
    global fast_ready
    if time.monotonic() < final_summary_until:
        return {'ready': False, 'mode': 'knowledge' if knowledge_owner else 'final-summary', 'fast_realtime': False}
    if FAST_ENABLED and not fast_ready:
        from fast_stream import warmup
        await asyncio.to_thread(warmup)
        fast_ready = True
    try:
        async with httpx.AsyncClient(timeout=5) as client:
            response = await client.get(ASR_URL + "/health", headers=headers())
            response.raise_for_status()
            state = response.json()
            primary=None
            if os.getenv('MEETINGBOX_PRIMARY_ASR','')=='fire_red':
                checked=await client.get(os.environ['MEETINGBOX_REFINE_URL']+'/health');checked.raise_for_status();primary=checked.json()
            speech_urls=[os.getenv(k,'').rstrip('/') for k in ('MEETINGBOX_PUNC_URL','MEETINGBOX_OVERLAP_URL') if os.getenv(k)]
            checks=await asyncio.gather(*(client.get(url+'/health',timeout=2) for url in speech_urls),return_exceptions=True)
            speech_ready=all(isinstance(r,httpx.Response) and r.is_success and r.json().get('ready') is True for r in checks)
        return {"ready": (speech_ready and ((fast_ready and (primary is None or primary.get("ready"))) if FAST_ENABLED else state.get("model_loaded") is True)),
                "fast_realtime": FAST_ENABLED and fast_ready, "primary": primary,
                  "audio_enhancement": os.getenv('MEETINGBOX_AUDIO_ENHANCE','off'),
                  "punctuation": {'enabled': bool(os.getenv('MEETINGBOX_PUNC_URL')), 'model': 'FireRedPunc' if os.getenv('MEETINGBOX_PUNC_URL') else None},
                "backend": "asrserve", "upstream": state}
    except (httpx.HTTPError, ValueError):
        return {"ready": False, "backend": "asrserve", "error": "upstream unavailable"}


@app.post("/v1/asr/transcribe-upload")
async def transcribe(audio: UploadFile = File(...), meeting_id: int = Form(...),
                     chunk_start_ms: int = Form(0), hotwords: str = Form("[]"),
                     skip_vad: str = Form("0")):
    if time.monotonic() < final_summary_until:
        raise HTTPException(409, 'Final minutes are using model resources')
    del skip_vad  # AsrServe owns speech activity detection.
    if not 0 <= chunk_start_ms <= MAX_SECONDS * 1000:
        raise HTTPException(400, "chunk_start_ms outside supported recording duration")
    blob = await audio.read(MAX_UPLOAD + 1)
    if len(blob) > MAX_UPLOAD:
        raise HTTPException(413, "audio chunk too large")
    pcm = pcm_bytes(blob)
    # Existing Tauri provider adds exactly 100 ms silence on each end.
    if len(pcm) <= 6400:
        raise HTTPException(400, "audio chunk too short")
    pcm = pcm[3200:-3200]
    if chunk_start_ms / 1000 + len(pcm) / 32000 > MAX_SECONDS:
        raise HTTPException(413, "meeting exceeds four hours")
    try:
        words = json.loads(hotwords)
        if not isinstance(words, list) or any(not isinstance(w, str) for w in words):
            raise ValueError()
    except (ValueError, TypeError):
        raise HTTPException(400, "hotwords must be a JSON string array")
    p = folder(meeting_id)
    async with locks.setdefault(meeting_id, asyncio.Lock()):
        if (p / "finalize.json").exists():
            raise HTTPException(409, "meeting has already been finalized")
        (p / f"{chunk_start_ms}.wav").write_bytes(wav_bytes(pcm))
        write_json(p / "hotwords.json", words)
        try:
            raw = await recognize(wav_bytes(pcm), ",".join(words), False)
        except httpx.HTTPError as error:
            raise HTTPException(502, "AsrServe transcription failed; audio retained for retry") from error
        write_json(p / f"{chunk_start_ms}.raw.json", raw)
    # Independent chunk labels cannot be trusted across a whole meeting.
    segments = [{"text": str(s.get("text", "")), "speaker": None}
                for s in raw.get("segments", []) if s.get("text")]
    if not segments and raw.get("text"):
        segments = [{"text": raw["text"], "speaker": None}]
    return {"segments": segments, "diarization": "pending-full-meeting"}


def assemble_audio(p: Path) -> bytes:
    if (p / "stream.pcm").exists():
        return wav_bytes((p / "stream.pcm").read_bytes())
    chunks = sorted(p.glob("*.wav"), key=lambda f: int(f.stem))
    if not chunks:
        raise ValueError("No recorded chunks are available")
    pcm = bytearray()
    for chunk in chunks:
        start = int(chunk.stem) * 32
        content = pcm_bytes(chunk.read_bytes())
        end = start + len(content)
        if end > MAX_SECONDS * 32000:
            raise ValueError("Recording exceeds four hours")
        if end > len(pcm):
            pcm.extend(b"\0" * (end - len(pcm)))
        pcm[start:end] = content
    return wav_bytes(pcm)


def correction_is_safe(original: str, corrected: str) -> bool:
    # Reject changed numeric facts and dropped/introduced negation. Uncertain
    # entity fixes remain suggestions for human review, never auto-published.
    if re.findall(r"\d+(?:\.\d+)?", original) != re.findall(r"\d+(?:\.\d+)?", corrected):
        return False
    if any(original.count(w) != corrected.count(w) for w in ("不", "没", "未", "无")):
        return False
    return bool(corrected.strip()) and len(corrected) <= len(original) * 1.3 + 20


def final_answer(content: str) -> str:
    # Never publish reasoning, including a response truncated inside a think block.
    content = re.sub(r"(?is)<think(?:ing)?>.*?</think(?:ing)?>", "", content)
    content = re.sub(r"(?is)<think(?:ing)?>.*$", "", content).strip()
    if not content:
        raise ValueError("model returned no final answer")
    return content


async def ollama_final(messages: list[dict], options: dict, json_output: bool = False) -> tuple[str, dict]:
    async with httpx.AsyncClient(timeout=1800) as client:
        response = await client.post(LLM_URL.removesuffix("/v1") + "/api/chat", json={
            # Use the explicit instruct model; publish final content only.
            "model": LLM_MODEL, "messages": messages, "think": False, "stream": False,
            "options": options, **({"format": "json"} if json_output else {}),
        })
        response.raise_for_status()
        payload = response.json()
        if payload.get("done_reason") == "length":
            raise ValueError("model output truncated; report not published")
        content = final_answer(payload["message"]["content"])
        if re.search(r"(?is)首先.{0,40}(?:用户要求|我需要)|For the main title|\[AI-Generated Title\]", content):
            raise ValueError("instruction commentary is not a meeting report")
        return content, payload


async def chat(system: str, text: str, json_output: bool = True) -> str:
    content, _ = await ollama_final(
        [{"role": "system", "content": system}, {"role": "user", "content": text}],
        {"temperature": 0, "num_predict": 8192, "num_thread": 8}, json_output)
    return content


@app.post("/v1/chat/completions")
async def final_chat_completion(request: Request):
    body = await request.json()
    if body.get("stream"):
        raise HTTPException(400, "streaming is not supported for final meeting reports")
    messages = body.get("messages")
    if not isinstance(messages, list) or not messages:
        raise HTTPException(400, "messages required")
    messages = [dict(message) for message in messages]
    guard = ("\n纪要必须与原文信息量相称，不重复扩写。明确区分提出的想法、测试话语、疑问与已同意的决定。"
             "疑似误识别或词义异常的内容写待核对，不解释成原文没有的业务问题。"
             "输出正式会议纪要，不显示逐句时间戳，不附逐字原文。标题概括实际主题。")
    if messages[0].get('role') == 'system':
        messages[0]['content'] = str(messages[0].get('content', '')) + guard
    else:
        messages.insert(0, {'role':'system','content':guard})
    options = {"temperature": body.get("temperature", 0), "num_predict": max(4096, min(body.get("max_tokens") or 4096, 16384)), "num_thread": 8}
    if body.get("top_p") is not None:
        options["top_p"] = body["top_p"]
    try:
        content, payload = await ollama_final(messages, options, body.get("response_format", {}).get("type") == "json_object")
        return {"id": "meetingbox-" + uuid.uuid4().hex, "object": "chat.completion", "model": payload.get("model", LLM_MODEL),
                "choices": [{"index": 0, "message": {"role": "assistant", "content": content}, "finish_reason": "stop"}],
                "usage": {"prompt_tokens": payload.get("prompt_eval_count", 0), "completion_tokens": payload.get("eval_count", 0)}}
    except (httpx.HTTPError, ValueError, KeyError, TypeError) as error:
        raise HTTPException(502, "final summary unavailable; existing report retained") from error


async def finish(meeting_id: int, job_id: str):
    p = folder(meeting_id)
    try:
        async with locks.setdefault(meeting_id, asyncio.Lock()):
            if meeting_id in streams:
                await streams[meeting_id].finish()
            blob = await asyncio.to_thread(assemble_audio, p)
            words = json.loads((p / "hotwords.json").read_text(encoding="utf-8")) if (p / "hotwords.json").exists() else []
            raw = await recognize(blob, ",".join(words), True)
            write_json(p / "meeting.raw.json", raw)
            segments = normalize_segments(raw)
            result = {"finalized": True, "pending": False, "status": "completed",
                      "job_id": job_id, "segments": segments,
                      "transcription_mode": "full-meeting",
                      "correction_status": "not-requested"}
            # Preserve acoustic transcript as the published transcript. Suggestions
            # are separately reviewable so a language model cannot silently rewrite it.
            if os.getenv("MEETINGBOX_CORRECTION_ENABLED", "0") == "1":
                original = json.dumps({"segments": [{"index": i, "text": segment["text"]}
                                       for i, segment in enumerate(segments)]}, ensure_ascii=False)
                try:
                    content = await chat(
                        '校对以下转写，只修正明显错字和标点。保留数字、单位、姓名、否定词；'
                        '不增添事实。不确定处保持原文。返回JSON对象，segments字段为index和text对象的数组；index原样保留。/no_think', original)
                    edits = json.loads(content)["segments"]
                    if not isinstance(edits, list) or len(edits) != len(segments):
                        raise ValueError("Correction changed segment count")
                    proposed = []
                    for index, (a, b) in enumerate(zip(segments, edits)):
                        if not isinstance(b, dict) or b.get("index") != index:
                            raise ValueError("Correction changed segment index")
                        if not isinstance(b.get("text"), str) or not correction_is_safe(a["text"], b["text"]):
                            raise ValueError("Correction changed protected facts")
                        proposed.append({**a, "text": b["text"]})
                    write_json(p / "meeting.correction-suggestions.json", proposed)
                    result["correction_status"] = "awaiting-review"
                except (httpx.HTTPError, ValueError, KeyError, TypeError):
                    result["correction_status"] = "failed-original-retained"
            write_json(p / "finalize.json", result)
            jobs[job_id] = result
    except Exception as error:
        jobs[job_id] = {"finalized": False, "pending": False, "status": "failed",
                        "job_id": job_id, "error": str(error)[:300], "segments": []}


class FinalizeRequest(BaseModel):
    meeting_id: int = Field(gt=0)
    segments: list = Field(default_factory=list)


@app.post("/v1/asr/finalize-meeting")
async def finalize(request: FinalizeRequest):
    p = folder(request.meeting_id)
    if (p / "finalize.json").exists():
        return json.loads((p / "finalize.json").read_text(encoding="utf-8"))
    for state in jobs.values():
        if state.get("meeting_id") == request.meeting_id and state.get("pending"):
            return state
    job_id = uuid.uuid4().hex
    jobs[job_id] = {"meeting_id": request.meeting_id, "job_id": job_id,
                    "pending": True, "finalized": False, "status": "running", "segments": []}
    task = asyncio.create_task(finish(request.meeting_id, job_id))
    tasks.add(task)
    task.add_done_callback(tasks.discard)
    return jobs[job_id]


@app.get("/v1/asr/finalize-meeting/{job_id}")
async def job_status(job_id: str):
    if job_id not in jobs:
        raise HTTPException(404, "unknown job; retry finalization after service restart")
    return jobs[job_id]


@app.get("/v1/asr/meetings/{meeting_id}/corrections")
async def corrections(meeting_id: int):
    p = folder(meeting_id)
    path = p / "meeting.correction-suggestions.json"
    return {"status": "awaiting-review" if path.exists() else "unavailable",
            "segments": json.loads(path.read_text(encoding="utf-8")) if path.exists() else []}


class SummaryRequest(BaseModel):
    meeting_id: int = 0
    template: str = "weekly"
    segments: list[dict]
    incremental: bool = True
    previous_context: dict = Field(default_factory=dict)
    interval_index: int = Field(default=0,ge=0)


class CorrectionRequest(BaseModel):
    text: str = Field(min_length=1, max_length=6000)
    context: str = Field(default="", max_length=6000)


@app.post("/v1/asr/correct-text")
async def correct_text(request: CorrectionRequest):
    try:
        content = await chat(
            '校对转写，只修改明显错字和标点。数字、单位、姓名、日期、否定和承诺保持原样；'
            '无法确定时保留原文。上下文只作参考，不加入原文。返回JSON对象，text字段为校对建议。/no_think',
            json.dumps(request.model_dump(), ensure_ascii=False))
        corrected = json.loads(content)["text"]
        if not isinstance(corrected, str) or not correction_is_safe(request.text, corrected):
            raise ValueError("protected facts changed")
        return {"original": request.text, "suggestion": corrected, "status": "awaiting-review"}
    except (httpx.HTTPError, ValueError, KeyError, TypeError) as error:
        raise HTTPException(502, "correction unavailable or protected facts changed; original retained") from error


@app.post("/v1/summary/generate")
async def summary(request: SummaryRequest):
    def timed(s):
        time=max(0,int(s.get('start_seconds') or 0))
        return f"[{time//60:02}:{time%60:02}] {s.get('speaker') or '发言人'}：{s.get('text', '')}"
    text = "\n".join(timed(s) for s in request.segments)
    if not text.strip() or len(text) > 24000:
        raise HTTPException(400, "empty or oversized transcript")
    try:
        # Give an overloaded real-time refinement queue priority over optional summaries.
        for _ in range(30):
            if not any(getattr(s,'refine_queue',None) and s.refine_queue.qsize()>2 and not s.done.is_set() for s in streams.values()):break
            await asyncio.sleep(.5)
        previous=request.previous_context
        if len(json.dumps(previous,ensure_ascii=False))>12000:raise ValueError('incremental memory oversized')
        async with httpx.AsyncClient(timeout=230) as client:
            response = await client.post(LLM_URL.removesuffix('/v1')+'/api/incremental',json=request.model_dump())
            response.raise_for_status()
            structure = validate_state(response.json()['structure'],previous,text)
        if not isinstance(structure, dict) or not isinstance(structure.get("summary"), str):
            raise ValueError("invalid summary")
        if not isinstance(structure.get('context_summary'),str):raise ValueError('missing cumulative context')
        for key in ("decisions", "agenda", "todos", "risks","discussion","open_questions","evidence"):
            structure.setdefault(key,[])
            if not isinstance(structure.get(key), list) or any(not isinstance(item, str) for item in structure[key]):
                raise ValueError("invalid summary fields")
        structure['summary']=structure['summary'][:120]
        structure['context_summary']=structure['context_summary'][:240]
        original_numbers = set(re.findall(r"\d+(?:\.\d+)?", text+'\n'+json.dumps(previous,ensure_ascii=False)))
        generated_text = structure["summary"] + "\n" + "\n".join(
            item for key in ("decisions", "agenda", "todos", "risks") for item in structure[key])
        if not set(re.findall(r"\d+(?:\.\d+)?", generated_text)).issubset(original_numbers):
            raise ValueError("summary introduced numeric facts")
        if request.meeting_id>0:
            write_json(folder(request.meeting_id)/f'incremental-summary-{request.interval_index}.json',{'structure':structure,'source':request.segments})
        return {"structure": structure}
    except (httpx.HTTPError, ValueError, KeyError, TypeError) as error:
        raise HTTPException(502, "local summary model unavailable or invalid output") from error
