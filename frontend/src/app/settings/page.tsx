'use client';

import { useState } from 'react';
import { ArrowLeft, KeyRound, Mic, ShieldAlert, Sparkles, Captions } from 'lucide-react';
import { useRouter } from 'next/navigation';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import { useConfig } from '@/contexts/ConfigContext';
import { accessControlService } from '@/services/accessControlService';
import { RecordingSettings } from '@/components/RecordingSettings';
import { TranscriptSettings, TranscriptModelProps } from '@/components/TranscriptSettings';
import { SummaryModelSettings } from '@/components/SummaryModelSettings';
import { SummaryTemplateSettings } from '@/components/SummaryTemplateSettings';
import { HotwordSettings } from '@/components/HotwordSettings';

type SettingsTab = 'recording' | 'transcription' | 'summary' | 'account';

export default function SettingsPage() {
  const router = useRouter();
  const { transcriptModelConfig, setTranscriptModelConfig } = useConfig();
  const [tab, setTab] = useState<SettingsTab>('recording');
  const [passwordOpen, setPasswordOpen] = useState(false);
  const [currentPassword, setCurrentPassword] = useState('');
  const [newPassword, setNewPassword] = useState('');
  const [confirmPassword, setConfirmPassword] = useState('');
  const [savingTranscript, setSavingTranscript] = useState(false);

  const saveTranscriptConfig = async (config: TranscriptModelProps) => {
    setTranscriptModelConfig(config);
    setSavingTranscript(true);
    try {
      await invoke('api_save_transcript_config', {
        provider: config.provider,
        model: config.model,
        apiKey: config.apiKey ?? null,
      });
      toast.success('转写配置已保存');
    } catch (error) {
      toast.error('无法保存转写配置', { description: String(error) });
    } finally {
      setSavingTranscript(false);
    }
  };

  const changePassword = async () => {
    if (newPassword !== confirmPassword) return toast.error('两次输入的新密码不一致');
    if (!/^\d{6}$/.test(newPassword)) return toast.error('管理员密码必须是 6 位数字');
    try {
      await accessControlService.changeAdminPassword(currentPassword, newPassword);
      setPasswordOpen(false);
      setCurrentPassword('');
      setNewPassword('');
      setConfirmPassword('');
      toast.success('管理员密码已修改');
    } catch (error) {
      toast.error('修改失败', { description: String(error) });
    }
  };

  return (
    <div className="hyos-page hyos-settings-page">
      <div className="hyos-page-title"><div><button className="hyos-back" onClick={() => router.push('/')}><ArrowLeft size={16} />返回工作台</button><h1>设置</h1><p>配置会议设备、语音转写、摘要模型与管理员账户。</p></div></div>
      <div className="hyos-settings-tabs" role="tablist">
        <button className={tab === 'recording' ? 'active' : ''} onClick={() => setTab('recording')}><Mic size={16} />录音与设备</button>
        <button className={tab === 'transcription' ? 'active' : ''} onClick={() => setTab('transcription')}><Captions size={16} />语音转写</button>
        <button className={tab === 'summary' ? 'active' : ''} onClick={() => setTab('summary')}><Sparkles size={16} />会议摘要</button>
        <button className={tab === 'account' ? 'active' : ''} onClick={() => setTab('account')}><KeyRound size={16} />管理员账户</button>
      </div>
      <section className="hyos-panel hyos-settings-workspace">
        {tab === 'recording' && <RecordingSettings />}
        {tab === 'transcription' && <div className="hyos-summary-settings-stack"><div><TranscriptSettings transcriptModelConfig={transcriptModelConfig} setTranscriptModelConfig={saveTranscriptConfig} /><div className="hyos-form-tip">{savingTranscript ? '正在保存转写配置…' : '选择变更后会立即保存到本机。'}</div></div><HotwordSettings /></div>}
        {tab === 'summary' && <div className="hyos-summary-settings-stack"><SummaryModelSettings /><SummaryTemplateSettings /></div>}
        {tab === 'account' && <div className="hyos-account-settings"><div className="hyos-account"><span>管</span><div><b>管理员</b><small>admin · 唯一管理员账户</small></div><button className="hyos-secondary" onClick={() => setPasswordOpen(true)}><KeyRound size={15} />修改密码</button></div><div className="hyos-warning hyos-settings-warning"><ShieldAlert size={18} /><span><b>请妥善保管管理员密码</b><small>管理员密码无法找回。遗忘后需刷写设备，已有数据可能丢失。</small></span></div></div>}
      </section>
      {passwordOpen && <div className="hyos-modal-backdrop"><div className="hyos-modal compact" role="dialog" aria-modal="true" aria-labelledby="change-password-title"><button className="hyos-modal-close" onClick={() => setPasswordOpen(false)} aria-label="关闭">×</button><h2 id="change-password-title">修改管理员密码</h2><p>修改后请立即妥善保存，新密码无法找回。</p><label>当前密码<input type="password" value={currentPassword} onChange={(event) => setCurrentPassword(event.target.value)} /></label><label>新密码<input type="password" inputMode="numeric" maxLength={6} value={newPassword} onChange={(event) => setNewPassword(event.target.value.replace(/\D/g, '').slice(0, 6))} /></label><label>再次输入新密码<input type="password" inputMode="numeric" maxLength={6} value={confirmPassword} onChange={(event) => setConfirmPassword(event.target.value.replace(/\D/g, '').slice(0, 6))} /></label><div className="hyos-form-tip">管理员密码必须是 6 位数字。</div><div className="hyos-modal-actions"><button className="hyos-secondary" onClick={() => setPasswordOpen(false)}>取消</button><button className="hyos-primary" onClick={changePassword} disabled={!currentPassword || !newPassword || !confirmPassword}>确认修改</button></div></div></div>}
    </div>
  );
}
