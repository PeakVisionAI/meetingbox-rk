'use client';

import React, { useEffect, useRef, useState } from 'react';
import { useSidebar } from '@/components/Sidebar/SidebarProvider';
import { usePathname, useRouter } from 'next/navigation';
import { Settings, Play, Power, BrainCircuit } from 'lucide-react';
import { useRecordingState } from '@/contexts/RecordingStateContext';
import { listen } from '@tauri-apps/api/event';
import { invoke } from '@tauri-apps/api/core';
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
  const isSettings = pathname === '/settings';
  const isKnowledge = pathname === '/knowledge';
  const isKnowledgeRef = useRef(isKnowledge);
  isKnowledgeRef.current = isKnowledge;
  const recordingActive = recordingState.isRecording || recordingState.isStopping || recordingState.isProcessing || recordingState.isSaving;
  const recordingActiveRef = useRef(recordingActive);
  const [closingApp, setClosingApp] = useState(false);
  const isHomeRef = useRef(isHome);
  recordingActiveRef.current = recordingActive;
  isHomeRef.current = isHome;

  const openSettings = () => {
    if (recordingActive) {
      toast.info('会议进行中，请先结束会议');
      return;
    }
    router.push('/settings');
  };

  const closeApp = async () => {
    setClosingApp(true);
    try {
      await invoke('close_application');
    } catch (error) {
      toast.error('无法关闭应用', { description: String(error) });
      setClosingApp(false);
    }
  };

  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;

    const setup = async () => {
      try {
        const stop = await listen('request-start-recording', () => {
          if (isKnowledgeRef.current) { toast.info('请先退出企业知识服务中心，再开始会议'); return; }
          if (recordingActiveRef.current) {
            sessionStorage.removeItem('autoStartRecording');
            if (!isHomeRef.current) router.push('/');
            return;
          }
          if (isHomeRef.current) {
            sessionStorage.removeItem('autoStartRecording');
            window.dispatchEvent(new CustomEvent('start-recording-from-sidebar'));
            return;
          }
          sessionStorage.setItem('autoStartRecording', 'true');
          router.push('/');
        });
        if (cancelled) {
          stop();
          return;
        }
        unlisten = stop;
      } catch (error) {
        console.warn('[MainContent] Tray start listener unavailable:', error);
      }
    };

    void setup();
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [router]);

  return (
    <main className={`hyos-main${isKnowledge ? ' hyos-knowledge-main' : ''}${isHome && !recordingActive ? ' hyos-home-main' : ''}${isSettings || (isHome && recordingActive) ? ' hyos-chrome-less' : ''}${isHome && recordingActive ? ' hyos-live-main' : ''}`}>
      {!isSettings && !(isHome && recordingActive) && <header className="hyos-topbar">
        <button className="hyos-brand" onClick={() => isKnowledge ? window.dispatchEvent(new CustomEvent('knowledge-exit')) : router.push('/')} aria-label="返回工作台">
          <span>AI智能会议助手</span>
        </button>
        <div className={`hyos-topbar-actions${isHome && !recordingActive ? ' hyos-home-topbar-actions' : ''}`}>
          {recordingActive && !isHome && (
            <div className="hyos-recording-actions">
              <button onClick={() => router.push('/')}>
                <Play size={14} />返回会议
              </button>
            </div>
          )}
          <button className="hyos-settings-button" disabled={recordingActive || pathname === '/knowledge'} onClick={() => router.push('/knowledge')}><BrainCircuit size={16} />企业知识服务中心</button>
          <button className={`hyos-settings-button${isSettings ? ' active' : ''}`} onClick={openSettings} disabled={recordingActive || isKnowledge} title={recordingActive ? '会议进行中，请先结束会议' : '设置'}><Settings size={16} />设置</button>
          {isHome && !recordingActive && <button className="hyos-power-button" onClick={() => void closeApp()} disabled={closingApp}><Power size={16} />{closingApp ? '正在关闭' : '关闭应用'}</button>}
          {(!isHome || recordingActive) && <div className="hyos-user"><span className={isMeetingActive ? 'recording-dot' : 'status-dot'} />{isMeetingActive ? '会议进行中' : '已登录'}</div>}
        </div>
      </header>}
      <div className="hyos-content">{children}</div>

    </main>
  );
};

export default MainContent;
