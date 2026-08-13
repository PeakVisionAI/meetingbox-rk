import { useCallback, useEffect, useRef, useState } from 'react';
import { convertFileSrc, invoke } from '@tauri-apps/api/core';

export function useMeetingAudioPlayer(meetingId?: string | null) {
  const audioRef = useRef<HTMLAudioElement | null>(null);
  const objectUrlRef = useRef<string | null>(null);
  const endTimeRef = useRef<number | null>(null);
  const [audioPath, setAudioPath] = useState<string | null>(null);
  const [isLoading, setIsLoading] = useState(false);
  const [isPlaying, setIsPlaying] = useState(false);
  const [currentTime, setCurrentTime] = useState(0);
  const [duration, setDuration] = useState(0);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!meetingId) return;
    let cancelled = false;
    setIsLoading(true);
    setError(null);

    const load = async () => {
      try {
          const path = await invoke<string>('get_meeting_audio_path', { meetingId });
          if (cancelled || !path) return;
          const url = convertFileSrc(path);
          const audio = new Audio(url);
          audio.preload = 'metadata';
          audio.addEventListener('loadedmetadata', () => {
            if (!cancelled) setDuration(Number.isFinite(audio.duration) ? audio.duration : 0);
          });
          audio.addEventListener('timeupdate', () => {
            if (cancelled) return;
            setCurrentTime(audio.currentTime);
            const end = endTimeRef.current;
            if (end !== null && audio.currentTime >= end) {
              audio.pause();
              audio.currentTime = end;
              endTimeRef.current = null;
              setIsPlaying(false);
            }
          });
          audio.addEventListener('play', () => setIsPlaying(true));
          audio.addEventListener('pause', () => setIsPlaying(false));
          audio.addEventListener('ended', () => {
            setIsPlaying(false);
            setCurrentTime(0);
            endTimeRef.current = null;
          });
          audio.addEventListener('error', () => {
            if (!cancelled) setError(`录音无法解码（${audio.error?.code ?? 'unknown'}）`);
          });
          audioRef.current = audio;
          setAudioPath(path);
          setIsLoading(false);
          return;
      } catch (loadError) {
        console.error('Failed to prepare meeting audio:', loadError);
      }
      if (!cancelled) {
        setError('未找到本次会议的录音文件');
        setIsLoading(false);
      }
    };

    load();
    return () => {
      cancelled = true;
      audioRef.current?.pause();
      audioRef.current = null;
      if (objectUrlRef.current) URL.revokeObjectURL(objectUrlRef.current);
      objectUrlRef.current = null;
      setAudioPath(null);
    };
  }, [meetingId]);

  const playSegment = useCallback(async (start: number, end?: number) => {
    const audio = audioRef.current;
    if (!audio) return;
    const safeStart = Math.max(0, start || 0);
    endTimeRef.current = end !== undefined && end > safeStart ? end : null;
    if (Math.abs(audio.currentTime - safeStart) > 0.25) audio.currentTime = safeStart;
    await audio.play();
  }, []);

  const toggle = useCallback(async () => {
    const audio = audioRef.current;
    if (!audio) return;
    endTimeRef.current = null;
    if (audio.paused) await audio.play();
    else audio.pause();
  }, []);

  const seek = useCallback((value: number) => {
    const audio = audioRef.current;
    if (!audio) return;
    endTimeRef.current = null;
    audio.currentTime = Math.max(0, Math.min(value, duration || audio.duration || value));
    setCurrentTime(audio.currentTime);
  }, [duration]);

  return { audioPath, isLoading, isPlaying, currentTime, duration, error, toggle, seek, playSegment };
}
