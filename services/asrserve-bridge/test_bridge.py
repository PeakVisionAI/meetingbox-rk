import asyncio
import importlib.util
import json
import sqlite3
from pathlib import Path

import sys
sys.path.insert(0,str(Path(__file__).parents[1]/'qwen-openvino'))
import pytest
from fastapi.testclient import TestClient

spec = importlib.util.spec_from_file_location("bridge", Path(__file__).with_name("bridge.py"))
bridge = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bridge)


@pytest.fixture
def client(tmp_path, monkeypatch):
    monkeypatch.setattr(bridge, "DATA", tmp_path)
    bridge.jobs.clear()
    bridge.streams.clear()
    bridge.locks.clear()
    bridge.inference_slot = asyncio.Lock()
    with TestClient(bridge.app) as client:
        yield client


def test_chunk_upload_preserves_raw_audio_and_defers_speaker_identity(client, monkeypatch):
    async def recognize(blob, hotwords, diarize):
        assert hotwords == "峰瞰,HYOS"
        assert diarize is False
        assert len(bridge.pcm_bytes(blob)) == 32000
        return {"text": "测试", "segments": [{"text": "测试", "speaker": "Speaker1"}]}
    monkeypatch.setattr(bridge, "recognize", recognize)
    padded = bridge.wav_bytes(b"\0" * 3200 + b"\x01\0" * 16000 + b"\0" * 3200)
    result = client.post("/v1/asr/transcribe-upload", files={"audio": ("chunk.wav", padded)},
                         data={"meeting_id": 7, "chunk_start_ms": 2000,
                               "hotwords": '["峰瞰","HYOS"]'})
    assert result.status_code == 200
    assert result.json()["segments"][0]["speaker"] is None
    assert (bridge.DATA / "7" / "2000.raw.json").exists()


def test_assemble_keeps_recording_offsets_and_gaps(tmp_path):
    (tmp_path / "0.wav").write_bytes(bridge.wav_bytes(b"\x01\0" * 16000))
    (tmp_path / "2000.wav").write_bytes(bridge.wav_bytes(b"\x02\0" * 16000))
    pcm = bridge.pcm_bytes(bridge.assemble_audio(tmp_path))
    assert len(pcm) == 96000
    assert pcm[32000:64000] == b"\0" * 32000
    assert pcm[64000:] == b"\x02\0" * 16000


def test_finalization_clusters_the_whole_recording(client, monkeypatch):
    p = bridge.folder(8)
    (p / "0.wav").write_bytes(bridge.wav_bytes(b"\0" * 32000))
    bridge.write_json(p / "hotwords.json", [])
    async def recognize(blob, hotwords, diarize):
        assert diarize
        return {"segments": [{"start": 0, "end": .4, "text": "甲", "speaker": "a"},
                              {"start": .4, "end": .7, "text": "乙", "speaker": "b"},
                              {"start": .7, "end": 1, "text": "甲二", "speaker": "a"}]}
    monkeypatch.setattr(bridge, "recognize", recognize)
    monkeypatch.setenv("MEETINGBOX_CORRECTION_ENABLED", "0")
    asyncio.run(bridge.finish(8, "testjob"))
    result = bridge.jobs["testjob"]
    assert result["finalized"]
    assert [s["speaker_id"] for s in result["segments"]] == [1, 2, 1]
    assert client.post("/v1/asr/finalize-meeting", json={"meeting_id": 8}).json()["finalized"]
    assert (p / "meeting.raw.json").exists()


def test_missing_upstream_never_reports_ready(client, monkeypatch):
    monkeypatch.setattr(bridge, "ASR_URL", "http://127.0.0.1:1")
    assert client.get("/healthz").json()["ready"] is False


def test_full_correction_keeps_acoustic_attribution(client, monkeypatch):
    p = bridge.folder(9)
    (p / "0.wav").write_bytes(bridge.wav_bytes(b"\0" * 32000))
    bridge.write_json(p / "hotwords.json", [])
    async def recognize(*args):
        return {"segments":[{"start":0.2,"end":0.8,"text":"预算100万元","speaker":"a"}]}
    async def chat(*args):
        return '{"segments":[{"index":0,"text":"预算100万元。"}]}'
    monkeypatch.setattr(bridge,"recognize",recognize)
    monkeypatch.setattr(bridge,"chat",chat)
    monkeypatch.setenv("MEETINGBOX_CORRECTION_ENABLED","1")
    asyncio.run(bridge.finish(9,"correctedjob"))
    result=bridge.jobs["correctedjob"]
    proposed=json.loads((p / "meeting.correction-suggestions.json").read_text(encoding="utf-8"))
    assert result["correction_status"]=="awaiting-review"
    assert result["segments"][0]["text"]=="预算100万元"
    assert proposed[0]["text"]=="预算100万元。"
    assert proposed[0]["start_ms"]==200 and proposed[0]["end_ms"]==800
    assert proposed[0]["speaker_id"]==1


def test_continuous_stream_waits_for_pause_and_stable_speaker():
    stream=bridge.LiveStream(123)
    stream.accept({"utterance":0,"delta":"预算","audio_ms":500,"utterance_end":False})
    stream.accept({"utterance":0,"delta":"100万元。","audio_ms":1500,"utterance_end":True})
    assert stream.take()["segments"]==[]
    stream.accept({"utterance":0,"speaker":"说话人1"})
    result=stream.take()["segments"]
    assert result==[{"start_ms":0,"end_ms":1500,"text":"预算100万元。","speaker":"发言人1","utterance":0}]
    stream.accept({"utterance":1,"speaker":"说话人2"})
    stream.accept({"utterance":1,"delta":"我不同意。","audio_ms":3400,"utterance_end":True})
    assert stream.take()["segments"][0]["speaker"]=="发言人2"
    assert stream.take()["segments"]==[]


def test_stream_audio_preserves_silence_and_short_tail(tmp_path):
    pcm=b'\x01\0'*100 + b'\0'*32000 + b'\x02\0'*17
    (tmp_path/'stream.pcm').write_bytes(pcm)
    assert bridge.pcm_bytes(bridge.assemble_audio(tmp_path))==pcm


def test_stream_transport_keeps_hotwords_pcm_and_tail(client, monkeypatch):
    import base64
    frames=[]
    contexts=[]
    class Socket:
        async def send(self, data): frames.append(data)
    async def open_stream(self, context=""):
        contexts.append(context)
        self.socket=Socket()
    monkeypatch.setattr(bridge.LiveStream,"open",open_stream)
    header=base64.b64encode(json.dumps(["峰瞰","HYOS"],ensure_ascii=False).encode()).decode()
    pcm=b'\0'*64000
    response=client.post('/v1/asr/stream/123?chunk_index=0',content=pcm,headers={"X-Meeting-Hotwords":header})
    assert response.status_code==200
    assert contexts==["峰瞰,HYOS"]
    assert all(len(frame)<=32000 for frame in frames)
    assert b''.join(frames)==pcm
    assert client.post('/v1/asr/stream/123?chunk_index=1',content=b'\x01\0').status_code==200
    assert (bridge.DATA/'123'/'stream.pcm').read_bytes()==pcm+b'\x01\0'
    assert client.post('/v1/asr/stream/123?chunk_index=1',content=b'\x01\0').status_code==409


def test_changed_numeric_fact_is_rejected(client, monkeypatch):
    async def chat(*args, **kwargs):
        return '{"text":"预算是200万元。"}'
    monkeypatch.setattr(bridge, "chat", chat)
    assert client.post("/v1/asr/correct-text", json={"text": "预算是100万元。"}).status_code == 502
    assert not bridge.correction_is_safe("我们不交付", "我们交付")


def test_correction_is_suggestion_only(client, monkeypatch):
    async def chat(*args, **kwargs):
        return '{"text":"预算是100万元。"}'
    monkeypatch.setattr(bridge, "chat", chat)
    result = client.post("/v1/asr/correct-text", json={"text": "预算是100万元"}).json()
    assert result["original"] == "预算是100万元"
    assert result["status"] == "awaiting-review"


def test_invalid_upload_is_rejected(client):
    result = client.post("/v1/asr/transcribe-upload", files={"audio": ("bad.wav", b"bad")},
                         data={"meeting_id": 1})
    assert result.status_code == 400
    assert client.post("/v1/asr/finalize-meeting", json={"meeting_id": -1}).status_code == 422


def test_summary_rejects_corrupted_amount(client, monkeypatch):
    async def chat(*args, **kwargs):
        return '{"summary":"预算100万元","decisions":["预算10:00万元"],"agenda":[],"todos":[],"risks":[]}'
    monkeypatch.setattr(bridge, "chat", chat)
    assert client.post("/v1/summary/generate", json={"segments":[{"text":"预算100万元"}]}).status_code == 502


def test_correction_history_migration_preserves_original():
    database = sqlite3.connect(":memory:")
    database.execute("CREATE TABLE transcripts(id TEXT PRIMARY KEY)")
    migration = Path(__file__).parents[2] / "frontend/src-tauri/migrations/20261014000001_transcript_correction_history.sql"
    database.executescript(migration.read_text())
    database.execute("INSERT INTO transcripts VALUES ('t1')")
    database.execute("INSERT INTO transcript_correction_history VALUES ('r1','t1','m1','原文','校正版',NULL,NULL,'now')")
    assert database.execute("SELECT original_text, corrected_text FROM transcript_correction_history").fetchone() == ("原文", "校正版")


def test_final_answer_drops_reasoning_and_rejects_unclosed():
    assert bridge.final_answer("<think>内部分析</think>\n# 正式纪要") == "# 正式纪要"
    with pytest.raises(ValueError):
        bridge.final_answer("<think>尚未完成的思考")


@pytest.mark.asyncio
async def test_summary_gateway_uses_final_content(monkeypatch):
    seen = {}
    class Response:
        def raise_for_status(self): pass
        def json(self): return {"message": {"content": "# 正式纪要", "thinking": "内部分析"}, "done_reason": "stop"}
    class Client:
        def __init__(self, **kwargs): pass
        async def __aenter__(self): return self
        async def __aexit__(self, *args): pass
        async def post(self, url, json):
            seen.update(url=url, body=json)
            return Response()
    monkeypatch.setattr(bridge.httpx, "AsyncClient", Client)
    result, _ = await bridge.ollama_final([{"role": "user", "content": "生成纪要"}], {"temperature": 0})
    assert result == "# 正式纪要"
    assert seen["url"].endswith("/api/chat")
    assert seen["body"]["think"] is False
    assert seen["body"]["stream"] is False
