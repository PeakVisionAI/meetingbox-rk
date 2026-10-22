import { useState, useEffect, useCallback, useRef } from 'react';
import { useRouter } from 'next/navigation';
import { listen } from '@tauri-apps/api/event';
import { toast } from 'sonner';
import { useTranscripts } from '@/contexts/TranscriptContext';
import { useSidebar } from '@/components/Sidebar/SidebarProvider';
import { useRecordingState, RecordingStatus } from '@/contexts/RecordingStateContext';
import { storageService } from '@/services/storageService';
import { transcriptService } from '@/services/transcriptService';
import Analytics from '@/lib/analytics';
import {
  applyPinnedSummaryLanguageToMeeting,
  detectAndCacheSummaryLanguage,
} from '@/lib/summary-language-preferences';
import { accessControlService } from '@/services/accessControlService';
import type { SpeakerCalibrationResult } from '@/services/recordingService';
import type { Transcript } from '@/types';
import { minuteSummaryService } from '@/services/minuteSummaryService';
import { finalizeMinuteSummaries } from '@/hooks/useMinuteSummaries';

type SummaryStatus = 'idle' | 'processing' | 'summarizing' | 'regenerating' | 'completed' | 'error';

interface UseRecordingStopReturn {
  handleRecordingStop: (
    callApi: boolean,
    speakerCalibration?: SpeakerCalibrationResult,
  ) => Promise<void>;
  isStopping: boolean;
  isProcessingTranscript: boolean;
  isSavingTranscript: boolean;
  summaryStatus: SummaryStatus;
  setIsStopping: (value: boolean) => void;
}

function stripProvisionalSpeakerPrefix(text: string): string {
  return text.replace(/(?:候选发言人|发言人\s*(?:待识别|\d+))[：:]\s*/g, '').trim();
}

export function finalizedSpeakerSegmentsToTranscripts(
  calibration: SpeakerCalibrationResult,
  fallback: Transcript[],
): Transcript[] {
  if (!calibration.finalized || calibration.segments.length === 0) return fallback;

  // The finalizer is a diarization pass, not a second transcription pass. Some
  // RK service versions return one full-meeting segment even when the live ASR
  // produced hundreds of chunks. Never replace those chunks with the merged
  // response; only overlay the speaker label that covers each chunk in time.
  if (fallback.length > 0) {
    const calibrationSegments = calibration.segments;
    return fallback.map((transcript) => {
      const start = transcript.audio_start_time ?? transcript.chunk_start_time ?? 0;
      const end = transcript.audio_end_time ?? start + (transcript.duration ?? 0);
      const midpoint = start + Math.max(0, end - start) / 2;
      const matchingSegment = calibrationSegments.find((segment) => {
        const segmentStart = segment.start_ms / 1000;
        const segmentEnd = segment.end_ms / 1000;
        return midpoint >= segmentStart && midpoint <= segmentEnd;
      }) ?? calibrationSegments.reduce((closest, segment) => {
        const distance = Math.abs(midpoint - (segment.start_ms / 1000));
        const closestDistance = Math.abs(midpoint - (closest.start_ms / 1000));
        return distance < closestDistance ? segment : closest;
      }, calibrationSegments[0]);
      const speaker = matchingSegment.speaker?.trim();
      return speaker
        ? { ...transcript, text: stripProvisionalSpeakerPrefix(transcript.text), speaker }
        : transcript;
    });
  }

  // Imported/fallback-free meetings can still use finalized diarization output.
  return calibration.segments.map((segment, index) => {
    const speaker = segment.speaker?.trim();
    const text = segment.text.trim();
    return {
      id: `final-speaker-${index + 1}`,
      text,
      speaker,
      timestamp: new Date().toLocaleTimeString('zh-CN', { hour12: false }),
      sequence_id: index + 1,
      chunk_start_time: segment.start_ms / 1000,
      is_partial: false,
      audio_start_time: segment.start_ms / 1000,
      audio_end_time: segment.end_ms / 1000,
      duration: Math.max(0, segment.end_ms - segment.start_ms) / 1000,
    };
  });
}

/**
 * Custom hook for managing recording stop lifecycle.
 * Handles the complex stop sequence: transcription wait → buffer flush → SQLite save → navigation.
 *
 * Features:
 * - Transcription completion polling (60s max, 500ms interval)
 * - Transcript buffer flush coordination
 * - SQLite meeting save with folder_path from sessionStorage
 * - Comprehensive analytics tracking (duration, word count, activation)
 * - Auto-navigation to meeting details
 * - Toast notifications for success/error
 * - Window exposure for Rust callbacks
 */
export function useRecordingStop(
  setIsRecording: (value: boolean) => void,
  setIsRecordingDisabled: (value: boolean) => void
): UseRecordingStopReturn {
  // USE global state instead
  const recordingState = useRecordingState();
  const {
    status,
    setStatus,
    isStopping,
    isProcessing: isProcessingTranscript,
    isSaving: isSavingTranscript
  } = recordingState;

  const {
    transcriptsRef,
    flushBuffer,
    clearTranscripts,
    meetingTitle,
    markMeetingAsSaved,
  } = useTranscripts();

  const {
    refetchMeetings,
    setCurrentMeeting,
    setMeetings,
    meetings,
    setIsMeetingActive,
  } = useSidebar();

  const router = useRouter();

  // Guard to prevent duplicate/concurrent stop calls (e.g., from UI and tray simultaneously)
  const stopInProgressRef = useRef(false);

  // Promise to track recording-stopped event data (fixes race condition with recording-stop-complete)
  const recordingStoppedDataRef = useRef<Promise<void> | null>(null);

  // Set up recording-stopped listener for meeting navigation
  useEffect(() => {
    let unlistenFn: (() => void) | undefined;

    const setupRecordingStoppedListener = async () => {
      try {
        console.log('Setting up recording-stopped listener for navigation...');
        unlistenFn = await listen<{
          message: string;
          folder_path?: string;
          meeting_name?: string;
        }>('recording-stopped', async (event) => {
          // Create promise that resolves when sessionStorage is set (prevents race condition)
          recordingStoppedDataRef.current = (async () => {
            const { folder_path, meeting_name } = event.payload;

            // Store folder_path and meeting_name for later use in handleRecordingStop
            if (folder_path) {
              sessionStorage.setItem('last_recording_folder_path', folder_path);
            }
            if (meeting_name) {
              sessionStorage.setItem('last_recording_meeting_name', meeting_name);
            }
          })();

        });
        console.log('Recording stopped listener setup complete');
      } catch (error) {
        console.error('Failed to setup recording stopped listener:', error);
      }
    };

    setupRecordingStoppedListener();

    return () => {
      console.log('Cleaning up recording stopped listener...');
      if (unlistenFn) {
        unlistenFn();
      }
    };
  }, [router]);

  // Main recording stop handler
  const handleRecordingStop = useCallback(async (
    isCallApi: boolean,
    speakerCalibration?: SpeakerCalibrationResult,
  ) => {
    if (recordingStoppedDataRef.current) {
      await recordingStoppedDataRef.current;
    }

    // Guard: prevent duplicate/concurrent stop calls
    if (stopInProgressRef.current) {
      return;
    }
    stopInProgressRef.current = true;

    // Set status to STOPPING immediately
    setStatus(RecordingStatus.STOPPING);
    setIsRecording(false);
    setIsRecordingDisabled(true);
    const stopStartTime = Date.now();

    try {
      console.log('Post-stop processing (new implementation)...', {
        stop_initiated_at: new Date(stopStartTime).toISOString(),
        current_transcript_count: transcriptsRef.current.length
      });

      // Note: stop_recording is already called by RecordingControls.stopRecordingAction
      // This function only handles post-stop processing (transcription wait, API call, navigation)
      console.log('Recording already stopped by RecordingControls, processing transcription...');

      // Wait for transcription to complete
      setStatus(RecordingStatus.PROCESSING_TRANSCRIPTS, 'Waiting for transcription...');
      console.log('Waiting for transcription to complete...');

      const MAX_WAIT_TIME = 60000; // 60 seconds maximum wait (increased for longer processing)
      const POLL_INTERVAL = 500; // Check every 500ms
      let elapsedTime = 0;
      let transcriptionComplete = false;

      // Listen for transcription-complete event
      const unlistenComplete = await listen('transcription-complete', () => {
        console.log('Received transcription-complete event');
        transcriptionComplete = true;
      });

      // Poll for transcription status
      while (elapsedTime < MAX_WAIT_TIME && !transcriptionComplete) {
        try {
          const status = await transcriptService.getTranscriptionStatus();
          console.log('Transcription status:', status);

          // Check if transcription is complete
          if (!status.is_processing && status.chunks_in_queue === 0) {
            console.log('Transcription complete - no active processing and no chunks in queue');
            transcriptionComplete = true;
            break;
          }

          // If no activity for more than 8 seconds and no chunks in queue, consider it done (increased from 5s to 8s)
          if (status.last_activity_ms > 8000 && status.chunks_in_queue === 0) {
            console.log('Transcription likely complete - no recent activity and empty queue');
            transcriptionComplete = true;
            break;
          }

          // Update user with current status
          if (status.chunks_in_queue > 0) {
            console.log(`Processing ${status.chunks_in_queue} remaining audio chunks...`);
            setStatus(RecordingStatus.PROCESSING_TRANSCRIPTS, `Processing ${status.chunks_in_queue} remaining chunks...`);
          }

          // Wait before next check
          await new Promise(resolve => setTimeout(resolve, POLL_INTERVAL));
          elapsedTime += POLL_INTERVAL;
        } catch (error) {
          console.error('Error checking transcription status:', error);
          break;
        }
      }

      // Clean up listener
      console.log('🧹 CLEANUP: Cleaning up transcription-complete listener');
      unlistenComplete();

      if (!transcriptionComplete && elapsedTime >= MAX_WAIT_TIME) {
        console.warn('⏰ Transcription wait timeout reached after', elapsedTime, 'ms');
      } else {
        console.log('✅ Transcription completed after', elapsedTime, 'ms');
        // Wait longer for any late transcript segments (increased from 1s to 4s)
        console.log('⏳ Waiting for late transcript segments...');
        await new Promise(resolve => setTimeout(resolve, 4000));
      }

      // Final buffer flush: process ALL remaining transcripts regardless of timing
      const flushStartTime = Date.now();
      console.log('🔄 Final buffer flush: forcing processing of any remaining transcripts...', {
        flush_started_at: new Date(flushStartTime).toISOString(),
        time_since_stop: flushStartTime - stopStartTime,
        current_transcript_count: transcriptsRef.current.length
      });
      setStatus(RecordingStatus.PROCESSING_TRANSCRIPTS, 'Flushing transcript buffer...');
      flushBuffer();
      const flushEndTime = Date.now();
      console.log('✅ Final buffer flush completed', {
        flush_duration: flushEndTime - flushStartTime,
        total_time_since_stop: flushEndTime - stopStartTime,
        final_transcript_count: transcriptsRef.current.length
      });

      // NOTE: Status remains PROCESSING_TRANSCRIPTS until we start saving

      // Wait a bit more to ensure all transcript state updates have been processed
      console.log('Waiting for transcript state updates to complete...');
      await new Promise(resolve => setTimeout(resolve, 500));

      // Save to SQLite
      // NOTE: enabled to save COMPLETE transcripts after frontend receives all updates
      // This ensures user sees all transcripts streaming in before database save
      if (isCallApi && transcriptionComplete == true) {

        setStatus(RecordingStatus.SAVING, 'Saving meeting to database...');

        // Get fresh transcript state (ALL transcripts including late ones)
        const freshTranscripts = speakerCalibration
          ? finalizedSpeakerSegmentsToTranscripts(
              speakerCalibration,
              [...transcriptsRef.current],
            )
          : [...transcriptsRef.current];

        if (speakerCalibration?.error) {
          toast.warning('说话人校准未完成', {
            description: '已保留实时候选标签，可稍后重新处理录音。',
          });
        }

        // Get folder_path and meeting_name from recording-stopped event
        const folderPath = sessionStorage.getItem('last_recording_folder_path');
        const savedMeetingName = sessionStorage.getItem('last_recording_meeting_name');
        const activeMeetingName = sessionStorage.getItem('active_recording_meeting_name');
        const activeMeetingStartedAt = sessionStorage.getItem('active_recording_started_at');
        const effectiveMeetingName = activeMeetingName || savedMeetingName || meetingTitle || 'New Meeting';

        console.log('💾 Saving COMPLETE transcripts to database...', {
          transcript_count: freshTranscripts.length,
          meeting_name: effectiveMeetingName,
          folder_path: folderPath,
          sample_text: freshTranscripts.length > 0 ? freshTranscripts[0].text.substring(0, 50) + '...' : 'none',
          last_transcript: freshTranscripts.length > 0 ? freshTranscripts[freshTranscripts.length - 1].text.substring(0, 30) + '...' : 'none',
        });

        try {
          const responseData = await storageService.saveMeeting(
            effectiveMeetingName,
            freshTranscripts,
            folderPath,
            activeMeetingStartedAt,
          );

          const meetingId = responseData.meeting_id;
          if (!meetingId) {
            console.error('No meeting_id in response:', responseData);
            throw new Error('No meeting ID received from save operation');
          }

          console.log('✅ Successfully saved COMPLETE meeting with ID:', meetingId);
          await accessControlService.setMeetingPassword(meetingId, null);
          const summaryTemplateId = sessionStorage.getItem('pending_summary_template_id') || 'standard_meeting';

          // The meeting is durable at this point. Show password setup on top of
          // its transcript/summary page immediately; non-critical post-processing
          // below must not hold the user on the recording screen.
          await markMeetingAsSaved();
          setCurrentMeeting({ id: meetingId, title: effectiveMeetingName });
          setStatus(RecordingStatus.COMPLETED);

          sessionStorage.removeItem('last_recording_folder_path');
          sessionStorage.removeItem('last_recording_meeting_name');
          sessionStorage.removeItem('active_recording_meeting_name');
          sessionStorage.removeItem('active_recording_started_at');
          sessionStorage.removeItem('indexeddb_current_meeting_id');
          sessionStorage.removeItem('pending_minute_summaries');
          sessionStorage.removeItem('pending_summary_template_id');

          const template = encodeURIComponent(summaryTemplateId);
          router.push(`/meeting-details?id=${meetingId}&source=recording&setPassword=1&template=${template}`);
          clearTranscripts();
          Analytics.trackPageView('meeting_details');
          setStatus(RecordingStatus.IDLE);

          let shouldDetectSummaryLanguage = false;
          try {
            shouldDetectSummaryLanguage = !(await applyPinnedSummaryLanguageToMeeting(meetingId));
          } catch (error) {
            console.warn('Failed to apply pinned summary language preference for new meeting:', error);
            toast.warning('Could not apply default summary language', {
              description: 'The meeting was saved, but the default summary language was not applied.',
            });
          }

          if (shouldDetectSummaryLanguage) {
            try {
              await detectAndCacheSummaryLanguage(
                meetingId,
                freshTranscripts.map(t => t.text)
              );
            } catch (error) {
              console.warn('Failed to detect summary language for new meeting:', error);
              toast.warning('Could not detect summary language', {
                description: 'The meeting was saved, but Auto could not detect the summary language.',
              });
            }
          }

          try {
            const minuteSummaries = await finalizeMinuteSummaries(freshTranscripts, summaryTemplateId);
            await minuteSummaryService.save(
              meetingId,
              minuteSummaries.filter((summary) => summary.status === 'completed'),
            );
          } catch (error) {
            console.warn('Failed to finalize minute summaries:', error);
            toast.warning('分时摘要暂未完整写入会议文件', { description: '完整逐字稿已保存，最终总结仍可正常生成。' });
          }
          console.log('   Transcripts:', freshTranscripts.length);
          console.log('   folder_path:', folderPath);

          // Refetch meetings and set current meeting
          await refetchMeetings();

          try {
            const meetingData = await storageService.getMeeting(meetingId);
            if (meetingData) {
              setCurrentMeeting({
                id: meetingId,
                title: meetingData.title
              });
              console.log('✅ Current meeting set:', meetingData.title);
            }
          } catch (error) {
            console.warn('Could not fetch meeting details, using ID only:', error);
            setCurrentMeeting({ id: meetingId, title: effectiveMeetingName });
          }

          // Confirm successful persistence without offering a route that could
          // bypass the mandatory password setup screen.
          toast.success('Recording saved successfully!', {
            description: `${freshTranscripts.length} transcript segments saved.`,
            duration: 5000,
          });
          // Track meeting completion analytics
          try {
            // Calculate meeting duration from transcript timestamps
            let durationSeconds = 0;
            if (freshTranscripts.length > 0 && freshTranscripts[0].audio_start_time !== undefined) {
              // Use audio_end_time of last transcript if available
              const lastTranscript = freshTranscripts[freshTranscripts.length - 1];
              durationSeconds = lastTranscript.audio_end_time || lastTranscript.audio_start_time || 0;
            }

            // Calculate word count
            const transcriptWordCount = freshTranscripts
              .map(t => t.text.split(/\s+/).length)
              .reduce((a, b) => a + b, 0);

            // Calculate words per minute
            const wordsPerMinute = durationSeconds > 0 ? transcriptWordCount / (durationSeconds / 60) : 0;

            // Get meetings count today
            const meetingsToday = await Analytics.getMeetingsCountToday();

            // Track meeting completed
            await Analytics.trackMeetingCompleted(meetingId, {
              duration_seconds: durationSeconds,
              transcript_segments: freshTranscripts.length,
              transcript_word_count: transcriptWordCount,
              words_per_minute: wordsPerMinute,
              meetings_today: meetingsToday
            });

            // Update meeting count in analytics.json
            await Analytics.updateMeetingCount();

            // Check for activation (first meeting)
            const { Store } = await import('@tauri-apps/plugin-store');
            const store = await Store.load('analytics.json');
            const totalMeetings = await store.get<number>('total_meetings');

            if (totalMeetings === 1) {
              const daysSinceInstall = await Analytics.calculateDaysSince('first_launch_date');
              await Analytics.track('user_activated', {
                meetings_count: '1',
                days_since_install: daysSinceInstall?.toString() || 'null',
                first_meeting_duration_seconds: durationSeconds.toString()
              });
            }
          } catch (analyticsError) {
            console.error('Failed to track meeting completion analytics:', analyticsError);
            // Don't block user flow on analytics errors
          }

        } catch (saveError) {
          console.error('Failed to save meeting to database:', saveError);
          setStatus(RecordingStatus.ERROR, saveError instanceof Error ? saveError.message : 'Unknown error');
          toast.error('Failed to save meeting', {
            description: saveError instanceof Error ? saveError.message : 'Unknown error'
          });
          throw saveError;
        }
      } else {
        // No save needed, go back to IDLE
        setStatus(RecordingStatus.IDLE);
      }

      setIsMeetingActive(false);
      // isRecording already set to false at function start
      setIsRecordingDisabled(false);
    } catch (error) {
      console.error('Error in handleRecordingStop:', error);
      setStatus(RecordingStatus.ERROR, error instanceof Error ? error.message : 'Unknown error');
      // isRecording already set to false at function start
      setIsRecordingDisabled(false);
    } finally {
      // Always reset the guard flag when done
      stopInProgressRef.current = false;
    }
  }, [
    setIsRecording,
    setIsRecordingDisabled,
    setStatus,
    transcriptsRef,
    flushBuffer,
    clearTranscripts,
    meetingTitle,
    markMeetingAsSaved,
    refetchMeetings,
    setCurrentMeeting,
    setMeetings,
    meetings,
    setIsMeetingActive,
    router,
  ]);

  // Expose handleRecordingStop function to window for Rust callbacks
  const handleRecordingStopRef = useRef(handleRecordingStop);
  useEffect(() => {
    handleRecordingStopRef.current = handleRecordingStop;
  });

  useEffect(() => {
    (window as any).handleRecordingStop = (callApi: boolean = true) => {
      handleRecordingStopRef.current(callApi);
    };

    // Cleanup on unmount
    return () => {
      delete (window as any).handleRecordingStop;
    };
  }, []);

  // Derive summaryStatus from RecordingStatus for backward compatibility
  const summaryStatus: SummaryStatus = status === RecordingStatus.PROCESSING_TRANSCRIPTS ? 'processing' : 'idle';

  return {
    handleRecordingStop,
    isStopping,
    isProcessingTranscript,
    isSavingTranscript,
    summaryStatus,
    setIsStopping: (value: boolean) => {
      setStatus(value ? RecordingStatus.STOPPING : RecordingStatus.IDLE);
    },
  };
}
