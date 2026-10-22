'use client';

import { Mic, MicOff } from 'lucide-react';
import { Switch } from '@/components/ui/switch';
import type { useVoiceWake } from '@/hooks/useVoiceWake';

type VoiceWakeControlProps = ReturnType<typeof useVoiceWake>;

export function VoiceWakeControl({ status, pending, listening, awakened, context, setEnabled, prepareFeedback }: VoiceWakeControlProps) {
  let label = '语音唤醒：连接中';
  if (status) {
    if (status.state === 'unavailable' || status.state === 'error') label = '语音唤醒：不可用';
    else if (!status.enabled || status.state === 'disabled') label = '语音唤醒：已关闭';
    else if (context === 'meeting') label = status.state === 'awaiting_command' ? '正在聆听结束指令…' : '会议中，可说“结束会议”';
    else if (context === 'transition') label = '语音唤醒：处理中';
    else if (awakened) label = '正在聆听会议指令…';
    else if (listening) label = `等待唤醒${status.wake_word ? `“${status.wake_word}”` : ''}`;
    else label = '语音唤醒：准备中';
  }

  const description = context === 'transition' ? '会议录制或保存中，语音指令暂不可用' :
    context === 'meeting' && awakened ? '已唤醒，请说“结束会议”' :
    awakened ? '已唤醒，请说“开始会议”' :
    status?.detail || (listening ? '唤醒后说“开始会议”即可开始；会议中可说“结束会议”' : label);

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
