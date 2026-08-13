'use client';

import { useEffect, useMemo, useState } from 'react';
import { FileAudio, FileText, LockKeyhole, Search, Upload } from 'lucide-react';
import { useRouter } from 'next/navigation';
import { toast } from 'sonner';
import { storageService, Meeting } from '@/services/storageService';
import { useImportDialog } from '@/contexts/ImportDialogContext';
import { accessControlService } from '@/services/accessControlService';

type ProtectedMeeting = Meeting & { usesIndependentPassword?: boolean };

export default function FilesPage() {
  const router = useRouter();
  const { openImportDialog } = useImportDialog();
  const [meetings, setMeetings] = useState<ProtectedMeeting[]>([]);
  const [query, setQuery] = useState('');

  useEffect(() => {
    storageService.getMeetings().then(async (loaded) => {
      const protectedMeetings = await Promise.all(loaded.map(async (meeting) => ({
        ...meeting,
        usesIndependentPassword: await accessControlService.meetingUsesIndependentPassword(meeting.id).catch(() => false),
      })));
      setMeetings(protectedMeetings);
    }).catch((error) => {
      console.error('Failed to load meeting files:', error);
      setMeetings([]);
    });
  }, []);

  const items = useMemo(
    () => {
      const normalized = query.trim().toLowerCase();
      if (!normalized) return meetings;
      return meetings.filter((meeting) => [
        meeting.title,
        meeting.id,
        meeting.created_at,
        meeting.folder_path,
        new Date(meeting.created_at || 0).toLocaleString('zh-CN', { hour12: false }),
      ].filter(Boolean).join(' ').toLowerCase().includes(normalized));
    },
    [meetings, query],
  );

  return (
    <div className="hyos-page hyos-files-page">
      <div className="hyos-page-title">
        <div><button className="hyos-back" onClick={() => router.push('/')}>← 返回工作台</button><h1>会议文件</h1><p>查看现场会议，或上传音频文件生成会议纪要。</p></div>
        <button className="hyos-primary" onClick={() => openImportDialog()}><Upload size={16} />上传音频文件</button>
      </div>
      <section className="hyos-files-layout">
        <div className="hyos-panel hyos-file-list">
          <div className="hyos-search"><Search size={16} /><input placeholder="搜索会议或音频文件" value={query} onChange={(event) => setQuery(event.target.value)} /></div>
          <div className="hyos-panel-head"><h2>全部文件</h2><span>{items.length} 项</span></div>
          {items.length === 0 ? <div className="hyos-empty-files">暂无会议文件</div> : items.map((meeting) => (
            <button className="hyos-file-row" key={meeting.id} onClick={() => router.push(`/meeting-details?id=${meeting.id}`)}>
              <span className="hyos-file-icon"><FileText size={16} /></span>
              <span><b>{meeting.title}</b><small>{new Date(meeting.created_at || Date.now()).toLocaleString('zh-CN', { hour12: false })}</small></span>
              <em><LockKeyhole size={13} />{meeting.usesIndependentPassword ? '独立会议密码' : '管理员密码'}</em>
            </button>
          ))}
        </div>
        <div className="hyos-panel hyos-file-placeholder"><FileAudio size={30} /><h2>选择会议文件</h2><p>从左侧选择会议，验证会议密码后查看逐字稿、AI 总结和录音成果。</p><button className="hyos-secondary" onClick={() => toast.info('请先选择左侧会议文件')}>查看文件详情</button></div>
      </section>
    </div>
  );
}
