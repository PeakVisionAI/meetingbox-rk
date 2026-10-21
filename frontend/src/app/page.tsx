'use client';

import { useEffect, useRef, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import { invoke } from '@tauri-apps/api/core';
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
  const [servicePreparing, setServicePreparing] = useState(false);
  const [startPending, setStartPending] = useState(false);
  const startPendingRef = useRef(false);
  const [startStage, setStartStage] = useState('正在检查录音状态');
  const [startWaitSeconds, setStartWaitSeconds] = useState(0);
  const [startError, setStartError] = useState('');
  const [summaryModelStatus, setSummaryModelStatus] = useState('总结模型在后台准备，不影响录音');
  useEffect(() => {
    if (!startPending) {setStartWaitSeconds(0); return;}
    const began = Date.now();
    const timer = window.setInterval(() => setStartWaitSeconds(Math.floor((Date.now()-began)/1000)),1000);
    return () => window.clearInterval(timer);
  }, [startPending]);
  useEffect(() => {
    const stage = (event: Event) => setStartStage((event as CustomEvent<string>).detail);
    window.addEventListener('recording-start-stage', stage);
    let disposed=false; let stop:(()=>void)|undefined;
    void listen<string>('recording-start-progress',event=>setStartStage(event.payload)).then(fn=>{if(disposed)fn();else stop=fn;});
    return ()=>{disposed=true;stop?.();window.removeEventListener('recording-start-stage',stage);};
  }, []);
  useEffect(() => {
    if(!recordingState.isRecording)return;
    let active=true;
    const poll=()=>void invoke<{stage:string}>('get_local_summary_status').then(value=>{if(active)setSummaryModelStatus(value.stage);}).catch(()=>{});
    poll();const timer=window.setInterval(poll,5000);
    return ()=>{active=false;window.clearInterval(timer);};
  },[recordingState.isRecording]);
  const [serviceProgress, setServiceProgress] = useState({stage:'检查本地模型服务',completed:0,total:3,ready:false,error:false});
  useEffect(() => {
    const start = () => { setServicePreparing(true); setServiceProgress({stage:'检查本地模型服务',completed:0,total:3,ready:false,error:false}); };
    const end = () => setServicePreparing(false);
    window.addEventListener('local-services-check-start', start);
    window.addEventListener('local-services-check-end', end);
    let disposed = false; let stop: (() => void) | undefined;
    void listen<typeof serviceProgress>('local-services-progress', event => setServiceProgress(event.payload)).then(fn => { if(disposed) fn(); else stop=fn; });
    return () => { disposed=true; stop?.(); window.removeEventListener('local-services-check-start', start); window.removeEventListener('local-services-check-end', end); };
  }, []);

  const [isRecordingState, setIsRecordingState] = useState(false);
  const [quickMeetingName, setQuickMeetingName] = useState('日常会议');
  const [quickMeetingTemplate, setQuickMeetingTemplate] = useState('standard_meeting');
  const [stopConfirmOpen, setStopConfirmOpen] = useState(false);
  const [showRecoveryDialog, setShowRecoveryDialog] = useState(false);
  const [meetingStartedAt, setMeetingStartedAt] = useState<number | null>(null);
  const stopResolverRef = useRef<((confirmed: boolean) => void) | null>(null);
  const minuteSummaryScrollRef = useRef<HTMLDivElement>(null);
  const shouldFollowMinuteSummaryRef = useRef(true);

  const { handleRecordingStart } = useRecordingStart(isRecordingState, setIsRecordingState, undefined);
  const { handleRecordingStop, isStopping, setIsStopping } = useRecordingStop(setIsRecordingState, () => {});
  const [shutdownSeconds, setShutdownSeconds] = useState(0);
  useEffect(() => {
    if (!isStopping) { setShutdownSeconds(0); return; }
    const started = Date.now();
    const timer = window.setInterval(() => setShutdownSeconds(Math.floor((Date.now() - started) / 1000)), 1000);
    return () => window.clearInterval(timer);
  }, [isStopping]);

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
    if(startPendingRef.current)return;
    startPendingRef.current=true;
    setStartPending(true);setStartStage('正在检查录音状态');setStartError('');
    try {
      setMeetingStartedAt(Date.now());
      sessionStorage.setItem('pending_summary_template_id', quickMeetingTemplate);
      sessionStorage.removeItem('pending_minute_summaries');
      await handleRecordingStart(name);
      setQuickMeetingOpen(false);
      setIsMeetingActive(true);
      toast.success('会议已开始', { description: '会议结束并保存后设置 6 位访问密码' });
    } catch (error) {
      setStartError(error instanceof Error ? error.message : String(error));
      toast.error('无法开始会议', { description: error instanceof Error ? error.message : '请检查会议麦克风和本地模型服务' });
    } finally {
      startPendingRef.current=false;setStartPending(false);
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
  const latestMinuteSummary = minuteSummaries[minuteSummaries.length - 1];

  useEffect(() => {
    const container = minuteSummaryScrollRef.current;
    if (!showLive || !container) return;

    shouldFollowMinuteSummaryRef.current = true;
    const handleScroll = () => {
      const distanceFromBottom = container.scrollHeight - container.scrollTop - container.clientHeight;
      shouldFollowMinuteSummaryRef.current = distanceFromBottom <= 80;
    };

    container.addEventListener('scroll', handleScroll, { passive: true });
    return () => container.removeEventListener('scroll', handleScroll);
  }, [showLive]);

  useEffect(() => {
    const container = minuteSummaryScrollRef.current;
    if (!showLive || !container || !shouldFollowMinuteSummaryRef.current) return;

    const frame = requestAnimationFrame(() => {
      container.scrollTop = container.scrollHeight;
    });
    return () => cancelAnimationFrame(frame);
  }, [showLive, minuteSummaries.length, latestMinuteSummary?.summary, latestMinuteSummary?.status, generatingMinute]);

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
          <div className="hyos-device-strip"><div><b>设备状态</b><span><i />本地运行</span></div><div><b>本地模型服务</b><strong>{servicePreparing ? '正在启动' : serviceProgress.ready ? '已就绪' : '开始前自动检查'}</strong></div><div><b>会议麦克风</b><strong>{selectedDevices.micDevice || '系统默认'}</strong></div></div>
        </>
      )}

      {showLive && (
        <section className="hyos-live-page">
          <div className="hyos-live-heading">
            <div className="hyos-live-title-row">
              <span className="hyos-live-meeting-mark"><img src="/hyos-document-mark.png" alt="" /></span>
              <h1>{meetingTitle || '日常会议'}</h1>
              <span className="hyos-live-record"><i />{isStopping || recordingState.isProcessing || recordingState.isSaving ? '正在保存' : '录音中'} {!isStopping && !recordingState.isProcessing && !recordingState.isSaving && formatClock(recordingState.activeDuration)}</span>
              <button className="hyos-live-end-button" onClick={stopMeetingFromHeader} disabled={!recordingState.isRecording || isStopping}>结束会议</button>
            </div>
            <div className="hyos-live-meta"><span>{liveStartLabel}–{formatClock(recordingState.activeDuration)}</span><span>总结模板:<b>{selectedTemplateName}</b></span><span>内容分段:<b>{transcripts.length}段</b></span></div>
          </div>
          {(isStopping || recordingState.isProcessing || recordingState.isSaving) && (
            <div role="status" className="mx-4 mb-4 flex items-center gap-3 rounded-xl border border-blue-200 bg-blue-50 px-5 py-4 text-blue-900">
              <span className="h-5 w-5 shrink-0 animate-spin rounded-full border-2 border-blue-200 border-t-blue-600" />
              <div><strong>{recordingState.isSaving ? '正在保存会议文件' : '录音已结束，正在处理最后的语音'}</strong><p className="mt-1 text-sm">已等待 {shutdownSeconds} 秒，逐字稿会继续补齐。完成后自动打开会议文件。</p></div>
            </div>
          )}
          <div className="hyos-live-grid">
            <div className="hyos-transcript-pane">
              <div className="hyos-pane-title"><div className="hyos-pane-title-main"><span>逐字稿</span><em className="hyos-transcript-mode">精选发言人</em></div><em>{transcripts.length}段</em></div>
              <div className="hyos-transcript-virtual">
                <VirtualizedTranscriptView segments={liveSegments} isRecording={recordingState.isRecording} isPaused={recordingState.isPaused} isProcessing={recordingState.isProcessing} isStopping={recordingState.isStopping} enableStreaming={false} showConfidence={false} />
              </div>
            </div>
            <aside className="hyos-summary-pane" aria-label="会议分时摘要">
              <div className="hyos-summary-heading">
                <h2>会议分时摘要</h2>
                <span>约五分钟 · 完整段落后归纳</span>
              </div>
              <p role="status" className="px-4 pb-2 text-xs text-slate-500">{summaryModelStatus}</p>
              <div ref={minuteSummaryScrollRef} className="hyos-minute-summary-list">
                {minuteSummaries.length === 0 && generatingMinute === null && (
                  <div className="hyos-summary-empty-state">
                    <div className="hyos-summary-empty-icon">✦</div>
                    <strong>等待首段摘要</strong>
                    <p>约五分钟后的首个完整段落结束时生成短摘要，最终纪要将复用并核查原文。</p>
                  </div>
                )}
                {minuteSummaries.map((summary, index) => (
                  <article key={summary.minute_index} className={`hyos-minute-summary${index === 0 ? ' is-current' : ''}${summary.status ? ` ${summary.status}` : ''}`}>
                    <div className="hyos-minute-summary-top">
                      <time>{formatMinuteRange(summary.start_seconds, summary.end_seconds)}</time>
                      <span>{summary.status === 'error' ? '生成失败' : summary.status === 'empty' ? '暂无内容' : '已生成'}</span>
                    </div>
                    <p>{summary.summary || '本时段暂未检测到有效发言。'}</p>
                    {Array.isArray(summary.structure?.todos) && summary.structure.todos.length > 0 && <small>已保留 {summary.structure.todos.length} 项待办，供最终纪要归纳</small>}
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

      {(startPending || servicePreparing) && !recordingState.isRecording && <div className="hyos-modal-backdrop" style={{zIndex:100}}><div className="hyos-modal" role="dialog" aria-modal="true" aria-label="准备开始会议"><h2>正在准备会议</h2><p role="status" aria-live="polite">{servicePreparing ? serviceProgress.stage : startStage}</p>{servicePreparing ? <progress aria-label="语音服务就绪进度" max={serviceProgress.total} value={serviceProgress.completed} style={{width:'100%',height:12,accentColor:'#2563eb'}} /> : <progress aria-label="正在准备录音" style={{width:'100%',height:12,accentColor:'#2563eb'}} />}<p>已等待 {startWaitSeconds} 秒{servicePreparing ? ` · 语音服务 ${serviceProgress.completed}/${serviceProgress.total} 就绪` : ''}</p><div className="hyos-form-tip">准备完成后自动开始录音，无需重复点击。总结模型在后台单独准备。</div></div></div>}
      {quickMeetingOpen && !startPending && !servicePreparing && <div className="hyos-modal-backdrop" role="presentation"><div className="hyos-modal" role="dialog" aria-modal="true" aria-labelledby="quick-meeting-title"><button className="hyos-modal-close" onClick={() => setQuickMeetingOpen(false)} aria-label="关闭">×</button><h2 id="quick-meeting-title">快速会议</h2><p>填写会议信息并选择总结模板后开始记录。</p><label>会议名称<input value={quickMeetingName} onChange={(event) => setQuickMeetingName(event.target.value)} /></label><label>总结模板<select value={quickMeetingTemplate} onChange={(event) => setQuickMeetingTemplate(event.target.value)}>{meetingTemplates.map((template) => <option key={template.id} value={template.id}>{template.name}</option>)}</select></label><div className="hyos-form-tip">会议密码将在会议结束并保存成功后设置。</div>{startError && <p role="alert" className="mt-2 text-sm text-red-600">{startError}</p>}<div className="hyos-modal-actions"><button className="hyos-secondary" onClick={() => setQuickMeetingOpen(false)}>取消</button><button className="hyos-primary" onClick={() => startMeeting(quickMeetingName)} disabled={!quickMeetingName.trim() || servicePreparing || startPending}>开始会议</button></div></div></div>}
      {stopConfirmOpen && <div className="hyos-modal-backdrop" role="presentation"><div className="hyos-modal compact" role="dialog" aria-modal="true" aria-labelledby="stop-meeting-title"><h2 id="stop-meeting-title">确认结束会议？</h2><p>系统将保存录音、逐字稿和分时摘要，随后设置本次会议密码。</p><div className="hyos-modal-actions"><button className="hyos-secondary" onClick={() => resolveStopConfirmation(false)}>继续会议</button><button className="hyos-danger" onClick={() => resolveStopConfirmation(true)}>结束并保存</button></div></div></div>}
      <TranscriptRecovery isOpen={showRecoveryDialog} onClose={() => setShowRecoveryDialog(false)} recoverableMeetings={recoverableMeetings} onRecover={handleRecovery} onDelete={deleteRecoverableMeeting} onLoadPreview={loadMeetingTranscripts} />
    </div>
  );
}
