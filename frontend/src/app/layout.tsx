'use client'

import './globals.css'
import { Source_Sans_3 } from 'next/font/google'
import { SidebarProvider } from '@/components/Sidebar/SidebarProvider'
import MainContent from '@/components/MainContent'
import AnalyticsProvider from '@/components/AnalyticsProvider'
import { Toaster, toast } from 'sonner'
import "sonner/dist/styles.css"
import { useState, useEffect, useCallback } from 'react'
import { listen, UnlistenFn } from '@tauri-apps/api/event'
import { TooltipProvider } from '@/components/ui/tooltip'
import { RecordingStateProvider } from '@/contexts/RecordingStateContext'
import { OllamaDownloadProvider } from '@/contexts/OllamaDownloadContext'
import { TranscriptProvider } from '@/contexts/TranscriptContext'
import { ConfigProvider } from '@/contexts/ConfigContext'
import { OnboardingProvider } from '@/contexts/OnboardingContext'
import { DownloadProgressToastProvider } from '@/components/shared/DownloadProgressToast'
import { UpdateCheckProvider } from '@/components/UpdateCheckProvider'
import { RecordingPostProcessingProvider } from '@/contexts/RecordingPostProcessingProvider'
import { ImportAudioDialog, ImportDropOverlay } from '@/components/ImportAudio'
import { ImportDialogProvider } from '@/contexts/ImportDialogContext'
import { isAudioExtension, getAudioFormatsDisplayList } from '@/constants/audioFormats'
import { uiText } from '@/lib/uiText'
import { HYOSAccessGate } from '@/components/HYOSAccessGate'


const sourceSans3 = Source_Sans_3({
  subsets: ['latin'],
  weight: ['400', '500', '600', '700'],
  variable: '--font-source-sans-3',
})

// Module-level component — stable reference across RootLayout re-renders.
// Defined here (not inside RootLayout) so React never sees a new function type
// on re-render, which would cause unmount/remount and break initialization logic.
function ConditionalImportDialog({
  showImportDialog,
  handleImportDialogClose,
  importFilePath,
}: {
  showImportDialog: boolean;
  handleImportDialogClose: (open: boolean) => void;
  importFilePath: string | null;
}) {
  return (
    <ImportAudioDialog
      open={showImportDialog}
      onOpenChange={handleImportDialogClose}
      preselectedFile={importFilePath}
    />
  );
}

// export { metadata } from './metadata'

function AuthenticatedApplication({
  children,
}: {
  children: React.ReactNode
}) {
  // Import audio state
  const [showDropOverlay, setShowDropOverlay] = useState(false)
  const [showImportDialog, setShowImportDialog] = useState(false)
  const [importFilePath, setImportFilePath] = useState<string | null>(null)

  // Disable context menu in production
  useEffect(() => {
    if (process.env.NODE_ENV === 'production') {
      const handleContextMenu = (e: MouseEvent) => e.preventDefault();
      document.addEventListener('contextmenu', handleContextMenu);
      return () => document.removeEventListener('contextmenu', handleContextMenu);
    }
  }, []);
  useEffect(() => {
    let cancelled = false
    import('@tauri-apps/api/window').then(({ getCurrentWindow }) => {
      if (cancelled) return
      getCurrentWindow().setTitle(uiText.appName).catch(() => {})
    }).catch(() => {})
    return () => { cancelled = true }
  }, [])
  useEffect(() => {
    // Listen for tray recording toggle request
    const unlisten = listen('request-recording-toggle', () => {
      console.log('[Layout] Received request-recording-toggle from tray');

      // RK distributions enter the application directly. Backend availability is
      // reported by the configured inference provider, not by model onboarding.
      console.log('[Layout] Forwarding to start-recording-from-sidebar');
      window.dispatchEvent(new CustomEvent('start-recording-from-sidebar'));
    });

    return () => {
      unlisten.then(fn => fn());
    };
  }, []);

  // Handle file drop for audio import
  const handleFileDrop = useCallback((paths: string[]) => {
    // Find the first audio file
    const audioFile = paths.find(p => {
      const ext = p.split('.').pop()?.toLowerCase();
      return !!ext && isAudioExtension(ext);
    });

    if (audioFile) {
      console.log('[Layout] Audio file dropped:', audioFile);
      setImportFilePath(audioFile);
      setShowImportDialog(true);
    } else if (paths.length > 0) {
      toast.error(uiText.dropAudioFile, {
        description: `支持格式：${getAudioFormatsDisplayList()}`
      });
    }
  }, []);

  // Listen for drag-drop events
  useEffect(() => {
    const unlisteners: UnlistenFn[] = [];
    const cleanedUpRef = { current: false };

    const setupListeners = async () => {
      // Drag enter/over - show overlay only if beta feature is enabled
      const unlistenDragEnter = await listen('tauri://drag-enter', () => {
        setShowDropOverlay(true);
      });
      if (cleanedUpRef.current) {
        unlistenDragEnter();
        return;
      }
      unlisteners.push(unlistenDragEnter);

      // Drag leave - hide overlay
      const unlistenDragLeave = await listen('tauri://drag-leave', () => {
        setShowDropOverlay(false);
      });
      if (cleanedUpRef.current) {
        unlistenDragLeave();
        unlisteners.forEach(u => u());
        return;
      }
      unlisteners.push(unlistenDragLeave);

      // Drop - process files
      const unlistenDrop = await listen<{ paths: string[] }>('tauri://drag-drop', (event) => {
        setShowDropOverlay(false);
        handleFileDrop(event.payload.paths);
      });
      if (cleanedUpRef.current) {
        unlistenDrop();
        unlisteners.forEach(u => u());
        return;
      }
      unlisteners.push(unlistenDrop);
    };

    setupListeners();

    return () => {
      cleanedUpRef.current = true;
      unlisteners.forEach((unlisten) => unlisten());
    };
  }, [handleFileDrop]);

  // Handle import dialog close
  const handleImportDialogClose = useCallback((open: boolean) => {
    setShowImportDialog(open);
    if (!open) {
      setImportFilePath(null);
    }
  }, []);

  // Handler for ImportDialogProvider - opens import dialog from any child component
  const handleOpenImportDialog = useCallback((filePath?: string | null) => {
    setImportFilePath(filePath ?? null);
    setShowImportDialog(true);
  }, []);

  return (
    <AnalyticsProvider>
      <RecordingStateProvider>
        <TranscriptProvider>
          <ConfigProvider>
            <OllamaDownloadProvider>
              <OnboardingProvider>
                <UpdateCheckProvider>
                  <SidebarProvider>
                    <TooltipProvider>
                      <RecordingPostProcessingProvider>
                        <ImportDialogProvider onOpen={handleOpenImportDialog}>
                          <DownloadProgressToastProvider />
                          <MainContent>{children}</MainContent>
                          <ImportDropOverlay visible={showDropOverlay} />
                          <ConditionalImportDialog
                            showImportDialog={showImportDialog}
                            handleImportDialogClose={handleImportDialogClose}
                            importFilePath={importFilePath}
                          />
                        </ImportDialogProvider>
                      </RecordingPostProcessingProvider>
                    </TooltipProvider>
                  </SidebarProvider>
                </UpdateCheckProvider>
              </OnboardingProvider>
            </OllamaDownloadProvider>
          </ConfigProvider>
        </TranscriptProvider>
      </RecordingStateProvider>
    </AnalyticsProvider>
  )
}

export default function RootLayout({
  children,
}: {
  children: React.ReactNode
}) {
  return (
    <html lang="zh-CN">
      <body className={`${sourceSans3.variable} font-sans antialiased`}>
        <HYOSAccessGate>
          <AuthenticatedApplication>{children}</AuthenticatedApplication>
        </HYOSAccessGate>
        <Toaster position="bottom-center" richColors closeButton />
      </body>
    </html>
  )
}
