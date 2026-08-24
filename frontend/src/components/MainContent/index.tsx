'use client';

import React, { useEffect, useRef, useState } from 'react';
import { useSidebar } from '@/components/Sidebar/SidebarProvider';
import { usePathname, useRouter } from 'next/navigation';
import { Settings, Play, Power, RotateCcw, Moon } from 'lucide-react';
import { useRecordingState } from '@/contexts/RecordingStateContext';
import { listen } from '@tauri-apps/api/event';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';

interface MainContentProps {
  children: React.ReactNode;
}

type PowerAction = 'reboot' | 'suspend' | 'poweroff';

const MainContent: React.FC<MainContentProps> = ({ children }) => {
  const router = useRouter();
  const pathname = usePathname();
  const { isMeetingActive } = useSidebar();
  const recordingState = useRecordingState();
  const isHome = pathname === '/';
  const isSettings = pathname === '/settings';
  const recordingActive = recordingState.isRecording || recordingState.isStopping || recordingState.isProcessing || recordingState.isSaving;
  const recordingActiveRef = useRef(recordingActive);
  const [powerMenuOpen, setPowerMenuOpen] = useState(false);
  const [pendingPowerAction, setPendingPowerAction] = useState<PowerAction | null>(null);
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

  const runPowerAction = async (action: PowerAction) => {
    setPendingPowerAction(action);
    try {
      await invoke('system_power_action', { action });
      setPowerMenuOpen(false);
    } catch (error) {
      toast.error('无法执行电源操作', { description: String(error) });
    } finally {
      setPendingPowerAction(null);
    }
  };

  useEffect(() => {
    if (!powerMenuOpen) return;
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === 'Escape' && !pendingPowerAction) setPowerMenuOpen(false);
    };
    window.addEventListener('keydown', closeOnEscape);
    return () => window.removeEventListener('keydown', closeOnEscape);
  }, [pendingPowerAction, powerMenuOpen]);

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
    <main className={`hyos-main${isHome && !recordingActive ? ' hyos-home-main' : ''}${isSettings || (isHome && recordingActive) ? ' hyos-chrome-less' : ''}${isHome && recordingActive ? ' hyos-live-main' : ''}`}>
      {!isSettings && !(isHome && recordingActive) && <header className="hyos-topbar">
        <button className="hyos-brand" onClick={() => router.push('/')} aria-label="返回工作台">
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
          <button className={`hyos-settings-button${isSettings ? ' active' : ''}`} onClick={openSettings} disabled={recordingActive} title={recordingActive ? '会议进行中，请先结束会议' : '设置'}><Settings size={16} />设置</button>
          {isHome && !recordingActive && <button className="hyos-power-button" onClick={() => setPowerMenuOpen(true)}><Power size={16} />关机</button>}
          {(!isHome || recordingActive) && <div className="hyos-user"><span className={isMeetingActive ? 'recording-dot' : 'status-dot'} />{isMeetingActive ? '会议进行中' : '已登录'}</div>}
        </div>
      </header>}
      <div className="hyos-content">{children}</div>
      {powerMenuOpen && (
        <div className="hyos-modal-backdrop hyos-power-backdrop" role="presentation" onMouseDown={(event) => event.target === event.currentTarget && !pendingPowerAction && setPowerMenuOpen(false)}>
          <section className="hyos-power-dialog" role="dialog" aria-modal="true" aria-labelledby="power-dialog-title" aria-describedby="power-dialog-description">
            <header><h2 id="power-dialog-title">电源选项</h2><p id="power-dialog-description">选择要执行的操作</p></header>
            <div className="hyos-power-options">
              <button onClick={() => void runPowerAction('reboot')} disabled={pendingPowerAction !== null}><span className="reboot"><RotateCcw size={31} /></span><b>{pendingPowerAction === 'reboot' ? '正在重启' : '重启'}</b></button>
              <button onClick={() => void runPowerAction('suspend')} disabled={pendingPowerAction !== null}><span className="suspend"><Moon size={31} /></span><b>{pendingPowerAction === 'suspend' ? '正在睡眠' : '睡眠'}</b></button>
              <button onClick={() => void runPowerAction('poweroff')} disabled={pendingPowerAction !== null}><span className="poweroff"><Power size={31} /></span><b>{pendingPowerAction === 'poweroff' ? '正在关机' : '关机'}</b></button>
            </div>
            <button className="hyos-power-cancel" onClick={() => setPowerMenuOpen(false)} disabled={pendingPowerAction !== null}>取消</button>
          </section>
        </div>
      )}
    </main>
  );
};

export default MainContent;
