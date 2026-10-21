'use client';

import { invoke } from '@tauri-apps/api/core';
import { appDataDir } from '@tauri-apps/api/path';
import { useCallback, useEffect, useState, useRef } from 'react';
import { Play, Square, Pause, Mic, AlertCircle, X } from 'lucide-react';
import { ProcessRequest, SummaryResponse } from '@/types/summary';
import { listen } from '@tauri-apps/api/event';
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert"
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from '@/components/ui/tooltip';
import Analytics from '@/lib/analytics';
import { uiText } from '@/lib/uiText';
import type { SpeakerCalibrationResult, StopRecordingResult } from '@/services/recordingService';
import { recordingService } from '@/services/recordingService';

// Exact native rejection for a voice stop whose session is no longer active.
const STALE_SESSION_ERROR = '语音结束请求对应的会议已变更';

// Result of a stop_recording invoke. Stale/no-recording both mean native refused
// the stop and the existing recording context must be left untouched.
type StopOutcome = 'stopped' | 'stale-session' | 'no-recording' | 'failed';

interface RecordingControlsProps {
  isRecording: boolean;
  barHeights: string[];
  onRecordingStop: (callApi?: boolean, speakerCalibration?: SpeakerCalibrationResult) => void;
  onRecordingStart: () => void;
  onTranscriptReceived: (summary: SummaryResponse) => void;
  onTranscriptionError?: (message: string) => void;
  onStopInitiated?: () => void; // Called immediately when stop button is clicked
  /**
   * Called when a stop that already announced itself was refused by native
   * (stale voice session, or nothing recording anymore) so the parent can drop
   * its stopping state. The current recording context is left intact.
   */
  onStopRejected?: () => void;
  onBeforeStop?: () => Promise<boolean>;
  isRecordingDisabled: boolean;
  isParentProcessing: boolean;
  selectedDevices?: {
    micDevice: string | null;
    systemDevice: string | null;
  };
  meetingName?: string;
  recordingDuration?: string;
  isPaused?: boolean;
  hasSpeech?: boolean;
}

export const RecordingControls: React.FC<RecordingControlsProps> = ({
  isRecording,
  barHeights,
  onRecordingStop,
  onRecordingStart,
  onTranscriptReceived,
  onTranscriptionError,
  onStopInitiated,
  onStopRejected,
  onBeforeStop,
  isRecordingDisabled,
  isParentProcessing,
  selectedDevices,
  meetingName,
  recordingDuration = '00:00',
  isPaused = false,
  hasSpeech = false,
}) => {
  const [showPlayback, setShowPlayback] = useState(false);
  const [recordingPath, setRecordingPath] = useState<string | null>(null);
  const [transcript, setTranscript] = useState<string>('');
  const [isProcessing, setIsProcessing] = useState(false);
  const [isStarting, setIsStarting] = useState(false);
  const [isStopping, setIsStopping] = useState(false);
  const MIN_RECORDING_DURATION = 2000; // 2 seconds minimum recording time
  const [transcriptionErrors, setTranscriptionErrors] = useState(0);
  const [isValidatingModel, setIsValidatingModel] = useState(false);
  const [deviceError, setDeviceError] = useState<{ title: string, message: string } | null>(null);
  const isWaveformActive = isRecording && !isPaused && hasSpeech;
  // One stop flow at a time. The ref also covers the manual confirmation dialog
  // while it is awaiting the user, so a duplicate voice command can never start
  // a second stop in parallel.
  const stopFlowRef = useRef(false);

  const togglePauseRecording = useCallback(async () => {
    if (!isRecording || isStopping || isProcessing || isParentProcessing) return;
    try {
      if (isPaused) {
        await recordingService.resumeRecording();
      } else {
        await recordingService.pauseRecording();
      }
    } catch (error) {
      onTranscriptionError?.(isPaused ? '无法继续录音，请重试' : '无法暂停录音，请重试');
    }
  }, [isPaused, isParentProcessing, isProcessing, isRecording, isStopping, onTranscriptionError]);

  const currentTime = 0;
  const duration = 0;
  const isPlaying = false;
  const progress = 0;

  const formatTime = (time: number) => {
    const minutes = Math.floor(time / 60);
    const seconds = Math.floor(time % 60);
    return `${minutes}:${seconds.toString().padStart(2, '0')}`;
  };

  useEffect(() => {
    const checkTauri = async () => {
      try {
        const result = await invoke('is_recording');
        console.log('Tauri is initialized and ready, is_recording result:', result);
      } catch (error) {
        console.error('Tauri initialization error:', error);
        alert('Failed to initialize recording. Please check the console for details.');
      }
    };
    checkTauri();
  }, []);

  const handleStartRecording = useCallback(async () => {
    if (isStarting || isValidatingModel) return;
    console.log('Starting recording...');
    console.log('Selected devices:', selectedDevices);
    console.log('Meeting name:', meetingName);
    console.log('Current isRecording state:', isRecording);

    setShowPlayback(false);
    setTranscript(''); // Clear any previous transcript
    try {
      // Call the validation callback which will:
      // 1. Check if model is ready
      // 2. Show appropriate toast/modal
      // 3. Call backend if valid
      // 4. Update UI state
      await onRecordingStart();
    } catch (error) {
      console.error('Failed to start recording:', error);
      console.error('Error details:', {
        message: error instanceof Error ? error.message : String(error),
        name: error instanceof Error ? error.name : 'Unknown',
        stack: error instanceof Error ? error.stack : undefined
      });

      // Parse error message to provide user-friendly feedback
      const errorMsg = error instanceof Error ? error.message : String(error);

      // Check for device-related errors
      if (errorMsg.includes('microphone') || errorMsg.includes('mic') || errorMsg.includes('input')) {
        setDeviceError({
          title: 'Microphone Not Available',
          message: 'Unable to access your microphone. Please check that:\n• Your microphone is connected\n• The app has microphone permissions\n• No other app is using the microphone'
        });
      } else if (errorMsg.includes('system audio') || errorMsg.includes('speaker') || errorMsg.includes('output')) {
        setDeviceError({
          title: 'System Audio Not Available',
          message: 'Unable to capture system audio. Please check that:\n• A virtual audio device (like BlackHole) is installed\n• The app has screen recording permissions (macOS)\n• System audio is properly configured'
        });
      } else if (errorMsg.includes('permission')) {
        setDeviceError({
          title: 'Permission Required',
          message: 'Recording permissions are required. Please:\n• Grant microphone access in System Settings\n• Grant screen recording access for system audio (macOS)\n• Restart the app after granting permissions'
        });
      } else {
        setDeviceError({
          title: 'Recording Failed',
          message: 'Unable to start recording. Please check your audio device settings and try again.'
        });
      }
    }
  }, [onRecordingStart, isStarting, isValidatingModel, selectedDevices, meetingName, isRecording]);

  const stopRecordingAction = useCallback(async (sessionId?: string): Promise<StopOutcome> => {
    console.log('Executing stop recording...', sessionId ? { session_id: sessionId } : '');
    try {
      setIsProcessing(true);
      const dataDir = await appDataDir();
      const timestamp = new Date().toISOString().replace(/[:.]/g, '-');
      const savePath = `${dataDir}/recording-${timestamp}.wav`;
      console.log('Saving recording to:', savePath);
      console.log('About to call stop_recording command');
      const result = await invoke<StopRecordingResult>('stop_recording', {
        args: {
          save_path: savePath,
          // Native rejects the stop atomically when this session is no longer
          // the active recording, so a stale voice command cannot end another.
          ...(sessionId ? { session_id: sessionId } : {}),
        }
      });
      console.log('stop_recording command completed successfully:', result);
      setRecordingPath(savePath);
      // setShowPlayback(true);
      setIsProcessing(false);
      // Track successful transcription
      Analytics.trackTranscriptionSuccess();
      onRecordingStop(true, result.speaker_calibration);
      return 'stopped';
    } catch (error) {
      console.error('Failed to stop recording:', error);
      const errorText = error instanceof Error ? error.message : typeof error === 'string' ? error : String(error);
      if (errorText.includes(STALE_SESSION_ERROR)) {
        // The voice command targeted a session that already ended. There is no
        // save flow to run and the current recording (if any) must stay intact.
        console.warn('Ignoring stale voice stop request:', errorText);
        setIsProcessing(false);
        return 'stale-session';
      }
      if (errorText.includes('No recording in progress')) {
        setIsProcessing(false);
        return 'no-recording';
      }
      setIsProcessing(false);
      onRecordingStop(false);
      return 'failed';
    } finally {
      setIsStopping(false);
    }
  }, [onRecordingStop]);

  const handleStopRecording = useCallback(async (sessionId?: string) => {
    console.log('handleStopRecording called - isRecording:', isRecording, 'isStarting:', isStarting, 'isStopping:', isStopping);
    if (!isRecording || isStarting || isStopping || stopFlowRef.current) {
      console.log('Early return from handleStopRecording due to state check');
      return;
    }

    // Claim the flow before any await so a duplicate stop (manual or voice)
    // cannot enter while an earlier one is still waiting for the user.
    stopFlowRef.current = true;
    try {
      if (sessionId) {
        // A voice command was already confirmed by the speaker and must end the
        // meeting directly, but only when native confirms the requested session
        // is still the active recording.
        try {
          const nativeState = await recordingService.getRecordingState();
          if (!nativeState.is_recording || nativeState.session_id !== sessionId) {
            console.warn('Voice stop session no longer active; skipping stop');
            return;
          }
        } catch (error) {
          console.warn('Could not verify voice stop session:', error);
          return;
        }
      } else if (onBeforeStop && !(await onBeforeStop())) return;

      console.log('Stopping recording...');

      // Notify the parent immediately (for UI state updates). onStopRejected
      // below undoes this if native refuses the stop after this point.
      const stopAnnounced = !!onStopInitiated;
      onStopInitiated?.();

      setIsStopping(true);

      // Immediately trigger the stop action
      const outcome = await stopRecordingAction(sessionId);
      if (stopAnnounced && (outcome === 'stale-session' || outcome === 'no-recording')) {
        // Native refused an announced stop (the session ended atomically, or
        // nothing was recording anymore). Drop the parent's stopping state so
        // the UI stays usable; the recording context itself is untouched.
        onStopRejected?.();
      }
    } finally {
      stopFlowRef.current = false;
    }
  }, [isRecording, isStarting, isStopping, stopRecordingAction, onStopInitiated, onStopRejected, onBeforeStop]);

  useEffect(() => {
    return () => {
      // Cleanup on unmount if needed
    };
  }, []);

  useEffect(() => {
    console.log('Setting up recording event listeners');
    let unsubscribes: (() => void)[] = [];

    const setupListeners = async () => {
      try {
        // Transcript error listener - handles both regular and actionable errors
        const transcriptErrorUnsubscribe = await listen('transcript-error', (event) => {
          console.log('transcript-error event received:', event);
          console.error('Transcription error received:', event.payload);
          const errorMessage = event.payload as string;

          Analytics.trackTranscriptionError(errorMessage);
          console.log('Tracked transcription error:', errorMessage);

          setTranscriptionErrors(prev => {
            const newCount = prev + 1;
            console.log('Transcription error count incremented:', newCount);
            return newCount;
          });
          setIsProcessing(false);
          console.log('Calling onRecordingStop(false) due to transcript error');
          onRecordingStop(false);
          if (onTranscriptionError) {
            onTranscriptionError(errorMessage);
          }
        });

        // Transcription error listener - handles structured error objects with actionable flag
        const transcriptionErrorUnsubscribe = await listen('transcription-error', (event) => {
          console.log('transcription-error event received:', event);
          console.error('Transcription error received:', event.payload);

          let errorMessage: string;
          let isActionable = false;

          if (typeof event.payload === 'object' && event.payload !== null) {
            const payload = event.payload as { error: string, userMessage: string, actionable: boolean };
            errorMessage = payload.userMessage || payload.error;
            isActionable = payload.actionable || false;
          } else {
            errorMessage = String(event.payload);
          }

          Analytics.trackTranscriptionError(errorMessage);
          console.log('Tracked transcription error:', errorMessage);

          setTranscriptionErrors(prev => {
            const newCount = prev + 1;
            console.log('Transcription error count incremented:', newCount);
            return newCount;
          });
          setIsProcessing(false);
          if (isActionable) {
            console.log('Calling onRecordingStop(false) due to actionable transcription error');
            onRecordingStop(false);
          }

          // For actionable errors (like model loading failures), the main page will handle showing the model selector
          // For regular errors, they are handled by useModalState global listener which shows a toast
          // We don't want to show a modal (via onTranscriptionError) AND a toast, so we skip the callback here
          /* if (onTranscriptionError && !isActionable) {
            onTranscriptionError(errorMessage);
          } */
        });

        // Pause/Resume events are now handled by RecordingStateContext
        // No need for duplicate listeners here

        unsubscribes = [
          transcriptErrorUnsubscribe,
          transcriptionErrorUnsubscribe
        ];
        console.log('Recording event listeners set up successfully');
      } catch (error) {
        console.error('Failed to set up recording event listeners:', error);
      }
    };

    setupListeners();

    return () => {
      console.log('Cleaning up recording event listeners');
      unsubscribes.forEach(unsubscribe => {
        if (unsubscribe && typeof unsubscribe === 'function') {
          unsubscribe();
        }
      });
    };
  }, [onRecordingStop, onTranscriptionError]);

  // Single owner of window-level stop requests. Both entry points route through
  // handleStopRecording so the duplicate guard covers manual and voice stops.
  useEffect(() => {
    const handleHeaderStop = () => {
      void handleStopRecording();
    };
    // A voice stop carries the native session id, so a stale route action can
    // never end a meeting it did not target.
    const handleVoiceStop = (event: Event) => {
      const detail = (event as CustomEvent<{ session_id?: string }>).detail;
      const sessionId = detail?.session_id;
      if (!sessionId) return;
      void handleStopRecording(sessionId);
    };
    window.addEventListener('end-meeting-from-header', handleHeaderStop);
    window.addEventListener('end-meeting-by-voice', handleVoiceStop);
    return () => {
      window.removeEventListener('end-meeting-from-header', handleHeaderStop);
      window.removeEventListener('end-meeting-by-voice', handleVoiceStop);
    };
  }, [handleStopRecording]);

  // Announce bridge readiness after the stop listener above is registered, so a
  // route handoff only replays a pending voice stop once this component owns the
  // window event. The parent also probes once on mount for the reverse order.
  useEffect(() => {
    const announce = () => {
      window.dispatchEvent(new CustomEvent('recording-controls-ready', { detail: true }));
    };
    announce();
    window.addEventListener('request-recording-controls-ready', announce);
    return () => {
      window.removeEventListener('request-recording-controls-ready', announce);
      window.dispatchEvent(new CustomEvent('recording-controls-ready', { detail: false }));
    };
  }, []);

  return (
    <TooltipProvider>
      <div className="flex flex-col space-y-2">
        <div className="flex items-center space-x-2 bg-white rounded-full shadow-lg px-4 py-2">
          {isProcessing && !isParentProcessing ? (
            <div className="flex items-center space-x-2">
              <div className="animate-spin rounded-full h-5 w-5 border-b-2 border-gray-900"></div>
              <span className="text-sm text-gray-600">正在处理录音…</span>
            </div>
          ) : (
            <>
              {showPlayback ? (
                <>
                  <button
                    onClick={handleStartRecording}
                    className="w-10 h-10 flex items-center justify-center bg-red-500 rounded-full text-white hover:bg-red-600 transition-colors"
                  >
                    <Mic size={16} />
                  </button>

                  <div className="w-px h-6 bg-gray-200 mx-1" />

                  <div className="flex items-center space-x-1 mx-2">
                    <div className="text-sm text-gray-600 min-w-[40px]">
                      {formatTime(currentTime)}
                    </div>
                    <div
                      className="relative w-24 h-1 bg-gray-200 rounded-full"
                    >
                      <div
                        className="absolute h-full bg-blue-500 rounded-full"
                        style={{ width: `${progress}%` }}
                      />
                    </div>
                    <div className="text-sm text-gray-600 min-w-[40px]">
                      {formatTime(duration)}
                    </div>
                  </div>

                  <button
                    className="w-10 h-10 flex items-center justify-center bg-gray-300 rounded-full text-white cursor-not-allowed"
                    disabled
                  >
                    <Play size={16} />
                  </button>
                </>
              ) : (
                <>
                  {!isRecording ? (
                    // Start recording button
                    <Tooltip>
                      <TooltipTrigger asChild>
                        <button
                          onClick={() => {
                            Analytics.trackButtonClick('start_recording', 'recording_controls');
                            handleStartRecording();
                          }}
                          disabled={isStarting || isProcessing || isRecordingDisabled || isValidatingModel}
                          className={`w-12 h-12 flex items-center justify-center ${isStarting || isProcessing || isValidatingModel ? 'bg-gray-400' : 'bg-red-500 hover:bg-red-600'
                            } rounded-full text-white transition-colors relative`}
                        >
                          {isValidatingModel ? (
                            <div className="animate-spin rounded-full h-5 w-5 border-b-2 border-white"></div>
                          ) : (
                            <Mic size={20} />
                          )}
                        </button>
                      </TooltipTrigger>
                      <TooltipContent>
                        <p>{uiText.startRecording}</p>
                      </TooltipContent>
                    </Tooltip>
                  ) : (
                    // Current product flow supports ending the meeting only.
                    <>
                      <Tooltip>
                        <TooltipTrigger asChild>
                          <button
                            onClick={() => {
                              Analytics.trackButtonClick('pause_recording', 'recording_controls');
                              void togglePauseRecording();
                            }}
                            disabled={isStopping}
                            className={`w-10 h-10 flex items-center justify-center ${isStopping ? 'bg-gray-400' : 'bg-blue-600 hover:bg-blue-700'
                              } rounded-full text-white transition-colors relative`}
                          >
                            {isPaused ? <Play size={17} /> : <Pause size={17} />}
                            {isStopping && (
                              <div className="absolute -top-8 text-gray-600 font-medium text-xs">
                                Stopping...
                              </div>
                            )}
                          </button>
                        </TooltipTrigger>
                        <TooltipContent>
                        <p>{isPaused ? uiText.resumeRecording : uiText.pauseRecording}</p>
                        </TooltipContent>
                      </Tooltip>
                    </>
                  )}

                  <div
                    className={`hyos-control-waveform flex items-center space-x-1 mx-4${isWaveformActive ? ' is-active' : ''}`}
                    aria-hidden="true"
                  >
                    {barHeights.map((height, index) => (
                      <div
                        key={index}
                        className="w-1 rounded-full transition-all duration-200 bg-red-500"
                        style={{
                          height: isWaveformActive ? height : '4px',
                          animationDelay: `${index * -70}ms`,
                          opacity: 1,
                        }}
                      />
                    ))}
                  </div>
                </>
              )}
            </>
          )}
        </div>

        {/* Show validation status only */}
        {isValidatingModel && (
          <div className="text-xs text-gray-600 text-center mt-2">
            Validating speech recognition...
          </div>
        )}

        {/* Device error alert */}
        {deviceError && (
          <Alert variant="destructive" className="mt-4 border-red-300 bg-red-50">
            <AlertCircle className="h-5 w-5 text-red-600" />
            <button
              onClick={() => setDeviceError(null)}
              className="absolute right-3 top-3 text-red-600 hover:text-red-800 transition-colors"
              aria-label="Close alert"
            >
              <X className="h-4 w-4" />
            </button>
            <AlertTitle className="text-red-800 font-semibold mb-2">
              {deviceError.title}
            </AlertTitle>
            <AlertDescription className="text-red-700">
              {deviceError.message.split('\n').map((line, i) => (
                <div key={i} className={i > 0 ? 'ml-2' : ''}>
                  {line}
                </div>
              ))}
            </AlertDescription>
          </Alert>
        )}

        {/* {showPlayback && recordingPath && (
        <div className="text-sm text-gray-600 px-4">
          Recording saved to: {recordingPath}
        </div>
      )} */}
      </div>
    </TooltipProvider>
  );
};
