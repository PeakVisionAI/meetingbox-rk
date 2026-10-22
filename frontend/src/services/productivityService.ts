import { invoke } from '@tauri-apps/api/core';

export interface MeetingSearchResult {
  id: string;
  title: string;
  created_at: string;
  updated_at: string;
  folder_path?: string | null;
  transcript_count: number;
  duration_seconds?: number | null;
}

export interface PaginatedMeetingSearchResponse {
  meetings: MeetingSearchResult[];
  total_count: number;
  has_more: boolean;
}

export interface SpeakerLabel {
  meeting_id: string;
  speaker_key: string;
  display_name: string;
  contact_id?: string | null;
}

export interface SpeakerContact {
  id: string;
  name: string;
  note?: string | null;
  created_at: string;
  updated_at: string;
}

export interface Hotword {
  id: string;
  phrase: string;
  replacement?: string | null;
  enabled: boolean;
}

export interface MeetingAction {
  id: string;
  meeting_id: string;
  content: string;
  assignee?: string | null;
  due_date?: string | null;
  status: 'open' | 'done' | string;
  source_transcript_id?: string | null;
  created_at: string;
  updated_at: string;
}

export const productivityService = {
  searchMeetings: (params: { query?: string; speaker?: string; dateFrom?: string; dateTo?: string; limit?: number; offset?: number }) => invoke<PaginatedMeetingSearchResponse>('api_search_meetings', {
    query: params.query ?? '',
    speaker: params.speaker ?? '',
    dateFrom: params.dateFrom ?? '',
    dateTo: params.dateTo ?? '',
    limit: params.limit ?? 30,
    offset: params.offset ?? 0,
  }),
  listSpeakerLabels: (meetingId: string) => invoke<SpeakerLabel[]>('api_list_speaker_labels', { meetingId }),
  saveSpeakerLabel: (meetingId: string, speakerKey: string, displayName: string, contactId?: string | null) => invoke<SpeakerLabel>('api_save_speaker_label', { meetingId, speakerKey, displayName, contactId: contactId ?? null }),
  mergeSpeakerLabels: (meetingId: string, fromSpeaker: string, toSpeaker: string) => invoke<void>('api_merge_speaker_labels', { meetingId, fromSpeaker, toSpeaker }),
  listSpeakerContacts: () => invoke<SpeakerContact[]>('api_list_speaker_contacts'),
  saveSpeakerContact: (name: string, note?: string, id?: string) => invoke<SpeakerContact>('api_save_speaker_contact', { id: id ?? null, name, note: note ?? null }),
  listHotwords: () => invoke<Hotword[]>('api_list_hotwords'),
  saveHotword: (phrase: string, replacement: string, enabled: boolean, id?: string) => invoke<Hotword>('api_save_hotword', { id: id ?? null, phrase, replacement: replacement || null, enabled }),
  deleteHotword: (id: string) => invoke<void>('api_delete_hotword', { id }),
  listMeetingActions: (meetingId: string) => invoke<MeetingAction[]>('api_list_meeting_actions', { meetingId }),
  extractMeetingActions: (meetingId: string) => invoke<MeetingAction[]>('api_extract_meeting_actions', { meetingId }),
  updateMeetingAction: (action: MeetingAction) => invoke<void>('api_update_meeting_action', { id: action.id, content: action.content, assignee: action.assignee ?? null, dueDate: action.due_date ?? null, status: action.status }),
  updateTranscript: (transcriptId: string, text: string, speaker?: string | null) => invoke<void>('api_update_transcript', { transcriptId, text, speaker: speaker ?? null }),
  suggestTranscriptCorrection: (transcriptId: string) => invoke<{ original: string; suggestion: string; status: string }>('api_suggest_transcript_correction', { transcriptId }),
  exportMeeting: (meetingId: string, format: 'md' | 'txt' | 'srt' | 'docx' | 'pdf') => invoke<string>('api_export_meeting_format', { meetingId, format }),
};
