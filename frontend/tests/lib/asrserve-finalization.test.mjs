import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';
import ts from 'typescript';
import { test } from 'node:test';

const source = fs.readFileSync(new URL('../../src/hooks/useRecordingStop.ts', import.meta.url), 'utf8');
const start = source.indexOf('function stripProvisionalSpeakerPrefix');
const end = source.indexOf('/**', start);
const code = ts.transpileModule(source.slice(start, end).replace('export function', 'function'),
  { compilerOptions: { target: ts.ScriptTarget.ES2020 } }).outputText;
const context = vm.createContext({});
vm.runInContext(code, context);
const finalize = context.finalizedSpeakerSegmentsToTranscripts;
const live = [{ id: 'live1', text: '原片段', audio_start_time: 0, audio_end_time: 2 }];
const segments = [{ start_ms: 0, end_ms: 1000, text: '整场第一句', speaker: '发言人1' },
  { start_ms: 1000, end_ms: 2000, text: '整场第二句', speaker: '发言人2' }];

test('AsrServe full-meeting result adopts text and speaker boundaries', () => {
  const result = finalize({ finalized: true, segments, transcription_mode: 'full-meeting' }, live);
  assert.equal(result.length, 2);
  assert.equal(result[1].text, '整场第二句');
  assert.equal(result[1].speaker, '发言人2');
  assert.equal(result[1].audio_start_time, 1);
  assert.equal(live[0].text, '原片段');
});
test('Existing RK calibration keeps live transcript text', () => {
  const result = finalize({ finalized: true, segments }, live);
  assert.equal(result.length, 1);
  assert.equal(result[0].text, '原片段');
});
test('Failed finalization preserves all live transcripts', () => {
  assert.equal(finalize({ finalized: false, segments: [], error: 'unavailable' }, live), live);
});
