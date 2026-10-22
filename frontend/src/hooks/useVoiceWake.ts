'use client';

import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { toast } from 'sonner';

export interface VoiceWakeStatus {
  enabled: boolean;
  state: string;
  detail?: string;
  wake_word: string;
  /** Which command set native has armed: idle starts, meeting ends, transition
   *  is a start/stop handoff where commands are suppressed. */
  context?: string;
}

interface VoiceWakeEvent {
  type: 'wake' | 'start' | 'stop' | 'timeout' | 'cancel';
  detail?: string;
  wake_word?: string;
  /** Same values as VoiceWakeStatus.context; wake events are idle or meeting. */
  context?: string;
}

const PREFERENCE_KEY = 'voice-wake-enabled';
const PROMPT_ID = 'voice-wake-prompt';
const WAKE_PROMPT = '已唤醒，请说“开始会议”';
const MEETING_WAKE_PROMPT = '已唤醒，请说“结束会议”';

type WakeContext = 'idle' | 'meeting' | 'transition';

function resolveContext(context: string | undefined, recordingActive: boolean): WakeContext {
  if (context === 'idle' || context === 'meeting' || context === 'transition') return context;
  // Older native builds did not send context; the recording flag is the fallback.
  return recordingActive ? 'meeting' : 'idle';
}

function readPreference(): boolean | null {
  try {
    const saved = localStorage.getItem(PREFERENCE_KEY);
    return saved === 'true' ? true : saved === 'false' ? false : null;
  } catch {
    return null;
  }
}

export interface FeedbackController {
  /** Resume from a user gesture synchronously, then suspend again once no
   *  chime is playing. */
  unlock(): void;
  /** Resume if needed, then play the wake chime. */
  play(): void;
  /** Close the context; work that lands after teardown plays nothing. */
  dispose(): void;
}

// Owns the optional wake-chime AudioContext. The context is only resumed while
// a chime is playing (or briefly on a user gesture), so an idle app does not
// keep the WebAudio device running. Exported for tests.
export function createFeedbackController(): FeedbackController {
  let context: AudioContext | null = null;
  let activeChimes = 0;
  let disposed = false;
  // Serializes resume/play/suspend so an idle suspend can never land between a
  // resume and the chime it was started for.
  let queue: Promise<void> = Promise.resolve();
  // The latest resume attempt while it is still settling; release waits for it
  // so a suspend check can never race past an in-flight resume.
  let resumeInFlight: Promise<void> | null = null;

  const enqueue = (task: () => Promise<void>) => {
    queue = queue.then(task).catch(() => {});
  };

  const createContext = (): AudioContext | null => {
    try {
      const AudioContextClass = window.AudioContext || (window as Window & {
        webkitAudioContext?: typeof AudioContext;
      }).webkitAudioContext;
      return AudioContextClass ? new AudioContextClass() : null;
    } catch {
      // Feedback is optional in webviews without usable WebAudio.
      return null;
    }
  };

  // Calls resume synchronously; WebKit only grants playback for resumes that
  // happen while the user gesture is still active.
  const startResume = (audio: AudioContext): Promise<void> => {
    const attempt = audio.resume().then(() => {}, () => {});
    resumeInFlight = attempt;
    void attempt.then(() => {
      if (resumeInFlight === attempt) resumeInFlight = null;
    });
    return attempt;
  };

  // Reads the state through a helper so TypeScript cannot keep a narrowing
  // from before an await; the context can be resumed or closed concurrently.
  const contextState = (audio: AudioContext): AudioContextState => audio.state;

  const acquire = async (): Promise<AudioContext | null> => {
    if (disposed) return null;
    if (!context || contextState(context) === 'closed') {
      context = createContext();
      if (!context) return null;
    }
    if (contextState(context) !== 'running') {
      if (resumeInFlight) await resumeInFlight;
      if (disposed || !context) return null;
      if (contextState(context) !== 'running') await startResume(context);
    }
    if (disposed || !context || contextState(context) !== 'running') return null;
    return context;
  };

  // Suspend once the last overlapping chime has ended.
  const release = async () => {
    if (disposed || activeChimes > 0 || !context) return;
    const pending = resumeInFlight;
    if (pending) await pending;
    if (disposed || activeChimes > 0 || !context || context.state !== 'running') return;
    try {
      await context.suspend();
    } catch {
      // A failed suspend leaves the device running but never breaks the prompt.
    }
  };

  return {
    unlock() {
      if (disposed) return;
      if (!context || context.state === 'closed') {
        context = createContext();
        if (!context) return;
      }
      if (context.state !== 'running') startResume(context);
      enqueue(release);
    },
    play() {
      enqueue(async () => {
        const audio = await acquire();
        if (!audio) return;
        let oscillator: OscillatorNode | null = null;
        let gain: GainNode | null = null;
        let counted = false;
        let finished = false;
        // Counts the chime out exactly once, even when start/stop throw, so a
        // failed chime can never leave the context resumed with stuck nodes.
        const finishChime = () => {
          if (finished) return;
          finished = true;
          if (counted) activeChimes -= 1;
          if (oscillator) {
            try { oscillator.disconnect(); } catch { /* already detached */ }
          }
          if (gain) {
            try { gain.disconnect(); } catch { /* already detached */ }
          }
          enqueue(release);
        };
        try {
          oscillator = audio.createOscillator();
          gain = audio.createGain();
          const now = audio.currentTime;
          oscillator.type = 'sine';
          oscillator.frequency.setValueAtTime(660, now);
          oscillator.frequency.setValueAtTime(880, now + 0.09);
          gain.gain.setValueAtTime(0, now);
          gain.gain.linearRampToValueAtTime(0.08, now + 0.015);
          gain.gain.linearRampToValueAtTime(0, now + 0.18);
          oscillator.connect(gain);
          gain.connect(audio.destination);
          activeChimes += 1;
          counted = true;
          oscillator.onended = finishChime;
          oscillator.start(now);
          oscillator.stop(now + 0.2);
        } catch {
          // A blocked chime must not prevent the wake prompt; the count, the
          // nodes, and any resume are all released through the same path.
          finishChime();
        }
      });
    },
    dispose() {
      disposed = true;
      const audio = context;
      context = null;
      if (audio && audio.state !== 'closed') void audio.close().catch(() => {});
    },
  };
}

// Mount once in MainContent so route changes do not restart native subscriptions.
export function useVoiceWake(recordingActive: boolean) {
  const [status, setStatus] = useState<VoiceWakeStatus | null>(null);
  const [pending, setPending] = useState(false);
  const [awakened, setAwakened] = useState(false);
  const statusRef = useRef(status);
  const recordingActiveRef = useRef(recordingActive);
  const pendingRef = useRef(false);
  const lifecycleRef = useRef<object | null>(null);
  const statusRevisionRef = useRef(0);
  const feedbackRef = useRef<FeedbackController | null>(null);
  recordingActiveRef.current = recordingActive;

  const clearPrompt = useCallback(() => {
    setAwakened(false);
    toast.dismiss(PROMPT_ID);
  }, []);

  const applyStatus = useCallback((next: VoiceWakeStatus) => {
    statusRevisionRef.current += 1;
    statusRef.current = next;
    setStatus(next);
    if (!next.enabled || next.state !== 'awaiting_command') {
      clearPrompt();
    }
  }, [clearPrompt]);

  const getFeedback = useCallback(() => {
    if (!feedbackRef.current) feedbackRef.current = createFeedbackController();
    return feedbackRef.current;
  }, []);

  // Unlock WebAudio on the first normal interaction; it suspends again while idle.
  const prepareFeedback = useCallback(() => {
    getFeedback().unlock();
  }, [getFeedback]);

  const playFeedback = useCallback(() => {
    getFeedback().play();
  }, [getFeedback]);

  useEffect(() => {
    const lifecycle = {};
    lifecycleRef.current = lifecycle;
    let cancelled = false;
    pendingRef.current = true;
    setPending(true);
    const unlisteners: UnlistenFn[] = [];
    const retainListener = (unlisten: UnlistenFn) => {
      if (cancelled) unlisten();
      else unlisteners.push(unlisten);
    };

    const setup = async () => {
      try {
        await Promise.all([
          listen<VoiceWakeStatus>('voice-wake-status', ({ payload }) => {
            if (!cancelled) applyStatus(payload);
          }).then(retainListener),
          listen<VoiceWakeEvent>('voice-wake-event', ({ payload }) => {
            if (cancelled) return;
            const context = resolveContext(payload.context, recordingActiveRef.current);
            if (payload.type === 'wake') {
              const current = statusRef.current;
              if (!current?.enabled || !['listening', 'awaiting_command', 'meeting'].includes(current.state)) return;
              if (context === 'transition') return;
              setAwakened(true);
              if (context === 'meeting') {
                // Silence during meetings, including the wake chime: the webview
                // outputs are captured by the recording pipeline.
                toast.info(MEETING_WAKE_PROMPT, {
                  id: PROMPT_ID,
                  description: '正在聆听结束指令…',
                  duration: Infinity,
                });
                return;
              }
              toast.info(WAKE_PROMPT, {
                id: PROMPT_ID,
                description: '正在聆听会议指令…',
                duration: Infinity,
              });
              playFeedback();
            } else {
              // Native sends request-start-recording / request-stop-recording
              // after confirming the command; the existing bridges own the action.
              clearPrompt();
            }
          }).then(retainListener),
        ]);
        if (cancelled) return;
        const revision = statusRevisionRef.current;
        const initial = await invoke<VoiceWakeStatus>('get_voice_wake_status');
        if (cancelled) return;
        if (revision === statusRevisionRef.current) applyStatus(initial);
        const preference = readPreference();
        // With no explicit user choice, preserve the native deployment default.
        if (preference !== null && preference !== statusRef.current?.enabled) {
          const preferenceRevision = statusRevisionRef.current;
          const next = await invoke<VoiceWakeStatus>('set_voice_wake_enabled', { enabled: preference });
          if (!cancelled && preferenceRevision === statusRevisionRef.current) applyStatus(next);
        }
      } catch (error) {
        if (cancelled) return;
        console.warn('[VoiceWake] Service unavailable:', error);
        applyStatus({ enabled: statusRef.current?.enabled ?? false, state: 'unavailable', detail: '语音唤醒服务不可用', wake_word: statusRef.current?.wake_word ?? '' });
      } finally {
        if (!cancelled) {
          pendingRef.current = false;
          setPending(false);
        }
      }
    };

    // Unlock feedback on the first normal interaction, before a hands-free wake.
    window.addEventListener('pointerdown', prepareFeedback);
    window.addEventListener('keydown', prepareFeedback);
    void setup();
    return () => {
      cancelled = true;
      window.removeEventListener('pointerdown', prepareFeedback);
      window.removeEventListener('keydown', prepareFeedback);
      if (lifecycleRef.current === lifecycle) lifecycleRef.current = null;
      unlisteners.forEach(unlisten => unlisten());
      toast.dismiss(PROMPT_ID);
      const feedback = feedbackRef.current;
      feedbackRef.current = null;
      feedback?.dispose();
    };
  }, [applyStatus, clearPrompt, playFeedback, prepareFeedback, getFeedback]);

  const context = resolveContext(status?.context, recordingActive);

  useEffect(() => {
    // The meeting stop prompt only exists while the meeting command set is
    // armed. Leaving meeting mode clears it; idle mode clears any non-awaiting
    // status through applyStatus, and keeps an awaiting start prompt.
    if (context === 'meeting') return;
    setAwakened(false);
    if (statusRef.current?.state !== 'awaiting_command') clearPrompt();
  }, [context, clearPrompt]);

  const setEnabled = useCallback(async (enabled: boolean) => {
    const lifecycle = lifecycleRef.current;
    if (!lifecycle || pendingRef.current) return;
    pendingRef.current = true;
    setPending(true);
    prepareFeedback();
    try {
      const revision = statusRevisionRef.current;
      const next = await invoke<VoiceWakeStatus>('set_voice_wake_enabled', { enabled });
      if (lifecycleRef.current !== lifecycle) return;
      if (revision === statusRevisionRef.current) applyStatus(next);
      try {
        localStorage.setItem(PREFERENCE_KEY, String(next.enabled));
      } catch {
        // Native remains authoritative when browser storage is unavailable.
      }
    } catch (error) {
      if (lifecycleRef.current === lifecycle) {
        toast.error('无法更改语音唤醒设置', { description: String(error) });
      }
    } finally {
      if (lifecycleRef.current === lifecycle) {
        pendingRef.current = false;
        setPending(false);
      }
    }
  }, [applyStatus, prepareFeedback]);

  // A running meeting keeps the microphone armed for the stop command, so the
  // control stays live while recording instead of reporting a paused listener.
  const listening = !!status?.enabled &&
    (status.state === 'listening' || status.state === 'awaiting_command' || status.state === 'meeting');

  return { status, pending, listening, awakened: listening && (awakened || status?.state === 'awaiting_command'), context, setEnabled, prepareFeedback };
}
