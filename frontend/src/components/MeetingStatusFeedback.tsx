'use client';

import { useCallback, useEffect, useState } from 'react';
import { AlertTriangle, CheckCircle2, CircleDashed, RefreshCw, Waves } from 'lucide-react';
import { recordingService } from '@/services/recordingService';
import { transcriptService } from '@/services/transcriptService';

type StatusKind = 'idle' | 'active' | 'processing' | 'error' | 'degraded';
type ServiceStatus = { kind: StatusKind; label: string; detail?: string };

interface MeetingStatusFeedbackProps {
  isRecording: boolean;
  isProcessing: boolean;
  summaryStatus?: 'idle' | 'processing' | 'completed' | 'error';
}

const idle: ServiceStatus = { kind: 'idle', label: '待机' };

export function MeetingStatusFeedback({ isRecording, isProcessing, summaryStatus = 'idle' }: MeetingStatusFeedbackProps) {
  const [vadActive, setVadActive] = useState(false);
  const [asr, setAsr] = useState<ServiceStatus>(idle);
  const [recording, setRecording] = useState<ServiceStatus>(idle);
  const [serviceError, setServiceError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      const state = await recordingService.getRecordingState();
      setRecording(state.is_recording ? { kind: 'active', label: '录音中' } : (isProcessing ? { kind: 'processing', label: '保存中' } : idle));
      const transcription = await transcriptService.getTranscriptionStatus();
      setAsr(transcription.is_processing ? { kind: 'processing', label: '转写中' } : (state.is_recording ? { kind: 'active', label: '已连接' } : idle));
      setServiceError(null);
    } catch (error) {
      setServiceError(String(error));
      setRecording({ kind: 'error', label: '服务异常', detail: '无法读取录音状态' });
      setAsr({ kind: 'degraded', label: '待重试', detail: 'ASR 状态不可用' });
    }
  }, [isProcessing]);

  useEffect(() => {
    void refresh();
    const timer = window.setInterval(() => void refresh(), 1500);
    return () => window.clearInterval(timer);
  }, [refresh]);

  useEffect(() => {
    let timer: number | undefined;
    const unlisten = recordingService.onSpeechDetected(() => {
      setVadActive(true);
      if (timer) window.clearTimeout(timer);
      timer = window.setTimeout(() => setVadActive(false), 1600);
    });
    return () => {
      if (timer) window.clearTimeout(timer);
      void unlisten.then((dispose) => dispose());
    };
  }, []);

  const speaker: ServiceStatus = isProcessing
    ? { kind: 'processing', label: '会后聚类' }
    : isRecording
      ? { kind: 'active', label: '候选发言人' }
      : idle;
  const summary: ServiceStatus = summaryStatus === 'processing'
    ? { kind: 'processing', label: '生成摘要' }
    : summaryStatus === 'completed'
      ? { kind: 'active', label: '摘要已更新' }
      : summaryStatus === 'error'
        ? { kind: 'error', label: '摘要失败', detail: '可在会后重试' }
        : idle;

  const items: Array<[string, ServiceStatus]> = [
    ['录音', recording],
    ['VAD', isRecording ? (vadActive ? { kind: 'active', label: '检测到人声' } : { kind: 'idle', label: '等待人声' }) : idle],
    ['ASR', asr],
    ['说话人', speaker],
    ['摘要', summary],
  ];

  return (
    <div className="hyos-status-feedback" aria-label="会议处理状态">
      <div className="hyos-status-items">
        {items.map(([name, status]) => (
          <span className={`hyos-status-item ${status.kind}`} key={name} title={status.detail || status.label}>
            <StatusIcon kind={status.kind} />
            <b>{name}</b>
            <small>{status.label}</small>
          </span>
        ))}
      </div>
      {serviceError && <div className="hyos-status-warning"><AlertTriangle size={14} /><span>本地服务状态读取失败，当前功能可能降级。</span><button type="button" onClick={() => void refresh()} aria-label="重试服务状态"><RefreshCw size={14} />重试</button></div>}
    </div>
  );
}

function StatusIcon({ kind }: { kind: StatusKind }) {
  if (kind === 'active') return <CheckCircle2 size={13} />;
  if (kind === 'processing') return <Waves size={13} />;
  if (kind === 'error' || kind === 'degraded') return <AlertTriangle size={13} />;
  return <CircleDashed size={13} />;
}
