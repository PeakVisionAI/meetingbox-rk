import { invoke } from '@tauri-apps/api/core';

export interface MinuteSummaryRecord {
  minute_index: number;
  start_seconds: number;
  end_seconds: number;
  summary: string;
  status?: 'completed' | 'empty' | 'error';
}

interface MinuteSummarySegment {
  text: string;
  speaker?: string;
}

export const minuteSummaryService = {
  generate: (templateId: string, minuteIndex: number, segments: MinuteSummarySegment[]) => invoke<{ minute_index: number; summary: string }>('api_generate_minute_summary', {
    templateId,
    minuteIndex,
    segments,
  }),
  save: (meetingId: string, summaries: MinuteSummaryRecord[]) => invoke<void>('api_save_meeting_minute_summaries', {
    meetingId,
    summaries: summaries.map(({ minute_index, start_seconds, end_seconds, summary }) => ({ minute_index, start_seconds, end_seconds, summary })),
  }),
  list: (meetingId: string) => invoke<MinuteSummaryRecord[]>('api_get_meeting_minute_summaries', { meetingId }),
};
