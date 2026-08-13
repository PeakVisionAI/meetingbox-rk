import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';

const AUDIO_CANDIDATES = [
  'audio.mp4', 'audio.m4a', 'audio.wav', 'audio.mp3', 'audio.flac',
  'audio.ogg', 'recording.mp4', 'audio.mkv', 'audio.webm', 'audio.wma',
];

function mimeType(path: string) {
  const extension = path.split('.').pop()?.toLowerCase();
  if (extension === 'wav') return 'audio/wav';
  if (extension === 'mp3') return 'audio/mpeg';
  if (extension === 'm4a' || extension === 'mp4') return 'audio/mp4';
  if (extension === 'ogg') return 'audio/ogg';
  if (extension === 'webm') return 'audio/webm';
  if (extension === 'flac') return 'audio/flac';
  return 'application/octet-stream';
}

export function useMeetingAudioPlayer(folderPath?: string | null) {
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
    if (!folderPath) return;
    let cancelled = false;
    setIsLoading(true);
    setError(null);

    const load = async () => {
      for (const name of AUDIO_CANDIDATES) {
        try {
          const path = `${folderPath.replace(/[\\/]$/, '')}/${name}`;
          const bytes = await invoke<number[]>('read_audio_file', { filePath: path });
          if (cancelled || !bytes?.length) continue;
          const url = URL.createObjectURL(new Blob([new Uint8Array(bytes)], { type: mimeType(path) }));
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
          audioRef.current = audio;
          objectUrlRef.current = url;
          setAudioPath(path);
          setIsLoading(false);
          return;
        } catch {
          // Try the next known recording name; missing candidates are expected.
        }
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
  }, [folderPath]);

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
