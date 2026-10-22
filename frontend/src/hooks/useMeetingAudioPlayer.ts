import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { readFile } from '@tauri-apps/plugin-fs';

function audioMimeType(path: string) {
  const extension = path.split('.').pop()?.toLowerCase();
  if (extension === 'wav') return 'audio/wav';
  if (extension === 'ogg') return 'audio/ogg';
  return 'audio/mpeg';
}

export function useMeetingAudioPlayer(meetingId?: string | null) {
  const audioRef = useRef<HTMLAudioElement | null>(null);
  const objectUrlRef = useRef<string | null>(null);
  const endTimeRef = useRef<number | null>(null);
  const loadPromiseRef = useRef<Promise<HTMLAudioElement> | null>(null);
  const aliveRef = useRef(true);
  const [audioPath, setAudioPath] = useState<string | null>(null);
  const [isLoading, setIsLoading] = useState(false);
  const [isPlaying, setIsPlaying] = useState(false);
  const [currentTime, setCurrentTime] = useState(0);
  const [duration, setDuration] = useState(0);
  const [error, setError] = useState<string | null>(null);

  const detachAudio = useCallback(() => {
    audioRef.current?.pause();
    audioRef.current = null;
    loadPromiseRef.current = null;
    if (objectUrlRef.current) {
      URL.revokeObjectURL(objectUrlRef.current);
      objectUrlRef.current = null;
    }
    setAudioPath(null);
    setIsPlaying(false);
    setCurrentTime(0);
    setDuration(0);
  }, []);

  const attachAudio = useCallback(async (path: string) => {
    const bytes = await readFile(path);
    const data = bytes.buffer.slice(
      bytes.byteOffset,
      bytes.byteOffset + bytes.byteLength,
    ) as ArrayBuffer;
    const url = URL.createObjectURL(new Blob([data], { type: audioMimeType(path) }));
    objectUrlRef.current = url;
    const audio = new Audio(url);
    audio.preload = 'metadata';
    audio.addEventListener('loadedmetadata', () => {
      setDuration(Number.isFinite(audio.duration) ? audio.duration : 0);
    });
    audio.addEventListener('timeupdate', () => {
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
      setError(`录音无法解码（${audio.error?.code ?? 'unknown'}）`);
    });
    audioRef.current = audio;
    setAudioPath(path);
    audio.load();
    return audio;
  }, []);

  const ensureAudio = useCallback(async () => {
    if (audioRef.current) return audioRef.current;
    if (loadPromiseRef.current) return loadPromiseRef.current;
    if (!meetingId) {
      throw new Error('未选择会议');
    }

    const load = (async () => {
      setIsLoading(true);
      setError(null);
      try {
        const path = await Promise.race([
          invoke<string>('get_meeting_audio_path', { meetingId }),
          new Promise<never>((_, reject) => {
            window.setTimeout(() => reject(new Error('准备录音播放文件超时，请稍后重试')), 120_000);
          }),
        ]);
        if (!aliveRef.current) {
          void invoke('release_meeting_audio_path', { meetingId }).catch(() => {});
          throw new Error('录音播放已取消');
        }
        if (!path) {
          throw new Error('未找到本次会议的录音文件');
        }
        return attachAudio(path);
      } catch (loadError) {
        console.error('Failed to prepare meeting audio:', loadError);
        setError(loadError instanceof Error ? loadError.message : '无法准备本次会议的录音');
        throw loadError;
      } finally {
        setIsLoading(false);
        loadPromiseRef.current = null;
      }
    })();
    loadPromiseRef.current = load;
    return load;
  }, [attachAudio, meetingId]);

  useEffect(() => {
    aliveRef.current = true;
    return () => {
      aliveRef.current = false;
      detachAudio();
      if (meetingId) {
        void invoke('release_meeting_audio_path', { meetingId }).catch(() => {});
      }
    };
  }, [detachAudio, meetingId]);

  const playSegment = useCallback(async (start: number, end?: number) => {
    try {
      const audio = await ensureAudio();
      const safeStart = Math.max(0, start || 0);
      endTimeRef.current = end !== undefined && end > safeStart ? end : null;
      if (Math.abs(audio.currentTime - safeStart) > 0.25) audio.currentTime = safeStart;
      await audio.play();
    } catch (playError) {
      console.error('Failed to play meeting audio segment:', playError);
      setError(playError instanceof Error ? playError.message : '录音播放失败');
    }
  }, [ensureAudio]);

  const toggle = useCallback(async () => {
    try {
      const audio = await ensureAudio();
      endTimeRef.current = null;
      if (audio.paused) await audio.play();
      else audio.pause();
    } catch (playError) {
      console.error('Failed to toggle meeting audio:', playError);
      setError(playError instanceof Error ? playError.message : '录音播放失败');
    }
  }, [ensureAudio]);

  const seek = useCallback((value: number) => {
    const audio = audioRef.current;
    if (!audio) return;
    endTimeRef.current = null;
    audio.currentTime = Math.max(0, Math.min(value, duration || audio.duration || value));
    setCurrentTime(audio.currentTime);
  }, [duration]);

  return { audioPath, isLoading, isPlaying, currentTime, duration, error, toggle, seek, playSegment };
}
