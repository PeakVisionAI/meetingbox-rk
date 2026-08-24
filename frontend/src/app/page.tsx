'use client';

import { useEffect, useRef, useState } from 'react';
import { ChevronRight } from 'lucide-react';
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

function formatClock(seconds: number | null) {
  const value = Math.max(0, Math.floor(seconds || 0));
  return `${String(Math.floor(value / 60)).padStart(2, '0')}:${String(value % 60).padStart(2, '0')}`;
}

function formatMeetingDuration(seconds?: number | null) {
  if (seconds == null || !Number.isFinite(seconds) || seconds <= 0) return '暂无时长';
  const totalSeconds = Math.max(1, Math.round(seconds));
  const minutes = Math.floor(totalSeconds / 60);
  const remainingSeconds = totalSeconds % 60;
  if (minutes === 0) return `${remainingSeconds}s`;
  if (remainingSeconds === 0) return `${minutes}min`;
  return `${minutes}m ${remainingSeconds}s`;
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
  const [meetingStartedAt, setMeetingStartedAt] = useState<number | null>(null);
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

  const formatMeetingDate = (value?: string) => {
    if (!value) return '暂无时间';
    const date = new Date(value);
    if (Number.isNaN(date.getTime())) return '暂无时间';
    return `${date.getFullYear()}.${String(date.getMonth() + 1).padStart(2, '0')}.${String(date.getDate()).padStart(2, '0')}  ${String(date.getHours()).padStart(2, '0')}:${String(date.getMinutes()).padStart(2, '0')}`;
  };

  const requestStopConfirmation = async () => new Promise<boolean>((resolve) => {
    stopResolverRef.current = resolve;
    setStopConfirmOpen(true);
  });

  const resolveStopConfirmation = (confirmed: boolean) => {
    stopResolverRef.current?.(confirmed);
    stopResolverRef.current = null;
    setStopConfirmOpen(false);
  };

  const stopMeetingFromHeader = () => {
    if (!recordingState.isRecording || isStopping) return;
    window.dispatchEvent(new CustomEvent('end-meeting-from-header'));
  };

  const startMeeting = async (name?: string) => {
    try {
      setMeetingStartedAt(Date.now());
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
  const liveStart = meetingStartedAt ? new Date(meetingStartedAt) : new Date();
  const liveStartLabel = `${liveStart.getFullYear()}.${String(liveStart.getMonth() + 1).padStart(2, '0')}.${String(liveStart.getDate()).padStart(2, '0')} ${String(liveStart.getHours()).padStart(2, '0')}:${String(liveStart.getMinutes()).padStart(2, '0')}`;

  return (
    <div className="hyos-page">
      {!showLive && (
        <>
          <section className="hyos-launcher" aria-label="会议工作台">
            <button className="hyos-launch primary" onClick={() => setQuickMeetingOpen(true)}><span><img src="/hyos-play-mark.png" alt="" /></span><strong>快速会议</strong><small>立即开始录音与转写</small></button>
            <button className="hyos-launch" onClick={() => router.push('/files')}><span><img src="/hyos-folder-mark.png" alt="" /></span><strong>会议管理</strong><small>查看和管理会议资料</small><ChevronRight className="hyos-launch-arrow" size={22} /></button>
          </section>
          <section className="hyos-panel hyos-recent">
            <div className="hyos-panel-head"><h2>最近会议</h2><button onClick={() => router.push('/files')}>查看全部 <ChevronRight size={15} /></button></div>
            {meetings.length === 0 ? <div className="hyos-empty-files">暂无会议文件</div> : meetings.slice(0, 3).map((meeting) => <button key={meeting.id} className="hyos-record-row" onClick={() => router.push(`/meeting-details?id=${meeting.id}`)}><span className="hyos-record-icon"><img src="/hyos-document-mark.png" alt="" /></span><span className="hyos-record-copy"><span className="hyos-record-title"><b>{meeting.title}</b><em>{formatMeetingDuration(meeting.duration_seconds)}</em></span><small>{formatMeetingDate(meeting.created_at)}</small></span></button>)}
          </section>
          <div className="hyos-device-strip"><div><b>设备状态</b><span><i />本地运行</span></div><div><b>本地模型服务</b><strong>已配置</strong></div><div><b>会议麦克风</b><strong>{selectedDevices.micDevice || '系统默认'}</strong></div></div>
        </>
      )}

      {showLive && (
        <section className="hyos-live-page">
          <div className="hyos-live-heading">
            <div className="hyos-live-title-row">
              <span className="hyos-live-meeting-mark"><img src="/hyos-document-mark.png" alt="" /></span>
              <h1>{meetingTitle || '日常会议'}</h1>
              <span className="hyos-live-record"><i />{recordingState.status === RecordingStatus.PROCESSING_TRANSCRIPTS ? '处理中' : '录音中'} {formatClock(recordingState.activeDuration)}</span>
              <button className="hyos-live-end-button" onClick={stopMeetingFromHeader} disabled={!recordingState.isRecording || isStopping}>结束会议</button>
            </div>
            <div className="hyos-live-meta"><span>{liveStartLabel}–{formatClock(recordingState.activeDuration)}</span><span>总结模板:<b>{selectedTemplateName}</b></span><span>内容分段:<b>{transcripts.length}段</b></span></div>
          </div>
          <div className="hyos-live-grid">
            <div className="hyos-transcript-pane">
              <div className="hyos-pane-title"><div className="hyos-pane-title-main"><span>逐字稿</span><em className="hyos-transcript-mode">精选发言人</em></div><em>{transcripts.length}段</em></div>
              <div className="hyos-transcript-virtual">
                {liveSegments.length === 0 ? (
                  <div className="hyos-live-empty-transcript"><i /><strong>等待语音转写</strong><span>开始发言后，逐字稿会显示在这里</span></div>
                ) : (
                  <VirtualizedTranscriptView segments={liveSegments} isRecording={recordingState.isRecording} isProcessing={recordingState.isProcessing} isStopping={recordingState.isStopping} enableStreaming={false} showConfidence={false} />
                )}
              </div>
            </div>
            <aside className="hyos-summary-pane" aria-label="会议分时摘要">
              <div className="hyos-summary-heading">
                <h2>会议分时摘要</h2>
                <span>每8分钟更新</span>
              </div>
              <div className="hyos-minute-summary-list">
                {minuteSummaries.length === 0 && !generatingMinute && (
                  <div className="hyos-summary-empty-state">
                    <div className="hyos-summary-empty-icon">✦</div>
                    <strong>摘要生成中</strong>
                    <p>根据每个时间段的转写内容生成重点、决定和待办。</p>
                  </div>
                )}
                {minuteSummaries.map((summary, index) => (
                  <article key={summary.minute_index} className={`hyos-minute-summary${index === 0 ? ' is-current' : ''}${summary.status ? ` ${summary.status}` : ''}`}>
                    <div className="hyos-minute-summary-top">
                      <time>{formatMinuteRange(summary.start_seconds, summary.end_seconds)}</time>
                      <span>{summary.status === 'error' ? '生成失败' : summary.status === 'empty' ? '暂无内容' : '已生成'}</span>
                    </div>
                    <p>{summary.summary || '本时段暂未检测到有效发言。'}</p>
                  </article>
                ))}
                {generatingMinute !== null && (
                  <div className="hyos-summary-generating"><i />正在生成第 {generatingMinute + 1} 段摘要…</div>
                )}
              </div>
            </aside>
          </div>
          <div className="hyos-live-controls"><RecordingControls isRecording={recordingState.isRecording} isPaused={recordingState.isPaused} hasSpeech={recordingState.speechActive} onRecordingStop={(callApi = true, speakerCalibration) => handleRecordingStop(callApi, speakerCalibration)} onRecordingStart={() => startMeeting(meetingTitle)} onTranscriptReceived={() => {}} onStopInitiated={() => setIsStopping(true)} onBeforeStop={requestStopConfirmation} barHeights={['6px', '12px', '18px', '10px', '16px', '8px', '20px', '14px', '9px', '16px', '7px', '13px', '18px', '11px', '8px']} recordingDuration={formatClock(recordingState.activeDuration)} onTranscriptionError={(message) => toast.error(message)} isRecordingDisabled={isRecordingDisabled} isParentProcessing={isStopping} selectedDevices={selectedDevices} meetingName={meetingTitle} /></div>
        </section>
      )}

      {quickMeetingOpen && <div className="hyos-modal-backdrop" role="presentation"><div className="hyos-modal" role="dialog" aria-modal="true" aria-labelledby="quick-meeting-title"><button className="hyos-modal-close" onClick={() => setQuickMeetingOpen(false)} aria-label="关闭">×</button><h2 id="quick-meeting-title">快速会议</h2><p>填写会议信息并选择总结模板后开始记录。</p><label>会议名称<input value={quickMeetingName} onChange={(event) => setQuickMeetingName(event.target.value)} /></label><label>总结模板<select value={quickMeetingTemplate} onChange={(event) => setQuickMeetingTemplate(event.target.value)}>{meetingTemplates.map((template) => <option key={template.id} value={template.id}>{template.name}</option>)}</select></label><div className="hyos-form-tip">会议密码将在会议结束并保存成功后设置。</div><div className="hyos-modal-actions"><button className="hyos-secondary" onClick={() => setQuickMeetingOpen(false)}>取消</button><button className="hyos-primary" onClick={() => startMeeting(quickMeetingName)} disabled={!quickMeetingName.trim()}>开始会议</button></div></div></div>}
      {stopConfirmOpen && <div className="hyos-modal-backdrop" role="presentation"><div className="hyos-modal compact" role="dialog" aria-modal="true" aria-labelledby="stop-meeting-title"><h2 id="stop-meeting-title">确认结束会议？</h2><p>系统将保存录音、逐字稿和分时摘要，随后设置本次会议密码。</p><div className="hyos-modal-actions"><button className="hyos-secondary" onClick={() => resolveStopConfirmation(false)}>继续会议</button><button className="hyos-danger" onClick={() => resolveStopConfirmation(true)}>结束并保存</button></div></div></div>}
      <TranscriptRecovery isOpen={showRecoveryDialog} onClose={() => setShowRecoveryDialog(false)} recoverableMeetings={recoverableMeetings} onRecover={handleRecovery} onDelete={deleteRecoverableMeeting} onLoadPreview={loadMeetingTranscripts} />
    </div>
  );
}
