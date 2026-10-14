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
}

interface VoiceWakeEvent {
  type: 'wake' | 'start' | 'timeout' | 'cancel';
}

const PREFERENCE_KEY = 'voice-wake-enabled';
const PROMPT_ID = 'voice-wake-prompt';
const WAKE_PROMPT = '已唤醒，请说“开始会议”';

function readPreference(): boolean | null {
  try {
    const saved = localStorage.getItem(PREFERENCE_KEY);
    return saved === 'true' ? true : saved === 'false' ? false : null;
  } catch {
    return null;
  }
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
  const audioRef = useRef<AudioContext | null>(null);
  recordingActiveRef.current = recordingActive;

  const clearPrompt = useCallback(() => {
    setAwakened(false);
    toast.dismiss(PROMPT_ID);
  }, []);

  const applyStatus = useCallback((next: VoiceWakeStatus) => {
    statusRevisionRef.current += 1;
    statusRef.current = next;
    setStatus(next);
    if (!next.enabled || next.state !== 'awaiting_command' || recordingActiveRef.current) {
      clearPrompt();
    }
  }, [clearPrompt]);

  const prepareFeedback = useCallback(() => {
    try {
      const AudioContextClass = window.AudioContext || (window as Window & {
        webkitAudioContext?: typeof AudioContext;
      }).webkitAudioContext;
      if (!AudioContextClass) return;
      const context = audioRef.current ?? new AudioContextClass();
      audioRef.current = context;
      if (context.state === 'suspended') void context.resume().catch(() => {});
    } catch {
      // Feedback is optional in webviews without usable WebAudio.
    }
  }, []);

  const playFeedback = useCallback(() => {
    prepareFeedback();
    const context = audioRef.current;
    if (!context || context.state !== 'running') return;
    try {
      const oscillator = context.createOscillator();
      const gain = context.createGain();
      const now = context.currentTime;
      oscillator.type = 'sine';
      oscillator.frequency.setValueAtTime(660, now);
      oscillator.frequency.setValueAtTime(880, now + 0.09);
      gain.gain.setValueAtTime(0, now);
      gain.gain.linearRampToValueAtTime(0.08, now + 0.015);
      gain.gain.linearRampToValueAtTime(0, now + 0.18);
      oscillator.connect(gain);
      gain.connect(context.destination);
      oscillator.onended = () => {
        oscillator.disconnect();
        gain.disconnect();
      };
      oscillator.start(now);
      oscillator.stop(now + 0.2);
    } catch {
      // A blocked chime must not prevent the wake prompt.
    }
  }, [prepareFeedback]);

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
            if (payload.type === 'wake') {
              const current = statusRef.current;
              if (recordingActiveRef.current || !current?.enabled ||
                  !['listening', 'awaiting_command'].includes(current.state)) return;
              setAwakened(true);
              toast.info(WAKE_PROMPT, { id: PROMPT_ID, description: '正在聆听会议指令…', duration: Infinity });
              playFeedback();
            } else {
              // Native sends request-start-recording after confirming the command.
              // The existing MainContent bridge is the only recording start path.
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
      const context = audioRef.current;
      audioRef.current = null;
      if (context && context.state !== 'closed') void context.close().catch(() => {});
    };
  }, [applyStatus, clearPrompt, playFeedback, prepareFeedback]);

  useEffect(() => {
    if (recordingActive) clearPrompt();
  }, [recordingActive, clearPrompt]);

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

  const listening = !recordingActive && !!status?.enabled &&
    (status.state === 'listening' || status.state === 'awaiting_command');

  return { status, pending, listening, awakened: listening && (awakened || status?.state === 'awaiting_command'), setEnabled, prepareFeedback };
}
