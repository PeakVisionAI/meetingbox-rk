"""Wake timing regressions through the real vendored VoiceSession/VoiceFront.

Only the KWS and VAD models are scripted: every 200 ms chunk is marked as
silence, speech, or a wake-word hit. Session state, deadlines, utterance
collection, recognition scheduling and event serialization stay real, and the
timing comes from the production ``server.settings``. Run with::

    python -m unittest discover -s tests/voice-front -v
"""
from __future__ import annotations

import asyncio
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import unittest
from unittest.mock import patch

import numpy as np


ROOT = Path(os.environ.get("MEETINGBOX_VOICE_TEST_ROOT", Path(__file__).resolve().parents[2]))
SERVICE_PATH = ROOT / "services" / "voice-front" / "server.py"
spec = importlib.util.spec_from_file_location("meetingbox_voice_front_server_policy_tests", SERVICE_PATH)
server = importlib.util.module_from_spec(spec)
spec.loader.exec_module(server)

from voice_front.kws import KwsHit  # noqa: E402 - vendor path is added by server.py
from voice_front.service import VoiceFront  # noqa: E402
from voice_front.session import VoiceSession  # noqa: E402
from voice_front.settings import VoiceFrontSettings  # noqa: E402
from voice_front.vad import VadSegment  # noqa: E402

CHUNK = 3200  # 200 ms, the native feed size
SILENCE, SPEECH, WAKE = 0.0, 0.5, 0.75  # exact in float32


def chunk(level):
    return np.full(CHUNK, level, dtype=np.float32)


class FakeKwsStream:
    def set_active_words(self, words):
        self.active = frozenset(words)

    def feed(self, pcm):
        if pcm.size and float(pcm[0]) == WAKE and "小峰小峰" in self.active:
            return [KwsHit("小峰小峰", "wake", 0.0)]
        return []


class FakeKwsEngine:
    wake_words = ["小峰小峰"]
    barge_words = ["取消"]

    def create_stream(self, wake_words=None):
        return FakeKwsStream()


class FakeVad:
    """Speech ends after the configured silence; short speech yields no segment."""

    def __init__(self, end_silence_s, min_speech_s):
        self.end_chunks = round(end_silence_s / 0.2)
        self.min_samples = int(min_speech_s * 16000)
        self.fed = 0
        self.speech = []
        self.speech_start = 0
        self.silent = 0
        self.in_speech = False

    def feed(self, pcm):
        segments = []
        for offset in range(0, pcm.size, CHUNK):
            piece = pcm[offset:offset + CHUNK]
            if float(piece[0]) == SPEECH:
                if not self.speech:
                    self.speech_start = self.fed
                self.speech.append(piece)
                self.silent = 0
                self.in_speech = True
            elif self.in_speech:
                self.silent += 1
                if self.silent >= self.end_chunks:
                    wav = np.concatenate(self.speech)
                    if wav.size >= self.min_samples:
                        segments.append(VadSegment(self.speech_start / 16000, wav))
                    self.speech, self.silent, self.in_speech = [], 0, False
            self.fed += piece.size
        return segments

    def flush(self):
        return []


def vad_factory(self, end_silence_s):
    return FakeVad(end_silence_s, self.settings.min_speech_s)


class SessionHarness:
    def __init__(self, settings):
        self.session = VoiceSession("desktop", settings, FakeKwsEngine(),
                                    vad_factory=lambda silence: FakeVad(silence, settings.min_speech_s))
        self.timeline = []

    def feed(self, level, count=1, **flags):
        for _ in range(count):
            events, utterances = self.session.feed(chunk(level), mode="wake", **flags)
            self.timeline.extend((round(self.session.t, 1), event) for event in events)
            if utterances:
                self.timeline.append((round(self.session.t, 1), {"type": "utterances", "count": len(utterances)}))

    def kinds(self):
        return [(t, event["type"], event.get("reason")) for t, event in self.timeline]


class VendorProvenanceTests(unittest.TestCase):
    def test_vendored_sources_match_recorded_hashes(self):
        vendor = SERVICE_PATH.parent / "vendor"
        provenance = json.loads((vendor / "PROVENANCE.json").read_text(encoding="utf-8"))
        for name, digest in provenance["files"].items():
            with self.subTest(file=name):
                self.assertEqual(hashlib.sha256((vendor / "voice_front" / name).read_bytes()).hexdigest(), digest)

    def test_wrapper_overrides_only_timing_and_limits(self):
        baseline = VoiceFrontSettings.from_env()
        self.assertEqual((baseline.wake_timeout_s, baseline.followup_window_s), (5.0, 10.0),
                         "vendor defaults changed; re-check the native wake policy")
        self.assertEqual((server.settings.wake_timeout_s, server.settings.followup_window_s), (20.0, 20.0))
        changed = {name for name in baseline.__dataclass_fields__
                   if getattr(baseline, name) != getattr(server.settings, name)}
        self.assertEqual(changed, {"wake_timeout_s", "followup_window_s", "max_utterance_s",
                                   "max_pending_audio_s", "max_sessions"})


class SessionTimingTests(unittest.TestCase):
    def assert_single_timeout(self, harness, deadline):
        drops = [k for k in harness.kinds() if k[1] == "dropped"]
        self.assertEqual([k[2] for k in drops], ["wake_timeout"])
        self.assertTrue(deadline <= drops[0][0] <= deadline + 0.2, drops)

    def test_vendor_default_cancels_a_six_second_pause(self):
        # Documents the regression: 5 s vendor timeout vs 20 s native window.
        harness = SessionHarness(VoiceFrontSettings.from_env())
        harness.feed(WAKE)
        harness.feed(SILENCE, 30)
        self.assertIn((5.2, "dropped", "wake_timeout"), harness.kinds())

    def test_wrapper_keeps_listening_through_a_six_second_pause(self):
        harness = SessionHarness(server.settings)
        harness.feed(WAKE)
        harness.feed(SILENCE, 30)
        self.assertEqual(harness.kinds(), [(0.2, "wake", None)])
        self.assertEqual(harness.session.state, "listening")
        harness.feed(SPEECH, 4)
        harness.feed(SILENCE, 10)
        self.assertEqual(harness.kinds()[1:], [(9.0, "speech_end", None), (9.0, "utterances", None)])

    def test_wrapper_times_out_at_twenty_seconds_without_speech(self):
        harness = SessionHarness(server.settings)
        harness.feed(WAKE)
        harness.feed(SILENCE, 98)
        self.assertEqual(harness.kinds(), [(0.2, "wake", None)])
        harness.feed(SILENCE, 3)  # deadline is 20.2 s; 0.2 s float steps may land one chunk late
        self.assert_single_timeout(harness, 20.2)
        self.assertEqual(harness.session.state, "idle")

    def test_too_short_renews_the_service_deadline_and_stays_listening(self):
        # Native policy holds the absolute deadline; the service may outlast it.
        harness = SessionHarness(server.settings)
        harness.feed(WAKE)
        harness.feed(SILENCE, 49)
        harness.feed(SPEECH)  # 200 ms is below min_speech_s
        harness.feed(SILENCE, 12)
        self.assertEqual(harness.kinds(), [(0.2, "wake", None), (12.4, "dropped", "too_short")])
        self.assertEqual(harness.session.state, "listening")
        harness.feed(SILENCE, 60)  # past the native 20 s deadline, still before the renewed one
        self.assertEqual(harness.kinds()[-1], (12.4, "dropped", "too_short"))
        self.assertEqual(harness.session.state, "listening")

    def test_followup_window_is_twenty_seconds(self):
        harness = SessionHarness(server.settings)
        harness.feed(SILENCE, 1, expect_followup=True)
        self.assertEqual(harness.session.state, "listening")
        harness.feed(SILENCE, 98, expect_followup=True)
        self.assertEqual(harness.kinds(), [])
        harness.feed(SILENCE, 2, expect_followup=True)
        self.assert_single_timeout(harness, 20.0)


class FrontRecognitionTests(unittest.IsolatedAsyncioTestCase):
    async def run_front(self, texts, script):
        queue = list(texts)

        async def recognize(samples):
            return queue.pop(0)

        front = VoiceFront(server.settings, recognize=recognize)
        front._engine = FakeKwsEngine()
        events, seq = [], 0
        with patch.object(VoiceSession, "_default_vad_factory", vad_factory):
            for level, count in script:
                for _ in range(count):
                    response = await front.feed(session_id="desktop", seq=seq, pcm=chunk(level), mode="wake")
                    events.extend(response["events"])
                    seq += 1
                    await front.drain()
        return events, front._entries["desktop"].session

    async def test_empty_recognition_drops_but_keeps_listening(self):
        events, session = await self.run_front(["  "], [(WAKE, 1), (SILENCE, 30), (SPEECH, 4), (SILENCE, 12)])
        self.assertEqual([(e["type"], e.get("reason")) for e in events],
                         [("wake", None), ("speech_end", None), ("dropped", "empty")])
        self.assertEqual(session.state, "listening")

    async def test_wake_word_only_recognition_is_not_an_utterance(self):
        events, session = await self.run_front(["小峰小峰。"], [(WAKE, 1), (SPEECH, 4), (SILENCE, 12)])
        self.assertEqual([e["type"] for e in events], ["wake", "speech_end"])
        self.assertEqual(session.state, "listening")

    async def test_command_after_six_seconds_reaches_native_as_final_utterance(self):
        events, session = await self.run_front(["小峰小峰，开始会议。"],
                                               [(WAKE, 1), (SILENCE, 30), (SPEECH, 4), (SILENCE, 12)])
        final = [e for e in events if e["type"] == "utterance"]
        self.assertEqual([(e["trigger"], e["text"]) for e in final], [("wake", "开始会议。")])
        self.assertNotIn("dropped", [e["type"] for e in events])
        self.assertEqual(session.state, "idle")


if __name__ == "__main__":
    unittest.main()
