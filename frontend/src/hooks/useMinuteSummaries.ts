import { useCallback, useEffect, useRef, useState } from 'react';
import type { Transcript } from '@/types';
import { minuteSummaryService, type MinuteSummaryRecord } from '@/services/minuteSummaryService';

const SUMMARY_GRACE_SECONDS = 5;
export const SUMMARY_INTERVAL_SECONDS = 10 * 60;
const SUMMARY_CACHE_VERSION = 'ten-minute-v1';
const SUMMARY_CACHE_VERSION_KEY = 'pending_minute_summaries_version';
const SUMMARY_CACHE_KEY = 'pending_minute_summaries';

let summaryGenerationQueue: Promise<void> = Promise.resolve();

function transcriptStart(transcript: Transcript): number {
  return transcript.audio_start_time ?? transcript.chunk_start_time ?? 0;
}

function transcriptEnd(transcript: Transcript): number {
  const start = transcriptStart(transcript);
  return Math.max(
    start + 1,
    transcript.audio_end_time ?? start + (transcript.duration ?? 0),
  );
}

function splitSpeaker(text: string) {
  const match = text.match(/^((?:候选发言人|发言人)[^：:\s]{0,20})[：:]\s*([\s\S]*)$/);
  return { speaker: match?.[1], text: (match?.[2] || text).trim() };
}

function usesCurrentInterval(summary: MinuteSummaryRecord) {
  const expectedStart = summary.minute_index * SUMMARY_INTERVAL_SECONDS;
  const summaryText = summary.summary.trimStart();
  const containsRawJson = summary.status === 'completed'
    && summaryText.startsWith('{')
    && /"(?:topic|agenda|summary)"\s*:/.test(summaryText);
  return summary.start_seconds === expectedStart
    && summary.end_seconds === expectedStart + SUMMARY_INTERVAL_SECONDS
    && !containsRawJson;
}

function readCachedSummaries(): MinuteSummaryRecord[] {
  const stored = sessionStorage.getItem(SUMMARY_CACHE_KEY);
  if (!stored) return [];
  try {
    const parsed = JSON.parse(stored) as MinuteSummaryRecord[];
    return Array.isArray(parsed) ? parsed : [];
  } catch {
    return [];
  }
}

function cacheSummary(summary: MinuteSummaryRecord) {
  const next = [
    ...readCachedSummaries().filter((item) => item.minute_index !== summary.minute_index),
    summary,
  ].sort((left, right) => left.minute_index - right.minute_index);
  sessionStorage.setItem(SUMMARY_CACHE_KEY, JSON.stringify(next));
  return next;
}

function serializeSummaryGeneration<T>(task: () => Promise<T>): Promise<T> {
  const result = summaryGenerationQueue.then(task, task);
  summaryGenerationQueue = result.then(() => undefined, () => undefined);
  return result;
}

function segmentsForInterval(transcripts: Transcript[], startSeconds: number, endSeconds: number) {
  return transcripts
    .filter((transcript) => {
      const start = transcriptStart(transcript);
      return !transcript.is_partial && start >= startSeconds && start < endSeconds;
    })
    .map((transcript) => splitSpeaker(transcript.text))
    .filter((segment) => segment.text.length > 0);
}

async function generateSummaryRecord(
  transcripts: Transcript[],
  templateId: string,
  minuteIndex: number,
  endSeconds: number,
): Promise<MinuteSummaryRecord> {
  const startSeconds = minuteIndex * SUMMARY_INTERVAL_SECONDS;
  const segments = segmentsForInterval(transcripts, startSeconds, endSeconds);
  if (!segments.length) {
    return {
      minute_index: minuteIndex,
      start_seconds: startSeconds,
      end_seconds: endSeconds,
      summary: '',
      status: 'empty',
    };
  }

  try {
    const result = await minuteSummaryService.generate(templateId, minuteIndex, segments);
    return {
      minute_index: minuteIndex,
      start_seconds: startSeconds,
      end_seconds: endSeconds,
      summary: result.summary,
      status: 'completed',
    };
  } catch (error) {
    console.warn(`Summary interval ${minuteIndex} failed:`, error);
    return {
      minute_index: minuteIndex,
      start_seconds: startSeconds,
      end_seconds: endSeconds,
      summary: '本时段摘要生成失败，可在会后总结中重新处理。',
      status: 'error',
    };
  }
}

export function generateMinuteSummaryInterval(
  transcripts: Transcript[],
  templateId: string,
  minuteIndex: number,
  endSeconds = (minuteIndex + 1) * SUMMARY_INTERVAL_SECONDS,
) {
  return serializeSummaryGeneration(async () => {
    const summary = await generateSummaryRecord(transcripts, templateId, minuteIndex, endSeconds);
    cacheSummary(summary);
    return summary;
  });
}

export function finalTranscriptEndSeconds(transcripts: Transcript[]) {
  return Math.ceil(transcripts
    .filter((transcript) => !transcript.is_partial && transcript.text.trim().length > 0)
    .reduce((latest, transcript) => Math.max(latest, transcriptEnd(transcript)), 0));
}

export function finalizeMinuteSummaries(transcripts: Transcript[], templateId: string) {
  return serializeSummaryGeneration(async () => {
    const meetingEndSeconds = finalTranscriptEndSeconds(transcripts);
    if (meetingEndSeconds <= 0) return readCachedSummaries();

    const intervalCount = Math.ceil(meetingEndSeconds / SUMMARY_INTERVAL_SECONDS);
    const summaries = readCachedSummaries();
    const byIndex = new Map(summaries.map((summary) => [summary.minute_index, summary]));

    for (let minuteIndex = 0; minuteIndex < intervalCount; minuteIndex += 1) {
      const startSeconds = minuteIndex * SUMMARY_INTERVAL_SECONDS;
      const endSeconds = Math.min(startSeconds + SUMMARY_INTERVAL_SECONDS, meetingEndSeconds);
      const existing = byIndex.get(minuteIndex);
      if (
        existing?.status === 'completed'
        && existing.start_seconds === startSeconds
        && existing.end_seconds === endSeconds
      ) {
        continue;
      }

      const summary = await generateSummaryRecord(transcripts, templateId, minuteIndex, endSeconds);
      byIndex.set(minuteIndex, summary);
      cacheSummary(summary);
    }

    return [...byIndex.values()].sort((left, right) => left.minute_index - right.minute_index);
  });
}

export function formatMinuteRange(startSeconds: number, endSeconds: number) {
  const format = (seconds: number) => `${String(Math.floor(seconds / 60)).padStart(2, '0')}:${String(seconds % 60).padStart(2, '0')}`;
  return `${format(startSeconds)}–${format(endSeconds)}`;
}

export function useMinuteSummaries({
  activeDuration,
  isRecording,
  transcripts,
  templateId,
}: {
  activeDuration: number | null;
  isRecording: boolean;
  transcripts: Transcript[];
  templateId: string;
}) {
  const [summaries, setSummaries] = useState<MinuteSummaryRecord[]>([]);
  const [generatingMinute, setGeneratingMinute] = useState<number | null>(null);
  const nextMinuteRef = useRef(0);
  const processingRef = useRef(false);

  const appendSummary = useCallback((summary: MinuteSummaryRecord) => {
    setSummaries((current) => {
      return [...current.filter((item) => item.minute_index !== summary.minute_index), summary]
        .sort((left, right) => left.minute_index - right.minute_index);
    });
  }, []);

  useEffect(() => {
    if (!isRecording) return;
    if (sessionStorage.getItem(SUMMARY_CACHE_VERSION_KEY) !== SUMMARY_CACHE_VERSION) {
      sessionStorage.removeItem(SUMMARY_CACHE_KEY);
      sessionStorage.setItem(SUMMARY_CACHE_VERSION_KEY, SUMMARY_CACHE_VERSION);
      setSummaries([]);
      nextMinuteRef.current = 0;
      return;
    }
    const stored = sessionStorage.getItem(SUMMARY_CACHE_KEY);
    if (!stored) return;
    try {
      const restored = JSON.parse(stored) as MinuteSummaryRecord[];
      if (!restored.every(usesCurrentInterval)) {
        sessionStorage.removeItem(SUMMARY_CACHE_KEY);
        setSummaries([]);
        nextMinuteRef.current = 0;
        return;
      }
      setSummaries(restored);
      nextMinuteRef.current = restored.length ? Math.max(...restored.map((item) => item.minute_index)) + 1 : 0;
    } catch {
      sessionStorage.removeItem(SUMMARY_CACHE_KEY);
    }
  }, [isRecording]);

  useEffect(() => {
    if (!isRecording || processingRef.current) return;
    const completedIntervals = Math.floor(
      Math.max(0, (activeDuration || 0) - SUMMARY_GRACE_SECONDS) / SUMMARY_INTERVAL_SECONDS,
    );
    if (nextMinuteRef.current >= completedIntervals) return;

    processingRef.current = true;
    const processPendingMinutes = async () => {
      while (nextMinuteRef.current < completedIntervals) {
        const minuteIndex = nextMinuteRef.current;
        setGeneratingMinute(minuteIndex);
        try {
          const summary = await generateMinuteSummaryInterval(transcripts, templateId, minuteIndex);
          appendSummary(summary);
        } finally {
          setGeneratingMinute(null);
          nextMinuteRef.current += 1;
        }
      }
    };
    void processPendingMinutes().finally(() => {
      processingRef.current = false;
    });
  }, [activeDuration, appendSummary, isRecording, templateId, transcripts]);

  return { summaries, generatingMinute };
}
