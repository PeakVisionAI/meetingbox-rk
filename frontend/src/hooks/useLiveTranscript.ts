'use client';

import { useEffect, useRef, useState } from 'react';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { recordingService } from '@/services/recordingService';

export interface LiveTranscriptPreview {
  session_id: string;
  text: string;
  start_time: number;
  end_time: number;
  is_final: boolean;
}

/** UI-facing streaming availability derived from native session status. */
type LiveTranscriptStatus = 'listening' | 'unavailable' | 'error';

type NativeStreamingStatus = 'connecting' | 'healthy' | 'degraded';

interface LiveTranscriptStatusPayload {
  session_id: string;
  status: NativeStreamingStatus;
  detail?: string;
}

type LiveEvent =
  | { kind: 'preview'; payload: LiveTranscriptPreview }
  | { kind: 'status'; payload: LiveTranscriptStatusPayload };

interface UseLiveTranscriptOptions {
  recordingActive: boolean;
  /** Greatest confirmed audio_end_time in this recording, or -1 if none. */
  maxEnd: number;
}

/** Ephemeral native previews only; transcript-update remains authoritative. */
export function useLiveTranscript({ recordingActive, maxEnd }: UseLiveTranscriptOptions) {
  const [preview, setPreview] = useState<LiveTranscriptPreview | null>(null);
  const [status, setStatus] = useState<LiveTranscriptStatus | null>(null);
  const confirmedEndRef = useRef(maxEnd);
  const recordingActiveRef = useRef(recordingActive);
  const syncRecordingRef = useRef<((active: boolean) => void) | null>(null);

  useEffect(() => {
    confirmedEndRef.current = maxEnd;
    setPreview((current) => current && maxEnd >= current.end_time ? null : current);
  }, [maxEnd]);

  useEffect(() => {
    let disposed = false;
    let stopped = !recordingActiveRef.current;
    let stoppedByEvent = false;
    let generation = 0;
    let sessionId: string | null = null;
    let sessionLookup: Promise<void> | null = null;
    let latestPreview: LiveTranscriptPreview | null = null;
    const unlisteners: UnlistenFn[] = [];
    // Keep only the newest event of each kind while native identity is resolving.
    // This queue is bounded to two entries and preserves their arrival order.
    const pendingEvents: LiveEvent[] = [];

    const clearPreview = () => {
      latestPreview = null;
      pendingEvents.length = 0;
      setPreview(null);
      setStatus(null);
    };

    const resolveSession = () => {
      if (disposed || stopped || sessionId || sessionLookup) return;
      const requestGeneration = generation;
      sessionLookup = recordingService.getRecordingState()
        .then((state) => {
          if (disposed || stopped || requestGeneration !== generation) return;
          if (!state.is_recording) {
            stopped = true;
            clearPreview();
            return;
          }
          if (!state.session_id) {
            setStatus('unavailable');
            return;
          }
          sessionId = state.session_id;
          pendingEvents.splice(0).forEach(receiveEvent);
        })
        .catch((error) => {
          if (disposed || stopped || requestGeneration !== generation) return;
          console.warn('[useLiveTranscript] Could not resolve recording session', error);
          setStatus('error');
        })
        .finally(() => {
          if (requestGeneration === generation) sessionLookup = null;
        });
    };

    function receiveEvent(incoming: LiveEvent) {
      if (disposed) return;
      const payload = incoming.payload;
      if (!payload || typeof payload.session_id !== 'string' || !payload.session_id) return;
      // A status can be emitted while the native session is still starting, so it
      // is queued like previews. Previews after a stop are always late arrivals.
      if (incoming.kind === 'preview' && stopped && stoppedByEvent && !recordingActiveRef.current) return;

      if (!sessionId) {
        const previous = pendingEvents.findIndex((event) => event.kind === incoming.kind);
        if (previous !== -1) pendingEvents.splice(previous, 1);
        pendingEvents.push(incoming);
        resolveSession();
        return;
      }
      // Never infer native identity from a preview: an old worker can emit late.
      if (payload.session_id !== sessionId) return;

      if (incoming.kind === 'status') {
        const { status: nativeStatus } = incoming.payload;
        if (nativeStatus === 'degraded') setStatus('unavailable');
        else if (nativeStatus === 'connecting' || nativeStatus === 'healthy') setStatus('listening');
        return;
      }

      if (stopped) return;
      const next = incoming.payload;
      if (typeof next.text !== 'string' || typeof next.is_final !== 'boolean'
        || !Number.isFinite(next.start_time) || !Number.isFinite(next.end_time)
        || next.start_time < 0 || next.end_time < next.start_time) return;

      if (!next.text.trim()) {
        // Empty non-final event is the native streaming-fault signal: drop the
        // ephemeral row, but only when it reaches the current preview position,
        // so an out-of-order fault cannot erase newer speech.
        if (!next.is_final && latestPreview && next.end_time >= latestPreview.end_time - 0.01) {
          latestPreview = null;
          setPreview(null);
        }
        return;
      }

      if (confirmedEndRef.current >= next.end_time) return;

      if (latestPreview) {
        // Cumulative hypotheses replace the row; older hypotheses cannot rewind it.
        if (next.end_time < latestPreview.end_time
          || next.start_time < latestPreview.start_time) return;
        // A late non-final that stays inside a finalized utterance must not
        // resurrect it. A resume that merely overlaps the silence-final end but
        // extends past it is new speech and is accepted.
        if (latestPreview.is_final && !next.is_final
          && next.start_time < latestPreview.end_time - 0.01
          && next.end_time <= latestPreview.end_time) return;
        const sameUtterance = next.start_time === latestPreview.start_time;
        if (sameUtterance && next.end_time === latestPreview.end_time
          && next.text === latestPreview.text && next.is_final === latestPreview.is_final) return;
      }

      latestPreview = next;
      setPreview(next);
      setStatus('listening');
      // is_final only closes the preview utterance. Keep its text through silence
      // until confirmed audio coverage catches up, without writing to persistence.
    }

    const resetSession = () => {
      generation += 1;
      sessionId = null;
      sessionLookup = null;
      clearPreview();
    };

    const syncRecording = (active: boolean) => {
      recordingActiveRef.current = active;
      if (!active) {
        stopped = true;
        resetSession();
      } else if (stopped && !stoppedByEvent) {
        stopped = false;
        resolveSession();
      }
    };
    syncRecordingRef.current = syncRecording;

    const register = async (registration: Promise<UnlistenFn>) => {
      try {
        const unlisten = await registration;
        if (disposed) unlisten();
        else unlisteners.push(unlisten);
      } catch (error) {
        if (disposed || stopped) return;
        console.warn('[useLiveTranscript] Could not subscribe to live preview', error);
        setStatus('error');
      }
    };

    void Promise.all([
      register(listen<LiveTranscriptPreview>('live-transcript', (event) => {
        receiveEvent({ kind: 'preview', payload: event.payload });
      })),
      register(listen<LiveTranscriptStatusPayload>('live-transcript-status', (event) => {
        receiveEvent({ kind: 'status', payload: event.payload });
      })),
      register(recordingService.onRecordingStarted(() => {
        if (disposed) return;
        stopped = false;
        stoppedByEvent = false;
        resetSession();
        resolveSession();
      })),
      register(recordingService.onRecordingStopped(() => {
        if (disposed) return;
        stopped = true;
        stoppedByEvent = true;
        resetSession();
      })),
    ]).then(resolveSession);

    return () => {
      disposed = true;
      stopped = true;
      generation += 1;
      pendingEvents.length = 0;
      if (syncRecordingRef.current === syncRecording) syncRecordingRef.current = null;
      unlisteners.forEach((unlisten) => unlisten());
    };
  }, []);

  useEffect(() => {
    syncRecordingRef.current?.(recordingActive);
  }, [recordingActive]);

  // Hide covered/stopped previews in the same render as their authoritative update.
  return {
    preview: recordingActive && preview && maxEnd < preview.end_time ? preview : null,
    status: recordingActive ? status : null,
  };
}
