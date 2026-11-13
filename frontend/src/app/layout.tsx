'use client'

import './globals.css'
import Sidebar from '@/components/Sidebar'
import { SidebarProvider, useSidebar } from '@/components/Sidebar/SidebarProvider'
import MainContent from '@/components/MainContent'
import AnalyticsProvider from '@/components/AnalyticsProvider'
import { toast, NoticeHost } from '@/lib/notice'
import { useState, useEffect, useCallback } from 'react'
import { usePathname } from 'next/navigation'
import { listen, UnlistenFn } from '@tauri-apps/api/event'
import { invoke } from '@tauri-apps/api/core'
import { TooltipProvider } from '@/components/ui/tooltip'
import { RecordingStateProvider } from '@/contexts/RecordingStateContext'
import { OllamaDownloadProvider } from '@/contexts/OllamaDownloadContext'
import { TranscriptProvider } from '@/contexts/TranscriptContext'
import { ConfigProvider, useConfig } from '@/contexts/ConfigContext'
import { OnboardingProvider } from '@/contexts/OnboardingContext'
import { OnboardingFlow } from '@/components/onboarding'
import { loadBetaFeatures } from '@/types/betaFeatures'
import { DownloadProgressToastProvider } from '@/components/shared/DownloadProgressToast'
import { RecordingPostProcessingProvider } from '@/contexts/RecordingPostProcessingProvider'
import { ImportAudioDialog, ImportDropOverlay } from '@/components/ImportAudio'
import { ImportDialogProvider } from '@/contexts/ImportDialogContext'
import { MenuBridge } from '@/components/MenuBridge'
import { FeedbackDialog } from '@/components/FeedbackDialog'
import { CalendarReminder } from '@/components/CalendarReminder'
import { MeetPrompt } from '@/components/MeetPrompt'
import { isAudioExtension, getAudioFormatsDisplayList } from '@/constants/audioFormats'
import { ThemeProvider, themeBootScript } from '@/contexts/ThemeContext'


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
  const { betaFeatures } = useConfig();

  // Only mount ImportAudioDialog (and its hooks/listeners) when feature is enabled
  if (!betaFeatures.importAndRetranscribe) {
    return null;
  }

  return (
    <ImportAudioDialog
      open={showImportDialog}
      onOpenChange={handleImportDialogClose}
      preselectedFile={importFilePath}
    />
  );
}

/** Notices sit over the content column, so they follow the sidebar width */
function NoticeLayer({ onboarding }: { onboarding: boolean }) {
  const { isCollapsed } = useSidebar();
  return <NoticeHost left={onboarding ? 0 : isCollapsed ? 90 : 228} />;
}

// export { metadata } from './metadata'

function MainLayout({
  children,
}: {
  children: React.ReactNode
}) {
  const [showOnboarding, setShowOnboarding] = useState(false)

  // The dictionary reads all past meetings once per launch (local, in the background)
  useEffect(() => {
    const timer = setTimeout(() => {
      invoke('api_learn_from_meetings').catch((error) => console.warn('Dictionary scan failed', error))
    }, 5000)
    return () => clearTimeout(timer)
  }, [])

  // Hide the window from screen sharing unless the user turned it off
  useEffect(() => {
    // Apply the saved choice only; nothing is written on start
    invoke<boolean>('api_get_hide_from_screen_share')
      .then((enabled) => (enabled ? invoke('api_set_hide_from_screen_share', { enabled }) : undefined))
      .catch((error) => console.warn('Screen share protection not applied', error))
  }, [])
  const [onboardingCompleted, setOnboardingCompleted] = useState(false)

  // Import audio state
  const [showDropOverlay, setShowDropOverlay] = useState(false)
  const [showImportDialog, setShowImportDialog] = useState(false)
  const [importFilePath, setImportFilePath] = useState<string | null>(null)


  useEffect(() => {
    // Check onboarding status first
    invoke<{ completed: boolean } | null>('get_onboarding_status')
      .then((status) => {
        const isComplete = status?.completed ?? false
        setOnboardingCompleted(isComplete)

        if (!isComplete) {
          console.log('[Layout] Onboarding not completed, showing onboarding flow')
          setShowOnboarding(true)
        } else {
          console.log('[Layout] Onboarding completed, showing main app')
        }
      })
      .catch((error) => {
        console.error('[Layout] Failed to check onboarding status:', error)
        // Default to showing onboarding if we can't check
        setShowOnboarding(true)
        setOnboardingCompleted(false)
      })
  }, [])

  // Disable context menu in production
  useEffect(() => {
    if (process.env.NODE_ENV === 'production') {
      const handleContextMenu = (e: MouseEvent) => e.preventDefault();
      document.addEventListener('contextmenu', handleContextMenu);
      return () => document.removeEventListener('contextmenu', handleContextMenu);
    }
  }, []);
  useEffect(() => {
    // Listen for tray recording toggle request
    const unlisten = listen('request-recording-toggle', () => {
      console.log('[Layout] Received request-recording-toggle from tray');

      if (showOnboarding) {
        toast.error("Спершу завершіть початкове налаштування", {
          description: "Запис стане доступним після початкового налаштування."
        });
      } else {
        // If in main app, forward to useRecordingStart via window event
        console.log('[Layout] Forwarding to start-recording-from-sidebar');
        window.dispatchEvent(new CustomEvent('start-recording-from-sidebar'));
      }
    });

    return () => {
      unlisten.then(fn => fn());
    };
  }, [showOnboarding]);

  // Handle file drop for audio import
  const handleFileDrop = useCallback((paths: string[]) => {
    // Check if beta features are enabled (read from localStorage directly since we're outside ConfigProvider)
    const betaFeatures = loadBetaFeatures();

    if (!betaFeatures.importAndRetranscribe) {
      toast.error('Імпорт вимкнено в налаштуваннях', {
        description: 'Увімкніть імпорт аудіо в Налаштуваннях.'
      });
      return;
    }

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
      toast.error('Перетягніть аудіофайл', {
        description: `Підтримувані формати: ${getAudioFormatsDisplayList()}`
      });
    }
  }, []);

  // Listen for drag-drop events
  useEffect(() => {
    if (showOnboarding) return; // Don't handle drops during onboarding

    const unlisteners: UnlistenFn[] = [];
    const cleanedUpRef = { current: false };

    const setupListeners = async () => {
      // Drag enter/over - show overlay only if beta feature is enabled
      const unlistenDragEnter = await listen('tauri://drag-enter', () => {
        if (loadBetaFeatures().importAndRetranscribe) {
          setShowDropOverlay(true);
        }
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
  }, [showOnboarding, handleFileDrop]);

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
  // The first start after an update says so once
  useEffect(() => {
    void import('@tauri-apps/api/app')
      .then(({ getVersion }) => getVersion())
      .then((v) => {
        let before: string | null = null
        try {
          before = localStorage.getItem('protokol.lastVersion')
          localStorage.setItem('protokol.lastVersion', v)
        } catch {
          return
        }
        if (before && before !== v) toast.success(`«Протокол» оновлено до версії ${v}`, { duration: 10000 })
      })
      .catch(() => {})
  }, [])
  // A newer version found by the daily check: told once, installed from Settings
  useEffect(() => {
    let unlisten: (() => void) | undefined
    void import('@tauri-apps/api/event').then(({ listen }) =>
      listen<{ version: string }>('update-available', (e) => {
        toast.info(`Є нова версія «Протоколу»: ${e.payload.version}`, {
          duration: 20000,
          action: { label: 'Відкрити', onClick: () => window.location.assign('/settings?section=general') },
        })
      }).then((u) => (unlisten = u)),
    )
    return () => unlisten?.()
  }, [])
  // Файл → Імпортувати аудіо… (⌘O) in the menu bar
  useEffect(() => {
    const open = () => handleOpenImportDialog(null);
    // Test runs only (the app started with PROTOKOL_TEST_IMPORT): a given file
    const openFile = (e: Event) => handleOpenImportDialog((e as CustomEvent<string>).detail);
    window.addEventListener('protokol:menu-import', open);
    window.addEventListener('protokol:test-import', openFile);
    // Test runs only: a notice of a given kind, to check where notices appear
    const testNotice = (e: Event) => {
      const { kind = 'success', title = '', description } = (e as CustomEvent<{ kind?: 'success' | 'info' | 'warning' | 'error'; title?: string; description?: string }>).detail ?? {};
      toast[kind](title, description ? { description } : undefined);
    };
    window.addEventListener('protokol:test-notice', testNotice);
    return () => {
      window.removeEventListener('protokol:menu-import', open);
      window.removeEventListener('protokol:test-import', openFile);
      window.removeEventListener('protokol:test-notice', testNotice);
    };
  }, [handleOpenImportDialog]);

  const handleOnboardingComplete = () => {
    console.log('[Layout] Onboarding completed, reloading app')
    setShowOnboarding(false)
    setOnboardingCompleted(true)
    // Optionally reload the window to ensure all state is fresh
    window.location.reload()
  }

  return (
    <html lang="uk" suppressHydrationWarning>
      <head>
        <script dangerouslySetInnerHTML={{ __html: themeBootScript }} />
      </head>
      <body className="font-sans antialiased">
        <ThemeProvider>
        <AnalyticsProvider>
          <RecordingStateProvider>
            <TranscriptProvider>
              <ConfigProvider>
                <OllamaDownloadProvider>
                  <OnboardingProvider>
                      <SidebarProvider>
                        <TooltipProvider>
                          <RecordingPostProcessingProvider>
                            <ImportDialogProvider onOpen={handleOpenImportDialog}>
                              {/* Download progress toast provider - listens for background downloads */}
                              <DownloadProgressToastProvider />
                              <NoticeLayer onboarding={showOnboarding} />
                              <MenuBridge />
                              <FeedbackDialog />

                              {/* Show onboarding or main app */}
                              {showOnboarding ? (
                                <OnboardingFlow onComplete={handleOnboardingComplete} />
                              ) : (
                                <div className="flex">
                                  {/* Invisible drag strip along the top edge of the window */}
                                  <div className="titlebar fixed top-0 left-0 right-0 h-3 z-50" data-tauri-drag-region />
                                  <div className="window-backdrop" aria-hidden="true" />
                                  <Sidebar />
                                  <MainContent>{children}</MainContent>
                                  <CalendarReminder />
                                  <MeetPrompt />
                                </div>
                              )}
                              {/* Import audio overlay and dialog */}
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
                  </OnboardingProvider>

                </OllamaDownloadProvider>
              </ConfigProvider>
            </TranscriptProvider>
          </RecordingStateProvider>
        </AnalyticsProvider>

        </ThemeProvider>
      </body>
    </html>
  )
}


/** The floating recording island and the Google Meet window's toolbar and
    offer are their own small webviews: no app chrome around them */
export default function RootLayout({ children }: { children: React.ReactNode }) {
  const pathname = usePathname()
  if (pathname?.startsWith('/island') || pathname?.startsWith('/meet-bar') || pathname?.startsWith('/meet-pop')) {
    return (
      <html lang="uk" suppressHydrationWarning>
        <head>
          <script dangerouslySetInnerHTML={{ __html: themeBootScript }} />
        </head>
        <body className="island-body font-sans antialiased">
          <ThemeProvider>{children}</ThemeProvider>
        </body>
      </html>
    )
  }
  return <MainLayout>{children}</MainLayout>
}
