'use client';

import { useEffect, useState } from 'react';
import { CalendarDays, ChevronLeft, ChevronRight, FileAudio, FileText, LockKeyhole, Search, SlidersHorizontal, Trash2, Upload } from 'lucide-react';
import { useRouter } from 'next/navigation';
import { toast } from 'sonner';
import { invoke } from '@tauri-apps/api/core';
import { useImportDialog } from '@/contexts/ImportDialogContext';
import { accessControlService } from '@/services/accessControlService';
import { MeetingSearchResult, productivityService } from '@/services/productivityService';

type ProtectedMeeting = MeetingSearchResult & { usesIndependentPassword?: boolean };
const PAGE_SIZE = 30;

export default function FilesPage() {
  const router = useRouter();
  const { openImportDialog } = useImportDialog();
  const [meetings, setMeetings] = useState<ProtectedMeeting[]>([]);
  const [query, setQuery] = useState('');
  const [speaker, setSpeaker] = useState('');
  const [dateFrom, setDateFrom] = useState('');
  const [dateTo, setDateTo] = useState('');
  const [page, setPage] = useState(0);
  const [totalCount, setTotalCount] = useState(0);
  const [hasMore, setHasMore] = useState(false);
  const [loading, setLoading] = useState(true);
  const [showFilters, setShowFilters] = useState(false);
  const [deleteTarget, setDeleteTarget] = useState<ProtectedMeeting | null>(null);
  const [deletingMeetingId, setDeletingMeetingId] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    productivityService.searchMeetings({ query, speaker, dateFrom, dateTo, limit: PAGE_SIZE, offset: page * PAGE_SIZE }).then(async (result) => {
      if (cancelled) return;
      const protectedMeetings = await Promise.all(result.meetings.map(async (meeting) => ({
        ...meeting,
        usesIndependentPassword: await accessControlService.meetingUsesIndependentPassword(meeting.id).catch(() => false),
      })));
      if (cancelled) return;
      setMeetings(protectedMeetings);
      setTotalCount(result.total_count);
      setHasMore(result.has_more);
    }).catch((error) => {
      if (cancelled) return;
      console.error('Failed to load meeting files:', error);
      setMeetings([]);
      setTotalCount(0);
      setHasMore(false);
      toast.error('无法加载会议文件', { description: String(error) });
    }).finally(() => {
      if (!cancelled) setLoading(false);
    });
    return () => { cancelled = true; };
  }, [query, speaker, dateFrom, dateTo, page]);

  const updateFilter = (setter: (value: string) => void, value: string) => {
    setter(value);
    setPage(0);
  };

  const deleteMeeting = async () => {
    if (!deleteTarget) return;
    const meeting = deleteTarget;
    setDeleteTarget(null);
    setDeletingMeetingId(meeting.id);
    try {
      await invoke('api_delete_meeting', { meetingId: meeting.id });
      setMeetings((current) => current.filter((item) => item.id !== meeting.id));
      setTotalCount((current) => Math.max(0, current - 1));
      if (meetings.length === 1 && page > 0) setPage((current) => current - 1);
      toast.success('会议文件已删除', { description: '会议记录、转写和摘要已从应用中移除，原始录音文件仍保留。' });
    } catch (error) {
      console.error('Failed to delete meeting:', error);
      toast.error('删除会议文件失败', { description: error instanceof Error ? error.message : String(error) });
    } finally {
      setDeletingMeetingId(null);
    }
  };

  return (
    <div className="hyos-page hyos-files-page">
      <div className="hyos-page-title">
        <div><button className="hyos-back" onClick={() => router.push('/')}>← 返回工作台</button><h1>会议文件</h1><p>按标题、日期、说话人或会议内容查找本地会议成果。</p></div>
        <button className="hyos-primary" onClick={() => openImportDialog()}><Upload size={16} />上传音频文件</button>
      </div>
      <section className="hyos-files-layout">
        <div className="hyos-panel hyos-file-list">
          <div className="hyos-search"><Search size={16} /><input placeholder="搜索标题或会议关键词" value={query} onChange={(event) => updateFilter(setQuery, event.target.value)} /><button type="button" onClick={() => setShowFilters((value) => !value)} aria-label="筛选条件" title="筛选条件"><SlidersHorizontal size={15} /></button></div>
          {showFilters && <div className="hyos-file-filters"><label><span>说话人</span><input placeholder="例如：候选发言人1" value={speaker} onChange={(event) => updateFilter(setSpeaker, event.target.value)} /></label><label><span><CalendarDays size={13} />开始日期</span><input type="date" value={dateFrom} onChange={(event) => updateFilter(setDateFrom, event.target.value)} /></label><label><span><CalendarDays size={13} />结束日期</span><input type="date" value={dateTo} onChange={(event) => updateFilter(setDateTo, event.target.value)} /></label></div>}
          <div className="hyos-panel-head"><h2>全部文件</h2><span>{totalCount} 项</span></div>
          <div className="hyos-file-rows custom-scrollbar">
            {loading ? <div className="hyos-empty-files">正在加载会议文件…</div> : meetings.length === 0 ? <div className="hyos-empty-files">没有匹配的会议文件</div> : meetings.map((meeting) => (
              <div className="hyos-file-row" key={meeting.id}>
                <button className="hyos-file-row-main" onClick={() => router.push(`/meeting-details?id=${meeting.id}`)}>
                  <span className="hyos-file-icon"><FileText size={16} /></span>
                  <span><b>{meeting.title}</b><small>{new Date(meeting.created_at || Date.now()).toLocaleString('zh-CN', { hour12: false })} · {meeting.transcript_count} 段转写</small></span>
                </button>
                <em><LockKeyhole size={13} />{meeting.usesIndependentPassword ? '独立会议密码' : '管理员密码'}</em>
                <button
                  type="button"
                  className="hyos-icon-danger hyos-file-delete"
                  onClick={() => setDeleteTarget(meeting)}
                  disabled={deletingMeetingId === meeting.id}
                  aria-label={`删除会议${meeting.title}`}
                  title="删除会议文件"
                >
                  <Trash2 size={15} />
                </button>
              </div>
            ))}
          </div>
          <div className="hyos-file-pagination"><button type="button" onClick={() => setPage((value) => Math.max(0, value - 1))} disabled={page === 0 || loading} aria-label="上一页"><ChevronLeft size={16} /></button><span>第 {page + 1} / {Math.max(1, Math.ceil(totalCount / PAGE_SIZE))} 页</span><button type="button" onClick={() => setPage((value) => value + 1)} disabled={!hasMore || loading} aria-label="下一页"><ChevronRight size={16} /></button></div>
        </div>
        <div className="hyos-panel hyos-file-placeholder"><FileAudio size={30} /><h2>选择会议文件</h2><p>从左侧选择会议，验证会议密码后查看逐字稿、摘要、行动项和录音成果。</p><button className="hyos-secondary" onClick={() => toast.info('请先选择左侧会议文件')}>查看文件详情</button></div>
      </section>
      {deleteTarget && (
        <div className="hyos-modal-backdrop" role="presentation" onClick={() => setDeleteTarget(null)}>
          <div className="hyos-modal compact" role="dialog" aria-modal="true" aria-labelledby="delete-meeting-title" onClick={(event) => event.stopPropagation()}>
            <h2 id="delete-meeting-title">删除会议文件？</h2>
            <p>将删除“{deleteTarget.title}”的会议记录、转写和摘要。原始录音文件会保留在本地。</p>
            <div className="hyos-modal-actions">
              <button className="hyos-secondary" type="button" onClick={() => setDeleteTarget(null)}>取消</button>
              <button className="hyos-danger hyos-confirm-delete" type="button" onClick={() => void deleteMeeting()}>确认删除</button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
