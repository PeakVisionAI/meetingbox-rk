'use client';

import { Mic, MicOff } from 'lucide-react';
import { Switch } from '@/components/ui/switch';
import type { useVoiceWake } from '@/hooks/useVoiceWake';

type VoiceWakeControlProps = ReturnType<typeof useVoiceWake>;

export function VoiceWakeControl({ status, pending, listening, awakened, setEnabled, prepareFeedback }: VoiceWakeControlProps) {
  let label = '语音唤醒：连接中';
  if (status) {
    if (status.state === 'unavailable' || status.state === 'error') label = '语音唤醒：不可用';
    else if (!status.enabled || status.state === 'disabled') label = '语音唤醒：已关闭';
    else if (awakened) label = '正在聆听会议指令…';
    else if (listening) label = `等待唤醒${status.wake_word ? `“${status.wake_word}”` : ''}`;
    else if (status.state === 'paused') label = '语音唤醒：已暂停';
    else label = '语音唤醒：准备中';
  }

  const description = awakened ? '已唤醒，请说“开始会议”' :
    status?.detail || (listening ? '唤醒后说“开始会议”即可开始；会议进行时暂停语音唤醒' : label);

  return (
    <div className="flex min-w-0 items-center gap-2 rounded-md border border-[#d9e2ee] bg-white px-3 py-1.5 text-xs text-[#52647a]" title={description} onPointerDown={prepareFeedback}>
      {listening ? <Mic size={15} className="shrink-0 text-[#2369ef]" aria-hidden="true" /> : <MicOff size={15} className="shrink-0" aria-hidden="true" />}
      <span id="voice-wake-label" role="status" aria-live="polite" className="max-w-48 truncate">{label}</span>
      <Switch
        aria-label="启用语音唤醒"
        aria-describedby="voice-wake-label"
        checked={status?.enabled ?? false}
        disabled={!status || pending}
        onCheckedChange={(enabled) => void setEnabled(enabled)}
        className="data-[state=checked]:bg-[#2369ef] data-[state=unchecked]:bg-[#cbd5e1]"
      />
    </div>
  );
}
