'use client';

import { useEffect, useRef, useState } from 'react';
import { FileText, Play } from 'lucide-react';
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
import { VirtualizedTranscriptView } from '@/components/VirtualizedTranscriptView';
import { useTranscriptRecovery } from '@/hooks/useTranscriptRecovery';
import { TranscriptRecovery } from '@/components/TranscriptRecovery';
import { indexedDBService } from '@/services/indexedDBService';
import { useRecordingStateSync } from '@/hooks/useRecordingStateSync';
import { useTemplates } from '@/hooks/meeting-details/useTemplates';
import { formatMinuteRange, SUMMARY_INTERVAL_SECONDS, useMinuteSummaries } from '@/hooks/useMinuteSummaries';
import { MeetingStatusFeedback } from '@/components/MeetingStatusFeedback';

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
  const [quickMeetingTemplate, setQuickMeetingTemplate] = useState('standard_meeting');
  const [stopConfirmOpen, setStopConfirmOpen] = useState(false);
  const [showRecoveryDialog, setShowRecoveryDialog] = useState(false);
  const stopResolverRef = useRef<((confirmed: boolean) => void) | null>(null);

  const { handleRecordingStart } = useRecordingStart(isRecordingState, setIsRecordingState, undefined);
  const { handleRecordingStop, isStopping, setIsStopping } = useRecordingStop(setIsRecordingState, () => {});
  const { isRecordingDisabled } = useRecordingStateSync(isRecordingState, setIsRecordingState, setIsMeetingActive);
  const { recoverableMeetings, checkForRecoverableTranscripts, recoverMeeting, loadMeetingTranscripts, deleteRecoverableMeeting } = useTranscriptRecovery();
  const { availableTemplates: meetingTemplates } = useTemplates();
  const selectedTemplateName = meetingTemplates.find((template) => template.id === quickMeetingTemplate)?.name || '标准会议纪要';
  const { summaries: minuteSummaries, generatingMinute } = useMinuteSummaries({
    activeDuration: recordingState.activeDuration,
    isRecording: recordingState.isRecording || recordingState.isProcessing,
    transcripts,
    templateId: quickMeetingTemplate,
  });

  const liveSegments = transcripts.flatMap((item) => {
    const parts = item.text.split(/(?=(?:候选发言人|发言人)[^：:\s]{1,20}[：:])/).filter(Boolean);
    return parts.map((part, index) => {
      const match = part.match(/^((?:候选发言人|发言人)[^：:\s]{1,20})[：:]\s*([\s\S]*)$/);
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
  const speakerCountLabel = speakerCount > 0 ? `${speakerCount} 个` : (transcripts.length > 0 ? '待识别' : '0 个');

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
    try {
      sessionStorage.setItem('pending_summary_template_id', quickMeetingTemplate);
      sessionStorage.removeItem('pending_minute_summaries');
      await handleRecordingStart(name);
      setQuickMeetingOpen(false);
      setIsMeetingActive(true);
      toast.success('会议已开始', { description: '会议结束并保存后设置 6 位访问密码' });
    } catch (error) {
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
            <button className="hyos-launch primary" onClick={() => setQuickMeetingOpen(true)}><span><Play size={21} /></span><strong>快速会议</strong></button>
            <button className="hyos-launch" onClick={() => router.push('/files')}><span><FileText size={21} /></span><strong>会议文件管理</strong><small>查看和管理会议资料</small></button>
          </section>
          <section className="hyos-panel hyos-recent">
            <div className="hyos-panel-head"><h2>最近会议</h2><button onClick={() => router.push('/files')}>查看全部</button></div>
            {meetings.length === 0 ? <div className="hyos-empty-files">暂无会议文件</div> : meetings.slice(0, 3).map((meeting) => <button key={meeting.id} className="hyos-record-row" onClick={() => router.push(`/meeting-details?id=${meeting.id}`)}><span className="hyos-record-icon">会</span><span><b>{meeting.title}</b><small>本地会议文件</small></span><em>已完成</em></button>)}
          </section>
          <div className="hyos-device-strip"><div><b>设备状态</b><span><i />本地运行</span></div><div><span>本地模型服务</span><strong>已配置</strong></div><div><span>会议麦克风</span><strong>{selectedDevices.micDevice || '系统默认'}</strong></div><div><span>会议资料</span><strong>本地保存</strong></div></div>
        </>
      )}

      {showLive && (
        <section className="hyos-live-page">
          <div className="hyos-live-heading"><h1>{meetingTitle || '日常会议'}</h1></div>
          <div className="hyos-live-meta"><span className="hyos-live-record"><i />{recordingState.status === RecordingStatus.PROCESSING_TRANSCRIPTS ? '正在处理' : '正在录音'}</span><span>会议时间 <b>{new Date().toLocaleString('zh-CN', { hour12: false }).slice(0, 16)}</b></span><span>当前时长 <b>{formatClock(recordingState.activeDuration)}</b></span><span>总结模板 <b>{selectedTemplateName}</b></span><span>内容分段 <b>{transcripts.length} 段</b></span><span className="hyos-live-local">转写中</span></div>
          <MeetingStatusFeedback isRecording={recordingState.isRecording} isProcessing={recordingState.isProcessing || isStopping} summaryStatus={generatingMinute !== null ? 'processing' : 'idle'} />
          <div className="hyos-live-grid">
            <div className="hyos-transcript-pane"><div className="hyos-pane-title"><span>逐字稿</span><em>{transcripts.length} 段</em></div><div className="hyos-transcript-virtual"><VirtualizedTranscriptView segments={liveSegments} isRecording={recordingState.isRecording} isProcessing={recordingState.isProcessing} isStopping={recordingState.isStopping} enableStreaming={false} showConfidence={false} /></div></div>
            <aside className="hyos-summary-pane"><div className="hyos-pane-title"><span>会议分时摘要</span><em><i />每10分钟更新</em></div><div className="hyos-minute-summary-list">{minuteSummaries.length === 0 && generatingMinute === null ? <div className="hyos-summary-placeholder">首个10分钟结束后，这里会显示对应时段的内容摘要。</div> : minuteSummaries.map((summary) => <article className={`hyos-minute-summary ${summary.status || 'completed'}`} key={summary.minute_index}><div><time>{formatMinuteRange(summary.start_seconds, summary.end_seconds)}</time><span>{summary.status === 'empty' ? '无有效发言' : summary.status === 'error' ? '生成失败' : '已完成'}</span></div><p>{summary.summary || '本时段没有检测到有效发言。'}</p></article>)}{generatingMinute !== null && <div className="hyos-minute-generating"><i /><div><b>{formatMinuteRange(generatingMinute * SUMMARY_INTERVAL_SECONDS, (generatingMinute + 1) * SUMMARY_INTERVAL_SECONDS)}</b><span>正在生成本时段摘要</span></div></div>}</div></aside>
          </div>
          <div className="hyos-live-controls"><RecordingControls isRecording={recordingState.isRecording} onRecordingStop={(callApi = true, speakerCalibration) => handleRecordingStop(callApi, speakerCalibration)} onRecordingStart={() => startMeeting(meetingTitle)} onTranscriptReceived={() => {}} onStopInitiated={() => setIsStopping(true)} onBeforeStop={requestStopConfirmation} barHeights={['12px', '24px', '17px']} onTranscriptionError={(message) => toast.error(message)} isRecordingDisabled={isRecordingDisabled} isParentProcessing={isStopping} selectedDevices={selectedDevices} meetingName={meetingTitle} /></div>
        </section>
      )}

      {quickMeetingOpen && <div className="hyos-modal-backdrop" role="presentation"><div className="hyos-modal" role="dialog" aria-modal="true" aria-labelledby="quick-meeting-title"><button className="hyos-modal-close" onClick={() => setQuickMeetingOpen(false)} aria-label="关闭">×</button><h2 id="quick-meeting-title">快速会议</h2><p>填写会议信息并选择总结模板后开始记录。</p><label>会议名称<input value={quickMeetingName} onChange={(event) => setQuickMeetingName(event.target.value)} /></label><label>总结模板<select value={quickMeetingTemplate} onChange={(event) => setQuickMeetingTemplate(event.target.value)}>{meetingTemplates.map((template) => <option key={template.id} value={template.id}>{template.name}</option>)}</select></label><div className="hyos-form-tip">会议密码将在会议结束并保存成功后设置。</div><div className="hyos-modal-actions"><button className="hyos-secondary" onClick={() => setQuickMeetingOpen(false)}>取消</button><button className="hyos-primary" onClick={() => startMeeting(quickMeetingName)} disabled={!quickMeetingName.trim()}>开始会议</button></div></div></div>}
      {stopConfirmOpen && <div className="hyos-modal-backdrop" role="presentation"><div className="hyos-modal compact" role="dialog" aria-modal="true" aria-labelledby="stop-meeting-title"><h2 id="stop-meeting-title">确认结束会议？</h2><p>系统将保存录音、逐字稿和分时摘要，随后设置本次会议密码。</p><div className="hyos-modal-actions"><button className="hyos-secondary" onClick={() => resolveStopConfirmation(false)}>继续会议</button><button className="hyos-danger" onClick={() => resolveStopConfirmation(true)}>结束并保存</button></div></div></div>}
      <TranscriptRecovery isOpen={showRecoveryDialog} onClose={() => setShowRecoveryDialog(false)} recoverableMeetings={recoverableMeetings} onRecover={handleRecovery} onDelete={deleteRecoverableMeeting} onLoadPreview={loadMeetingTranscripts} />
    </div>
  );
}
