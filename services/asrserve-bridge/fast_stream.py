"""Low-latency text with independently scheduled Nemotron speaker attribution."""
import asyncio
import json
import os
import time
import urllib.request
import base64
from pathlib import Path
import numpy as np

recognizer = None
speaker_parts = None


def warmup():
    global recognizer, speaker_parts
    import torch
    from app.services.realtime.speakers import SpeakerStream
    torch.set_num_threads(4)
    if os.getenv('MEETINGBOX_PRIMARY_ASR','')!='fire_red':
        import sherpa_onnx
        root = Path(os.getenv('MEETINGBOX_FAST_MODEL', '/fast-model'))
        recognizer = sherpa_onnx.OnlineRecognizer.from_zipformer2_ctc(
            tokens=str(root / 'tokens.txt'), model=str(root / 'model.int8.onnx'),
            num_threads=2, provider='cpu', decoding_method='greedy_search',
            enable_endpoint_detection=True, rule1_min_trailing_silence=1.2,
            # Preserve a little more linguistic context for the final Qwen pass;
            # draft tokens continue streaming immediately. 19.3s stays below 20s API cap.
            rule2_min_trailing_silence=1.2, rule3_min_utterance_length=19)
    speaker_parts = SpeakerStream()
    if speaker_parts.model is None:
        raise RuntimeError('speaker model failed to load')


class FastStream:
    def __init__(self, meeting_id, folder, speaker=None):
        if speaker is None:
            from app.services.realtime.speakers import SpeakerStream
            speaker = SpeakerStream()
        self.meeting_id, self.folder = meeting_id, folder
        self.lock, self.done = asyncio.Lock(), asyncio.Event()
        self.samples, self.last_index = 0, -1
        self.stream = recognizer.create_stream() if recognizer is not None else None
        self.speaker = speaker
        self.wake = asyncio.Event()
        self.speaker_task = asyncio.create_task(self.run_speakers())
        self.pending, self.records = {}, {}
        self.index, self.start_ms, self.last_text = 0, 0, ''
        self.token_times_ms = []
        self.generation = 0
        self.context = ''
        self.refine_url = os.getenv('MEETINGBOX_REFINE_URL', '').rstrip('/')
        self.refine_queue = asyncio.Queue(maxsize=4)
        self.refine_task = asyncio.create_task(self.run_refinement()) if self.refine_url else None

    async def open(self, context=''):
        self.context = context[:2048]

    def publish(self, text, final, end_ms=None):
        text = text.strip()
        if not text:
            return
        key = self.index * 100
        previous = self.records.get(key, {})
        record = {'utterance': key, 'start_ms': self.start_ms,
                  'end_ms': end_ms if end_ms is not None else self.samples // 16,
                  'text': text, 'speaker': previous.get('speaker'), 'is_partial': not final,
                  'text_ready_audio_ms': self.samples // 16, 'generation': self.generation,
                  'token_times_ms': self.token_times_ms, 'emitted_wall_time': time.time()}
        if self.token_times_ms:
            record['speech_start_ms'] = max(0, self.token_times_ms[0] - 100)
            record['speech_end_ms'] = min(record['end_ms'], self.token_times_ms[-1] + 200)
        self.generation += 1
        self.records[key] = record
        self.pending[key] = dict(record)
        with (self.folder / 'stream.events.jsonl').open('a', encoding='utf8') as out:
            out.write(json.dumps(record, ensure_ascii=False) + '\n')
        self.last_text = text

    def decode(self, pcm, end=False):
        audio = np.frombuffer(pcm, dtype='<i2').astype(np.float32) / 32768
        self.stream.accept_waveform(16000, audio)
        if end:
            self.stream.input_finished()
        while recognizer.is_ready(self.stream):
            recognizer.decode_stream(self.stream)
        result = json.loads(recognizer.get_result_as_json_string(self.stream))
        return result, recognizer.is_endpoint(self.stream)

    async def feed(self, pcm):
        self.speaker.feed(pcm)
        self.wake.set()
        result, endpoint = await asyncio.to_thread(self.decode, pcm)
        text = result.get('text', '')
        self.token_times_ms = [round((result.get('start_time', 0) + timestamp) * 1000) for timestamp in result.get('timestamps', [])]
        if text != self.last_text and text.strip():
            self.publish(text, False)
        if endpoint:
            self.close_utterance(text)

    def close_utterance(self, text):
        end_ms = self.samples // 16
        if text.strip():
            self.publish(text, True, end_ms)
            self.speaker.utterance(self.index, self.start_ms, end_ms)
            if self.refine_task:
                try:
                    self.refine_queue.put_nowait(dict(self.records[self.index * 100]))
                except asyncio.QueueFull:
                    self.refinement_error('Queue full; retained fast transcript')
        recognizer.reset(self.stream)
        self.index += 1
        self.start_ms, self.last_text = end_ms, ''
        self.token_times_ms = []
        self.wake.set()

    async def run_speakers(self):
        try:
            while True:
                await self.wake.wait()
                self.wake.clear()
                if hasattr(self.speaker, '_next_chunk'):
                    chunk = self.speaker._next_chunk()
                    if chunk is not None:
                        active, self.speaker.cache = await asyncio.to_thread(self.speaker._infer, *chunk)
                        self.speaker._accept(active, last=chunk[2])
                    labels = self.speaker.ready()
                    if not self.speaker.finished and self.speaker._next_chunk() is not None:
                        self.wake.set()
                else:
                    labels = await self.speaker.advance()
                for label in labels:
                    index = label['utterance'] * 100
                    record = self.records.get(index)
                    if record and not record.get('overlap_processed'):
                        speaker = label['speaker']
                        record = {**record, 'speaker': speaker.replace('说话人', '发言人', 1) if speaker else None}
                        self.records[index] = record
                        self.pending[index] = dict(record)
                        # Experimental intra-utterance turns amplify short activity flips.
                        # Keep whole-utterance speaker voting unless explicitly enabled.
                        if self.refine_task and os.getenv("MEETINGBOX_EXPERIMENTAL_SPEAKER_TURNS", "0") == "1":
                            from speaker_turns import speaker_turns
                            turns = speaker_turns(self.speaker.activity[:self.speaker.frames], record['start_ms'], record['end_ms'])
                            if len({turn['column'] for turn in turns if turn['column'] is not None}) > 1:
                                for turn in turns:
                                    column = turn.pop('column')
                                    if column is None:
                                        turn['speaker'] = None
                                        continue
                                    name = self.speaker.labels.setdefault(column, f'说话人{len(self.speaker.labels)+1}')
                                    turn['speaker'] = name.replace('说话人', '发言人', 1)
                                try:
                                    self.refine_queue.put_nowait({**record, 'turns': turns})
                                    record = {**record, 'speaker': None, 'mixed_speakers': True}
                                    self.records[index] = record
                                    self.pending[index] = dict(record)
                                except asyncio.QueueFull:
                                    self.refinement_error('Speaker-turn queue full; retained original')
                        with (self.folder / 'stream.events.jsonl').open('a', encoding='utf8') as out:
                            out.write(json.dumps(record, ensure_ascii=False) + '\n')
                if self.speaker.ended and getattr(self.speaker, 'finished', True):
                    break
        except Exception as error:
            (self.folder / 'speaker-error.txt').write_text(type(error).__name__ + ': ' + str(error), encoding='utf8')

    def take(self):
        records = list(self.pending.values())
        self.pending.clear()
        return {'segments': records, 'continuous': True, 'upsert': True, 'diarization': 'asynchronous'}

    def refinement_error(self, message):
        with (self.folder / 'refinement-errors.log').open('a', encoding='utf8') as out:
            out.write(str(message) + '\n')

    def refine(self, record):
        with (self.folder / 'stream.pcm').open('rb') as audio:
            # Include a small preceding silence margin to avoid cutting the initial phoneme.
            start_ms = max(0, record['start_ms'] - (0 if record.get('speaker_turn') else 300))
            audio.seek(start_ms * 32)
            pcm = audio.read((record['end_ms'] - start_ms) * 32)
        request = urllib.request.Request(self.refine_url + '/transcribe', data=pcm,
                                         headers={'Content-Type': 'application/octet-stream',
                                                  'X-ASR-Context': base64.b64encode(self.context.encode()).decode()})
        with urllib.request.urlopen(request, timeout=15) as response:
            return json.load(response)

    async def run_refinement(self):
        while True:
            record = await self.refine_queue.get()
            try:
                if record.get('turns'):
                    from aligned_turns import distribute_aligned_text
                    latest = self.records[record['utterance']]
                    aligned = latest.get('alignment') or await asyncio.to_thread(self.align, latest)
                    pieces = distribute_aligned_text(latest['text'], aligned['words'], record['turns'], aligned['audio_start_ms'])
                    refined = []
                    for sub, turn in enumerate(pieces):
                        part = {**latest, **turn, 'utterance': record['utterance'] + sub,
                                'speaker_turn': True, 'is_partial': False,
                                'token_times_ms': [], 'emitted_wall_time': time.time()}
                        part.pop('turns', None)
                        part.pop('mixed_speakers', None)
                        part.update(speech_start_ms=part['start_ms'], speech_end_ms=part['end_ms'])
                        part.update(is_refined=True, full_utterance_text=latest['text'],
                                    alignment_seconds=aligned.get('seconds'))
                        refined.append(part)
                    for part in refined:
                        self.records[part['utterance']] = part
                        self.pending[part['utterance']] = dict(part)
                        with (self.folder / 'stream.events.jsonl').open('a', encoding='utf8') as out:
                            out.write(json.dumps(part, ensure_ascii=False) + '\n')
                    continue
                result = await asyncio.to_thread(self.refine, record)
                text = result.get('text', '').strip()
                # Speaker updates may arrive during GPU inference; preserve the latest record.
                if text and len(text) <= 2000:
                    latest = self.records[record['utterance']]
                    if latest.get('speaker_turn'):
                        continue
                    updated = {**latest, 'text': text, 'is_partial': False,
                               'is_refined': True, 'draft_text': record['text'],
                               'refine_model': result.get('model'), 'refine_seconds': result.get('seconds')}
                    try:
                        aligned = await asyncio.to_thread(self.align, updated)
                        words = aligned['words']
                        if words:
                            origin = aligned['audio_start_ms']
                            updated.update(alignment=aligned,
                                speech_start_ms=max(0, origin + round(words[0]['start_time'] * 1000)),
                                speech_end_ms=min(updated['end_ms'], origin + round(words[-1]['end_time'] * 1000)))
                    except Exception as error:
                        self.refinement_error('Word timing unavailable: ' + str(error))
                    # Label inference may have updated the record while alignment ran.
                    current = self.records[record['utterance']]
                    updated['speaker'] = current.get('speaker')
                    if current.get('mixed_speakers'):
                        updated['mixed_speakers'] = True
                    self.records[record['utterance']] = updated
                    self.pending[record['utterance']] = dict(updated)
                    with (self.folder / 'stream.events.jsonl').open('a', encoding='utf8') as out:
                        out.write(json.dumps(updated, ensure_ascii=False) + '\n')
            except Exception as error:
                self.refinement_error(type(error).__name__ + ': ' + str(error))
            finally:
                self.refine_queue.task_done()

    def align(self, record):
        start_ms = max(0, record['start_ms'] - 300)
        with (self.folder / 'stream.pcm').open('rb') as audio:
            audio.seek(start_ms * 32)
            pcm = audio.read((record['end_ms'] - start_ms) * 32)
        request = urllib.request.Request(self.refine_url + '/align', data=pcm,
            headers={'Content-Type':'application/octet-stream',
                     'X-ASR-Text':base64.b64encode(record['text'].encode()).decode()})
        with urllib.request.urlopen(request, timeout=15) as response:
            result = json.load(response)
        result['audio_start_ms'] = start_ms
        return result

    def finalize_word_speakers(self):
        if not any(record.get('alignment') for record in self.records.values()):
            return
        from word_speakers import assign_words, register_stable_labels
        activity = self.speaker.activity[:self.speaker.frames]
        register_stable_labels(activity, self.speaker.labels)
        for index, record in list(self.records.items()):
            if record.get('is_partial') or record.get('speaker_turn'):
                continue
            try:
                parts = assign_words(record, activity, self.speaker.labels)
                for sub, part in enumerate(parts):
                    updated = {**record, **part, 'utterance': index + sub,
                               'speech_start_ms': part['start_ms'], 'speech_end_ms': part['end_ms'],
                               'speaker_turn': True, 'token_times_ms': []}
                    self.records[index + sub] = updated
                    self.pending[index + sub] = dict(updated)
            except Exception as error:
                self.refinement_error('Final word attribution retained original: ' + str(error))

    async def finish(self):
        async with self.lock:
            if not self.done.is_set():
                result, _ = await asyncio.to_thread(self.decode, b'\0' * 16000, True)
                self.close_utterance(result.get('text', ''))
                self.speaker.ended = True
                self.wake.set()
                try:
                    await asyncio.wait_for(asyncio.shield(self.speaker_task), 15)
                except asyncio.TimeoutError:
                    # Raw audio remains available for a later attribution pass.
                    self.speaker_task.cancel()
                if self.refine_task:
                    try:
                        await asyncio.wait_for(self.refine_queue.join(), 15)
                    except asyncio.TimeoutError:
                        self.refinement_error('Stop deadline reached; retained available transcripts')
                    self.refine_task.cancel()
                self.finalize_word_speakers()
                self.speaker.audio = np.empty(0, dtype=np.float32)
                self.speaker.activity = np.empty((0, 8), dtype=bool)
                self.speaker.cache = None
                self.done.set()
                (self.folder / 'live-final.json').write_text(json.dumps(list(self.records.values()), ensure_ascii=False), encoding='utf8')
            return self.take()

