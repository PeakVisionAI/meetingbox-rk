'use client';

import { usePathname, useRouter } from 'next/navigation';
import { Settings, FileText, Play, Radio } from 'lucide-react';
import { useSidebar } from '@/components/Sidebar/SidebarProvider';
import { useRecordingState } from '@/contexts/RecordingStateContext';

export default function HyosHeader() {
  const router = useRouter();
  const pathname = usePathname();
  const { currentMeeting } = useSidebar();
  const { isRecording } = useRecordingState();

  const startMeeting = () => {
    if (pathname !== '/') {
      router.push('/');
      window.setTimeout(() => window.dispatchEvent(new CustomEvent('start-recording-from-sidebar')), 80);
      return;
    }
    window.dispatchEvent(new CustomEvent('start-recording-from-sidebar'));
  };

  const openFiles = () => {
    if (currentMeeting?.id && currentMeeting.id !== 'intro-call') {
      router.push(`/meeting-details?id=${currentMeeting.id}`);
    } else {
      router.push('/');
    }
  };

  return (
    <header className="hyos-header" aria-label="智能会议助手主导航">
      <button className="hyos-brand" onClick={() => router.push('/')} aria-label="返回工作台">
        <span>智能会议助手</span><i>-</i><b>HYOS</b>
      </button>
      <nav className="hyos-nav" aria-label="主功能导航">
        <button className={pathname === '/' ? 'active' : ''} onClick={startMeeting} disabled={isRecording}>
          <Play size={15} fill="currentColor" />快速会议
        </button>
        <button className={pathname?.startsWith('/meeting-details') ? 'active' : ''} onClick={openFiles}>
          <FileText size={15} />文件管理
        </button>
        <button className={pathname === '/settings' ? 'active' : ''} onClick={() => router.push('/settings')}>
          <Settings size={15} />设置
        </button>
      </nav>
      <div className="hyos-service-pill"><Radio size={14} /><span>本机服务</span><i /></div>
    </header>
  );
}
