'use client';

import React, { useEffect, useRef, useState } from 'react';
import { useSidebar } from '@/components/Sidebar/SidebarProvider';
import { usePathname, useRouter } from 'next/navigation';
import { Settings, Play, Power, RotateCcw } from 'lucide-react';
import { RecordingStatus, useRecordingState } from '@/contexts/RecordingStateContext';
import { useVoiceWake } from '@/hooks/useVoiceWake';
import { VoiceWakeControl } from './VoiceWakeControl';
import { listen } from '@tauri-apps/api/event';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';

interface MainContentProps {
  children: React.ReactNode;
}

type PowerAction = 'reboot' | 'poweroff';

const MainContent: React.FC<MainContentProps> = ({ children }) => {
  const router = useRouter();
  const pathname = usePathname();
  const { isMeetingActive } = useSidebar();
  const recordingState = useRecordingState();
  const isHome = pathname === '/';
  const isSettings = pathname === '/settings';
  const recordingActive = recordingState.status === RecordingStatus.STARTING || recordingState.isRecording || recordingState.isStopping || recordingState.isProcessing || recordingState.isSaving;
  const voiceWake = useVoiceWake(recordingActive);
  const recordingActiveRef = useRef(recordingActive);
  const [powerMenuOpen, setPowerMenuOpen] = useState(false);
  const [pendingPowerAction, setPendingPowerAction] = useState<PowerAction | null>(null);
  const isHomeRef = useRef(isHome);
  const isMeetingActiveRef = useRef(isMeetingActive);
  recordingActiveRef.current = recordingActive;
  isHomeRef.current = isHome;
  isMeetingActiveRef.current = isMeetingActive;

  const openSettings = () => {
    if (recordingActive) {
      toast.info('会议进行中，请先结束会议');
      return;
    }
    router.push('/settings');
  };

  const runPowerAction = async (action: PowerAction) => {
    if (recordingActiveRef.current) {
      toast.info('会议录制或保存中，请等待完成后再执行电源操作');
      return;
    }
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
    const unlisteners: (() => void)[] = [];
    let controlsReady = false;
    let pendingStop: { session_id: string } | null = null;
    let stopTimeout: ReturnType<typeof setTimeout> | null = null;

    const clearPendingStop = () => {
      pendingStop = null;
      if (stopTimeout !== null) clearTimeout(stopTimeout);
      stopTimeout = null;
    };
    const deliverPendingStop = () => {
      if (cancelled || !isHomeRef.current || !controlsReady || !pendingStop) return;
      const request = pendingStop;
      clearPendingStop();
      window.dispatchEvent(new CustomEvent('end-meeting-by-voice', { detail: request }));
    };
    const handleControlsReady = (event: Event) => {
      controlsReady = (event as CustomEvent<boolean>).detail === true;
      deliverPendingStop();
    };

    // Readiness comes from the child after its voice-stop listener is installed
    // and the live view (recording state) is mounted. A parent render is not
    // sufficient, so a pending stop cannot fire before the bridge exists.
    window.addEventListener('recording-controls-ready', handleControlsReady);
    window.dispatchEvent(new CustomEvent('request-recording-controls-ready'));
    // Legacy latch from the previous route handoff; the stop intent now lives in
    // memory only, scoped to the native session id.
    sessionStorage.removeItem('autoStopRecording');

    const setup = async () => {
      try {
        const start = await listen('request-start-recording', () => {
          if (cancelled) return;
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
          if (sessionStorage.getItem('autoStartRecording') === 'true') return;
          sessionStorage.setItem('autoStartRecording', 'true');
          router.push('/');
        });
        if (cancelled) {
          start();
          return;
        }
        unlisteners.push(start);
      } catch (error) {
        console.warn('[MainContent] Tray start listener unavailable:', error);
      }
      try {
        // The speaker already confirmed the stop command, so no dialog is
        // raised; RecordingControls runs the existing save flow directly.
        const stopRecording = await listen<{ session_id: string }>('request-stop-recording', ({ payload }) => {
          if (cancelled || typeof payload?.session_id !== 'string' || !payload.session_id) return;
          // Deduplicate only the queued request; a deliberate later wake+stop
          // for the same running meeting must still be honored.
          if (pendingStop?.session_id === payload.session_id) return;
          // Without a live meeting session there is nothing to end; a stale
          // event must not navigate or stop a later recording.
          if (!recordingActiveRef.current && !isMeetingActiveRef.current) return;
          clearPendingStop();
          pendingStop = { session_id: payload.session_id };
          // Pending state is dropped after a short window so it can never
          // outlive the request and affect a different meeting.
          stopTimeout = setTimeout(clearPendingStop, 8000);
          if (!isHomeRef.current) router.push('/');
          deliverPendingStop();
        });
        if (cancelled) {
          stopRecording();
          return;
        }
        unlisteners.push(stopRecording);
      } catch (error) {
        console.warn('[MainContent] Voice stop listener unavailable:', error);
      }
    };

    void setup();
    return () => {
      cancelled = true;
      clearPendingStop();
      window.removeEventListener('recording-controls-ready', handleControlsReady);
      unlisteners.forEach((unlisten) => unlisten());
    };
  }, [router]);

  return (
    <main className={`hyos-main${isHome && !recordingActive ? ' hyos-home-main' : ''}${isSettings || (isHome && recordingActive) ? ' hyos-chrome-less' : ''}${isHome && recordingActive ? ' hyos-live-main' : ''}`}>
      {!isSettings && !(isHome && recordingActive) && <header className="hyos-topbar">
        <button className="hyos-brand" onClick={() => router.push('/')} aria-label="返回工作台">
          <span>AI智能会议助手</span>
        </button>
        <div className={`hyos-topbar-actions${isHome && !recordingActive ? ' hyos-home-topbar-actions' : ''}`}>
          {!recordingActive && <VoiceWakeControl {...voiceWake} />}
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
              <button onClick={() => void runPowerAction('reboot')} disabled={pendingPowerAction !== null || recordingActive}><span className="reboot"><RotateCcw size={31} /></span><b>{pendingPowerAction === 'reboot' ? '正在重启' : '重启'}</b></button>
              <button onClick={() => void runPowerAction('poweroff')} disabled={pendingPowerAction !== null || recordingActive}><span className="poweroff"><Power size={31} /></span><b>{pendingPowerAction === 'poweroff' ? '正在关机' : '关机'}</b></button>
            </div>
            <button className="hyos-power-cancel" onClick={() => setPowerMenuOpen(false)} disabled={pendingPowerAction !== null}>取消</button>
          </section>
        </div>
      )}
    </main>
  );
};

export default MainContent;
