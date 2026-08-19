'use client';

import React, { useEffect, useRef } from 'react';
import { useSidebar } from '@/components/Sidebar/SidebarProvider';
import { usePathname, useRouter } from 'next/navigation';
import { Settings, Play } from 'lucide-react';
import { useRecordingState } from '@/contexts/RecordingStateContext';
import { listen } from '@tauri-apps/api/event';
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
  const recordingActive = recordingState.isRecording || recordingState.isStopping || recordingState.isProcessing || recordingState.isSaving;
  const recordingActiveRef = useRef(recordingActive);
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

  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;

    const setup = async () => {
      try {
        const stop = await listen('request-start-recording', () => {
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
    <main className="hyos-main">
      <header className="hyos-topbar">
        <button className="hyos-brand" onClick={() => router.push('/')} aria-label="返回工作台">
          <span>智能会议助手</span>
        </button>
        <div className="hyos-topbar-actions">
          {recordingActive && !isHome && (
            <div className="hyos-recording-actions">
              <button onClick={() => router.push('/')}>
                <Play size={14} />返回会议
              </button>
            </div>
          )}
          <button className={`hyos-settings-button${isSettings ? ' active' : ''}`} onClick={openSettings} disabled={recordingActive} title={recordingActive ? '会议进行中，请先结束会议' : '设置'}><Settings size={16} />设置</button>
          <div className="hyos-user"><span className={isMeetingActive ? 'recording-dot' : 'status-dot'} />{isMeetingActive ? '会议进行中' : '已登录'}</div>
        </div>
      </header>
      <div className="hyos-content">{children}</div>
    </main>
  );
};

export default MainContent;
