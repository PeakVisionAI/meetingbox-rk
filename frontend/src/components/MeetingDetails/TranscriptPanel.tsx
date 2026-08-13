"use client";

import { Transcript, TranscriptSegmentData } from '@/types';
import { TranscriptView } from '@/components/TranscriptView';
import { VirtualizedTranscriptView } from '@/components/VirtualizedTranscriptView';
import { TranscriptButtonGroup } from './TranscriptButtonGroup';
import { useMemo } from 'react';
import { Pause, Play, Volume2 } from 'lucide-react';
import { uiText } from '@/lib/uiText';
import { useMeetingAudioPlayer } from '@/hooks/useMeetingAudioPlayer';

interface TranscriptPanelProps {
  transcripts: Transcript[];
  customPrompt: string;
  onPromptChange: (value: string) => void;
  onCopyTranscript: () => void;
  onOpenMeetingFolder: () => Promise<void>;
  isRecording: boolean;
  disableAutoScroll?: boolean;

  // Optional pagination props (when using virtualization)
  usePagination?: boolean;
  segments?: TranscriptSegmentData[];
  hasMore?: boolean;
  isLoadingMore?: boolean;
  totalCount?: number;
  loadedCount?: number;
  onLoadMore?: () => void;

  // Retranscription props
  meetingId?: string;
  meetingFolderPath?: string | null;
  onRefetchTranscripts?: () => Promise<void>;
}

export function TranscriptPanel({
  transcripts,
  customPrompt,
  onPromptChange,
  onCopyTranscript,
  onOpenMeetingFolder,
  isRecording,
  disableAutoScroll = false,
  usePagination = false,
  segments,
  hasMore,
  isLoadingMore,
  totalCount,
  loadedCount,
  onLoadMore,
  meetingId,
  meetingFolderPath,
  onRefetchTranscripts,
}: TranscriptPanelProps) {
  const audioPlayer = useMeetingAudioPlayer(meetingFolderPath);
  // Convert transcripts to segments if pagination is not used but we want virtualization
  const convertedSegments = useMemo(() => {
    if (usePagination && segments) {
      return segments;
    }
    // Convert transcripts to segments for virtualization
    return transcripts.map(t => ({
      id: t.id,
      timestamp: t.audio_start_time ?? 0,
      endTime: t.audio_end_time,
      text: t.text,
      confidence: t.confidence,
    }));
  }, [transcripts, usePagination, segments]);

  return (
    <div className="hyos-details-transcript min-w-0 bg-white flex-col relative shrink-0">
      {/* Title area */}
      <div className="border-b border-gray-200 p-4">
        <TranscriptButtonGroup
          transcriptCount={usePagination ? (totalCount ?? convertedSegments.length) : (transcripts?.length || 0)}
          onCopyTranscript={onCopyTranscript}
          onOpenMeetingFolder={onOpenMeetingFolder}
          meetingId={meetingId}
          meetingFolderPath={meetingFolderPath}
          onRefetchTranscripts={onRefetchTranscripts}
        />
        <div className="mt-3 rounded-lg border border-slate-200 bg-slate-50 p-3">
          <div className="mb-2 flex items-center gap-2 text-xs font-semibold text-slate-600">
            <Volume2 size={15} />
            <span>会议录音</span>
            {audioPlayer.isLoading && <span className="font-normal text-slate-400">正在加载…</span>}
          </div>
          <div className="flex items-center gap-3">
            <button
              type="button"
              className="flex h-8 w-8 flex-shrink-0 items-center justify-center rounded-full bg-blue-600 text-white disabled:cursor-not-allowed disabled:bg-slate-300"
              onClick={() => audioPlayer.toggle()}
              disabled={!audioPlayer.audioPath || audioPlayer.isLoading}
              title={audioPlayer.isPlaying ? '暂停录音' : '播放录音'}
              aria-label={audioPlayer.isPlaying ? '暂停录音' : '播放录音'}
            >
              {audioPlayer.isPlaying ? <Pause size={15} /> : <Play size={15} />}
            </button>
            <input
              type="range"
              min={0}
              max={audioPlayer.duration || 0}
              step={0.1}
              value={Math.min(audioPlayer.currentTime, audioPlayer.duration || 0)}
              onChange={(event) => audioPlayer.seek(Number(event.target.value))}
              disabled={!audioPlayer.audioPath}
              className="min-w-0 flex-1 accent-blue-600"
              aria-label="录音进度"
            />
            <span className="w-20 text-right text-xs tabular-nums text-slate-500">
              {formatTime(audioPlayer.currentTime)} / {formatTime(audioPlayer.duration)}
            </span>
          </div>
          {audioPlayer.error && <p className="mt-2 text-xs text-amber-600">{audioPlayer.error}</p>}
          {!audioPlayer.error && audioPlayer.audioPath && <p className="mt-2 text-[11px] text-slate-400">点击逐字稿右侧耳机按钮，从对应时间开始播放</p>}
        </div>
      </div>

      {/* Transcript content - use virtualized view for better performance */}
      <div className="flex-1 overflow-hidden pb-4">
        <VirtualizedTranscriptView
          segments={convertedSegments}
          isRecording={isRecording}
          isPaused={false}
          isProcessing={false}
          isStopping={false}
          enableStreaming={false}
          showConfidence={true}
          disableAutoScroll={disableAutoScroll}
          hasMore={hasMore}
          isLoadingMore={isLoadingMore}
          totalCount={totalCount}
          loadedCount={loadedCount}
          onLoadMore={onLoadMore}
          onPlaySegment={audioPlayer.playSegment}
        />
      </div>

      {/* Custom prompt input at bottom of transcript section */}
      {!isRecording && convertedSegments.length > 0 && (
        <div className="p-1 border-t border-gray-200">
          <textarea
            placeholder={uiText.summaryContextPlaceholder}
            className="w-full px-3 py-2 border border-gray-200 rounded-md text-sm focus:outline-none focus:ring-1 focus:ring-blue-500 focus:border-blue-500 bg-white shadow-sm min-h-[80px] resize-y"
            value={customPrompt}
            onChange={(e) => onPromptChange(e.target.value)}
          />
        </div>
      )}
    </div>
  );
}

function formatTime(seconds: number) {
  if (!Number.isFinite(seconds) || seconds <= 0) return '00:00';
  const total = Math.floor(seconds);
  return `${Math.floor(total / 60).toString().padStart(2, '0')}:${(total % 60).toString().padStart(2, '0')}`;
}
