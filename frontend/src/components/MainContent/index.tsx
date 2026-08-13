'use client';

import React from 'react';
import { useSidebar } from '@/components/Sidebar/SidebarProvider';
import { usePathname, useRouter } from 'next/navigation';
import { Settings, FileText, Play, Square } from 'lucide-react';
import { useRecordingState } from '@/contexts/RecordingStateContext';
import { invoke } from '@tauri-apps/api/core';
import { appDataDir } from '@tauri-apps/api/path';
import { toast } from 'sonner';

interface MainContentProps {
  children: React.ReactNode;
}

const MainContent: React.FC<MainContentProps> = ({ children }) => {
  const router = useRouter();
  const pathname = usePathname();
  const { isMeetingActive } = useSidebar();
  const recordingState = useRecordingState();
  const isHome = pathname === '/';
  const isFiles = pathname === '/files';
  const isSettings = pathname === '/settings';
  const recordingActive = recordingState.isRecording || recordingState.isStopping || recordingState.isProcessing || recordingState.isSaving;

  const goTo = (path: string) => {
    if (recordingActive && path !== '/') {
      toast.info('会议进行中，请先结束会议');
      return;
    }
    router.push(path);
  };

  const stopRecording = async () => {
    if (!window.confirm('确认结束并保存当前会议？')) return;
    try {
      const dataDir = await appDataDir();
      const timestamp = new Date().toISOString().replace(/[:.]/g, '-');
      await invoke('stop_recording', { args: { save_path: `${dataDir}/recording-${timestamp}.wav` } });
      (window as any).handleRecordingStop?.(true);
    } catch (error) {
      toast.error('无法结束会议', { description: String(error) });
    }
  };

  return (
    <main className="hyos-main">
      <header className="hyos-topbar">
        <button className="hyos-brand" onClick={() => router.push('/')} aria-label="返回工作台">
          <span>智能会议助手</span><i>-</i><b>HYOS</b>
        </button>
        <nav className="hyos-nav" aria-label="主功能导航">
          <button className={isHome ? 'active' : ''} onClick={() => goTo('/')}><Play size={15} />{recordingActive ? '当前会议' : '快速会议'}</button>
          <button className={isFiles ? 'active' : ''} onClick={() => goTo('/files')} disabled={recordingActive}><FileText size={15} />文件管理</button>
          <button className={isSettings ? 'active' : ''} onClick={() => goTo('/settings')} disabled={recordingActive}><Settings size={15} />设置</button>
        </nav>
        {recordingActive && !isHome ? <div className="hyos-recording-actions"><button onClick={() => router.push('/')}><Play size={14} />返回会议</button><button className="danger" onClick={stopRecording} disabled={!recordingState.isRecording}><Square size={14} />结束会议</button></div> : <div className="hyos-user"><span className={isMeetingActive ? 'recording-dot' : 'status-dot'} />{isMeetingActive ? '会议进行中' : '已登录'}</div>}
      </header>
      <div className="hyos-content">{children}</div>
    </main>
  );
};

export default MainContent;
