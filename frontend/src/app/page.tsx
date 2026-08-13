'use client';

import { useEffect, useRef, useState } from 'react';
import { FileText, LockKeyhole, Mic, Play, Settings, Wifi } from 'lucide-react';
import { useRouter } from 'next/navigation';
import { toast } from 'sonner';
import { useRecordingState, RecordingStatus } from '@/contexts/RecordingStateContext';
import { useTranscripts } from '@/contexts/TranscriptContext';
import { useConfig } from '@/contexts/ConfigContext';
import { useSidebar } from '@/components/Sidebar/SidebarProvider';
import { useRecordingStart } from '@/hooks/useRecordingStart';
import { useRecordingStop } from '@/hooks/useRecordingStop';
import { RecordingControls } from '@/components/RecordingControls';
import { useImportDialog } from '@/contexts/ImportDialogContext';
import { pendingMeetingAccess } from '@/services/pendingMeetingAccess';
import { VirtualizedTranscriptView } from '@/components/VirtualizedTranscriptView';
import { useTranscriptRecovery } from '@/hooks/useTranscriptRecovery';
import { TranscriptRecovery } from '@/components/TranscriptRecovery';
import { indexedDBService } from '@/services/indexedDBService';
import { useRecordingStateSync } from '@/hooks/useRecordingStateSync';

function formatClock(seconds: number | null) {
  const value = Math.max(0, Math.floor(seconds || 0));
  return `${String(Math.floor(value / 60)).padStart(2, '0')}:${String(value % 60).padStart(2, '0')}`;
}

export default function Home() {
  const router = useRouter();
  const { meetingTitle, transcripts } = useTranscripts();
  const { selectedDevices } = useConfig();
  const recordingState = useRecordingState();
  const { isMeetingActive, setIsMeetingActive, refetchMeetings, meetings } = useSidebar();
  const { openImportDialog } = useImportDialog();
  const [quickMeetingOpen, setQuickMeetingOpen] = useState(false);
  const [isRecordingState, setIsRecordingState] = useState(false);
  const [quickMeetingName, setQuickMeetingName] = useState('日常会议');
  const [quickMeetingPassword, setQuickMeetingPassword] = useState('');
  const [stopConfirmOpen, setStopConfirmOpen] = useState(false);
  const [showRecoveryDialog, setShowRecoveryDialog] = useState(false);
  const stopResolverRef = useRef<((confirmed: boolean) => void) | null>(null);

  const { handleRecordingStart } = useRecordingStart(isRecordingState, setIsRecordingState, undefined);
  const { handleRecordingStop, isStopping, setIsStopping } = useRecordingStop(setIsRecordingState, () => {});
  const { isRecordingDisabled } = useRecordingStateSync(isRecordingState, setIsRecordingState, setIsMeetingActive);
  const { recoverableMeetings, checkForRecoverableTranscripts, recoverMeeting, loadMeetingTranscripts, deleteRecoverableMeeting } = useTranscriptRecovery();

  const liveSegments = transcripts.flatMap((item) => {
    const parts = item.text.split(/(?=发言人[^：:\s]{1,20}[：:])/).filter(Boolean);
    return parts.map((part, index) => {
      const match = part.match(/^(发言人[^：:\s]{1,20})[：:]\s*([\s\S]*)$/);
      return {
        id: `${item.id}-${index}`,
        timestamp: item.audio_start_time ?? item.chunk_start_time ?? 0,
        endTime: item.audio_end_time,
        text: match ? match[2] : part,
        speakerLabel: match?.[1],
        confidence: item.confidence,
      };
    });
  });
  const speakerCount = new Set(liveSegments.map((item) => item.speakerLabel).filter(Boolean)).size;
  const speakerCountLabel = speakerCount > 0 ? `${speakerCount} 人` : (transcripts.length > 0 ? '待识别' : '0 人');

  const requestStopConfirmation = async () => new Promise<boolean>((resolve) => {
    stopResolverRef.current = resolve;
    setStopConfirmOpen(true);
  });

  const resolveStopConfirmation = (confirmed: boolean) => {
    stopResolverRef.current?.(confirmed);
    stopResolverRef.current = null;
    setStopConfirmOpen(false);
  };

  const startMeeting = async (name?: string) => {
    if (quickMeetingPassword && quickMeetingPassword.trim().length < 8) {
      toast.error('会议密码至少需要 8 个字符');
      return;
    }
    try {
      pendingMeetingAccess.set(quickMeetingPassword);
      await handleRecordingStart(name);
      setQuickMeetingOpen(false);
      setIsMeetingActive(true);
      toast.success('会议已开始', { description: quickMeetingPassword ? '本次会议密码已生效' : '将使用管理员密码访问文件' });
    } catch (error) {
      pendingMeetingAccess.clear();
      toast.error('无法开始会议', { description: error instanceof Error ? error.message : '请检查会议麦克风和本地模型服务' });
    }
  };

  useEffect(() => {
    if (recordingState.status === RecordingStatus.COMPLETED) {
      setIsMeetingActive(false);
      void refetchMeetings();
    }
  }, [recordingState.status, refetchMeetings, setIsMeetingActive]);

  useEffect(() => {
    if (recordingState.isRecording || recordingState.isProcessing || recordingState.isSaving) return;
    const checkRecovery = async () => {
      await indexedDBService.deleteOldMeetings(7).catch(() => {});
      await indexedDBService.deleteSavedMeetings(24).catch(() => {});
      await checkForRecoverableTranscripts();
    };
    void checkRecovery();
  }, [checkForRecoverableTranscripts, recordingState.isProcessing, recordingState.isRecording, recordingState.isSaving]);

  useEffect(() => {
    if (recoverableMeetings.length > 0 && !sessionStorage.getItem('recovery_dialog_shown')) {
      sessionStorage.setItem('recovery_dialog_shown', 'true');
      setShowRecoveryDialog(true);
    }
  }, [recoverableMeetings]);

  const handleRecovery = async (meetingId: string) => {
    const result = await recoverMeeting(meetingId);
    await refetchMeetings();
    if (result.meetingId) router.push(`/meeting-details?id=${result.meetingId}`);
    return result;
  };

  const showLive = recordingState.isRecording || recordingState.status === RecordingStatus.STOPPING || recordingState.status === RecordingStatus.PROCESSING_TRANSCRIPTS || recordingState.status === RecordingStatus.SAVING;

  return (
    <div className="hyos-page">
      {!showLive && (
        <>
          <section className="hyos-launcher" aria-label="会议工作台">
            <button className="hyos-launch primary" onClick={() => setQuickMeetingOpen(true)}><span><Play size={21} /></span><strong>快速会议</strong><small>开始会议记录</small></button>
            <button className="hyos-launch" onClick={() => router.push('/files')}><span><FileText size={21} /></span><strong>文件管理</strong><small>查看会议文件</small></button>
            <button className="hyos-launch" onClick={() => router.push('/settings')}><span><Settings size={21} /></span><strong>设置</strong><small>设备与版本管理</small></button>
          </section>
          <section className="hyos-home-grid">
            <div className="hyos-panel hyos-recent"><div className="hyos-panel-head"><h2>最近会议</h2><button onClick={() => router.push('/files')}>查看全部</button></div>
              {meetings.length === 0 ? <div className="hyos-empty-files">暂无会议文件</div> : meetings.slice(0, 2).map((meeting) => <button key={meeting.id} className="hyos-record-row" onClick={() => router.push(`/meeting-details?id=${meeting.id}`)}><span className="hyos-record-icon">会</span><span><b>{meeting.title}</b><small>本地会议文件</small></span><em>已完成</em></button>)}
            </div>
            <div className="hyos-security"><LockKeyhole size={21} /><b>资料仅保存在本机</b><p>会议密码用于界面访问控制，不等同于磁盘文件加密。</p><span><Wifi size={14} /> 本地服务正常</span></div>
          </section>
          <div className="hyos-device-strip"><div><b>设备状态</b><span><i />本地运行</span></div><div><span>本地模型服务</span><strong>已配置</strong></div><div><span>会议麦克风</span><strong>{selectedDevices.micDevice || '系统默认'}</strong></div><div><span>会议资料</span><strong>本地保存</strong></div></div>
        </>
      )}

      {showLive && (
        <section className="hyos-live-page">
          <div className="hyos-live-heading"><button className="hyos-back" onClick={() => router.push('/')}>← 返回工作台</button><h1>{meetingTitle || '日常会议'}</h1></div>
          <div className="hyos-live-meta"><span className="hyos-live-record"><i />{recordingState.status === RecordingStatus.PROCESSING_TRANSCRIPTS ? '正在处理' : '正在录音'}</span><span>会议时间 <b>{new Date().toLocaleString('zh-CN', { hour12: false }).slice(0, 16)}</b></span><span>当前时长 <b>{formatClock(recordingState.activeDuration)}</b></span><span>已识别发言人 <b>{speakerCountLabel}</b></span><span>内容分段 <b>{transcripts.length} 段</b></span><span className="hyos-live-local">转写中</span></div>
          <div className="hyos-live-grid">
            <div className="hyos-transcript-pane"><div className="hyos-pane-title"><span>逐字稿</span><em>{transcripts.length} 段</em></div><div className="hyos-transcript-virtual"><VirtualizedTranscriptView segments={liveSegments} isRecording={recordingState.isRecording} isProcessing={recordingState.isProcessing} isStopping={recordingState.isStopping} enableStreaming={false} showConfidence={false} /></div></div>
            <aside className="hyos-summary-pane"><div className="hyos-pane-title"><span>会议摘要</span><em>结束后生成</em></div><div className="hyos-summary-placeholder">结束会议后，可选择基于完整逐字稿生成 AI 会议总结。</div></aside>
          </div>
          <div className="hyos-live-controls"><RecordingControls isRecording={recordingState.isRecording} onRecordingStop={(callApi = true) => handleRecordingStop(callApi)} onRecordingStart={() => startMeeting(meetingTitle)} onTranscriptReceived={() => {}} onStopInitiated={() => setIsStopping(true)} onBeforeStop={requestStopConfirmation} barHeights={['12px', '24px', '17px']} onTranscriptionError={(message) => toast.error(message)} isRecordingDisabled={isRecordingDisabled} isParentProcessing={isStopping} selectedDevices={selectedDevices} meetingName={meetingTitle} /></div>
        </section>
      )}

      {quickMeetingOpen && <div className="hyos-modal-backdrop" role="presentation"><div className="hyos-modal" role="dialog" aria-modal="true" aria-labelledby="quick-meeting-title"><button className="hyos-modal-close" onClick={() => setQuickMeetingOpen(false)} aria-label="关闭">×</button><h2 id="quick-meeting-title">快速会议</h2><p>填写会议信息后开始现场录音与转写。</p><label>会议名称<input value={quickMeetingName} onChange={(event) => setQuickMeetingName(event.target.value)} /></label><label>会议密码<input type="password" value={quickMeetingPassword} onChange={(event) => setQuickMeetingPassword(event.target.value)} placeholder="不填写则使用管理员密码" /></label><div className="hyos-form-tip">独立会议密码至少 8 个字符；不填写时使用管理员密码。</div><div className="hyos-modal-actions"><button className="hyos-secondary" onClick={() => setQuickMeetingOpen(false)}>取消</button><button className="hyos-primary" onClick={() => startMeeting(quickMeetingName)} disabled={!quickMeetingName.trim()}>开始会议</button></div></div></div>}
      {stopConfirmOpen && <div className="hyos-modal-backdrop" role="presentation"><div className="hyos-modal compact" role="dialog" aria-modal="true" aria-labelledby="stop-meeting-title"><h2 id="stop-meeting-title">确认结束会议？</h2><p>结束后将保存录音与逐字稿，并进入会议详情。</p><div className="hyos-modal-actions"><button className="hyos-secondary" onClick={() => resolveStopConfirmation(false)}>继续会议</button><button className="hyos-danger" onClick={() => resolveStopConfirmation(true)}>结束并保存</button></div></div></div>}
      <TranscriptRecovery isOpen={showRecoveryDialog} onClose={() => setShowRecoveryDialog(false)} recoverableMeetings={recoverableMeetings} onRecover={handleRecovery} onDelete={deleteRecoverableMeeting} onLoadPreview={loadMeetingTranscripts} />
    </div>
  );
}
