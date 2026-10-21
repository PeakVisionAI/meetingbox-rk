import { invoke } from '@tauri-apps/api/core';

export interface MinuteSummaryRecord {
  minute_index: number;
  start_seconds: number;
  end_seconds: number;
  summary: string;
  status?: 'completed' | 'empty' | 'error';
  structure?: Record<string, unknown>;
  source_text?: string;
}

interface MinuteSummarySegment {
  text: string;
  speaker?: string;
  start_seconds?: number;
}

export const minuteSummaryService = {
  generate: (templateId: string, minuteIndex: number, segments: MinuteSummarySegment[], previousContext: Record<string, unknown> = {}) => invoke<{ minute_index: number; summary: string; structure: Record<string, unknown> }>('api_generate_minute_summary', {
    templateId,
    minuteIndex,
    segments,
    previousContext,
  }),
  save: (meetingId: string, summaries: MinuteSummaryRecord[]) => invoke<void>('api_save_meeting_minute_summaries', {
    meetingId,
    summaries: summaries.filter(item => item.status === 'completed' || item.status === undefined).map(({ minute_index, start_seconds, end_seconds, summary, structure, source_text }) => ({ minute_index, start_seconds, end_seconds, summary, structure, source_text })),
  }),
  list: (meetingId: string) => invoke<MinuteSummaryRecord[]>('api_get_meeting_minute_summaries', { meetingId }),
};
