'use client';

import React, { useEffect, useState, useCallback } from 'react';
import { listen } from '@tauri-apps/api/event';
import { toast } from '@/lib/notice';
import { getDownloadTotalMb, getSummaryModelDisplayName } from '@/lib/onboarding-summary-model';
import type { ParakeetDownloadProgressEvent } from '@/lib/parakeet';

interface DownloadProgress {
  modelName: string;
  displayName: string;
  progress: number;
  downloadedMb: number;
  totalMb: number;
  speedMbps: number;
  status: 'downloading' | 'completed' | 'error' | 'cancelled';
  unitLabel?: string;
  error?: string;
}

// Categorize error messages for better user experience
function categorizeError(error: string): string {
  const lowerError = error.toLowerCase();

  if (lowerError.includes('network') ||
    lowerError.includes('connection') ||
    lowerError.includes('timeout') ||
    lowerError.includes('failed to start download')) {
    return 'Помилка мережі: перевірте підключення до інтернету';
  }

  if (lowerError.includes('status:') || lowerError.includes('http')) {
    return 'Помилка сервера: завантаження тимчасово недоступне';
  }

  if (lowerError.includes('disk') ||
    lowerError.includes('write') ||
    lowerError.includes('file')) {
    return 'Помилка запису: перевірте вільне місце на диску';
  }

  if (lowerError.includes('invalid') || lowerError.includes('validation')) {
    return 'Файл пошкоджено: спробуйте завантажити ще раз';
  }

  // Fallback to original error
  return error;
}

/** «420 МБ», «1,2 ГБ» */
function formatSize(mb: number): string {
  if (mb >= 1000) return `${(mb / 1000).toFixed(1).replace('.', ',')} ГБ`;
  return `${Math.round(mb)} МБ`;
}

function DownloadIcon({ state }: { state: 'downloading' | 'completed' | 'error' | 'cancelled' }) {
  const common = { width: 16, height: 16, viewBox: '0 0 16 16', fill: 'none', stroke: 'currentColor', strokeWidth: 1.6, strokeLinecap: 'round' as const, strokeLinejoin: 'round' as const, 'aria-hidden': true };
  if (state === 'completed') {
    return <svg {...common}><path d="M3.5 8.5l3 3 6-7" /></svg>;
  }
  if (state === 'error' || state === 'cancelled') {
    return <svg {...common}><path d="M4.5 4.5l7 7M11.5 4.5l-7 7" /></svg>;
  }
  return <svg {...common}><path d="M8 2.5v8M4.5 7.5L8 11l3.5-3.5M3 13.5h10" /></svg>;
}

/** A model download inside the glass notice: name, one line of status, a thin bar */
function DownloadToastContent({
  download,
  onDismiss,
}: {
  download: DownloadProgress;
  onDismiss?: () => void;
}) {
  const isComplete = download.status === 'completed';
  const hasError = download.status === 'error';
  const isCancelled = download.status === 'cancelled';
  const isDownloading = !isComplete && !hasError && !isCancelled;
  const unitLabel = download.unitLabel ?? 'МБ';
  const sizes = unitLabel === 'МБ'
    ? `${formatSize(download.downloadedMb)} з ${formatSize(download.totalMb)}`
    : `${download.downloadedMb.toFixed(1)} з ${download.totalMb.toFixed(1)} ${unitLabel}`;
  const speed = download.speedMbps > 0 ? ` · ${download.speedMbps.toFixed(1).replace('.', ',')} ${unitLabel}/с` : '';

  const line = hasError
    ? download.error || 'Не вдалося завантажити'
    : isComplete
      ? 'Завантажено'
      : isCancelled
        ? 'Завантаження скасовано'
        : `${sizes}${speed} · ${Math.round(download.progress)}%`;

  return (
    <div className="download-notice" role={hasError ? 'alert' : 'status'}>
      <div className="notice-row">
        <span className={`notice-icon ${hasError ? 'notice-icon-error' : isCancelled ? 'notice-icon-muted' : ''}`}>
          <DownloadIcon state={download.status} />
        </span>
        <div className="notice-text">
          <span className="notice-title">{download.displayName}</span>
          <span className="notice-line">{line}</span>
        </div>
        {isDownloading && onDismiss ? (
          <button type="button" className="notice-close" aria-label="Сховати" title="Сховати, завантаження продовжиться" onClick={onDismiss}>
            <svg width="10" height="10" viewBox="0 0 10 10" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" aria-hidden="true"><path d="M1.5 1.5l7 7M8.5 1.5l-7 7" /></svg>
          </button>
        ) : null}
      </div>
      {isDownloading ? (
        <div className="download-notice-bar" aria-hidden="true">
          <div style={{ width: `${Math.max(2, Math.min(100, download.progress))}%` }} />
        </div>
      ) : null}
    </div>
  );
}

// Hook to manage download progress toasts
export function useDownloadProgressToast() {
  const [downloads, setDownloads] = useState<Map<string, DownloadProgress>>(new Map());
  const [dismissedModels, setDismissedModels] = useState<Set<string>>(new Set());

  const updateDownload = useCallback((modelName: string, data: Partial<DownloadProgress>) => {
    setDownloads((prev) => {
      const updated = new Map(prev);
      const existing = updated.get(modelName) || {
        modelName,
        displayName: modelName,
        progress: 0,
        downloadedMb: 0,
        totalMb: 0,
        speedMbps: 0,
        status: 'downloading' as const,
      };

      updated.set(modelName, { ...existing, ...data });
      return updated;
    });
  }, []);

  const cleanupDownload = useCallback((modelName: string, delay: number = 4000) => {
    // Remove download from map after delay (allows toast to show and auto-dismiss)
    setTimeout(() => {
      setDownloads((prev) => {
        const updated = new Map(prev);
        updated.delete(modelName);
        return updated;
      });
    }, delay);
  }, []);

  const showDownloadToast = useCallback((download: DownloadProgress) => {
    const toastId = `download-${download.modelName}`;

    // Determine duration based on status
    const getDuration = () => {
      switch (download.status) {
        case 'completed': return 3000;      // 3 seconds
        case 'cancelled': return 5000;      // 5 seconds
        case 'error': return 10000;         // 10 seconds
        case 'downloading': return Infinity; // Manual dismiss only
      }
    };

    // Dismiss handler
    const dismissToast = () => {
      toast.dismiss(toastId);
      setDismissedModels(prev => {
        const next = new Set(prev);
        next.add(download.modelName);
        return next;
      });
    };

    toast.custom(
      (t) => (
        <DownloadToastContent
          download={download}
          onDismiss={dismissToast}
        />
      ),
      {
        position: 'top-right',
        id: toastId,
        duration: getDuration(),
      }
    );
  }, []);

  // Effect to handle toast visibility based on dismissed state
  useEffect(() => {
    downloads.forEach((download) => {
      // If model was dismissed and is still downloading, don't show it
      if (dismissedModels.has(download.modelName) && download.status === 'downloading') {
        return;
      }

      // If status changed to completed or error, we might want to show it even if dismissed previously
      // (Optional: remove from dismissed set if you want to force show completion)
      if (download.status === 'completed' || download.status === 'error') {
        if (dismissedModels.has(download.modelName)) {
          // Remove from dismissed so we can show the completion/error toast
          setDismissedModels(prev => {
            const next = new Set(prev);
            next.delete(download.modelName);
            return next;
          });
        }
      }

      showDownloadToast(download);
    });
  }, [downloads, dismissedModels, showDownloadToast]);

  // Listen to Parakeet download events
  useEffect(() => {
    const unlistenProgress = listen<ParakeetDownloadProgressEvent>(
      'parakeet-model-download-progress',
      (event) => {
        const { modelName, progress, downloaded_mb, total_mb, speed_mbps, status } = event.payload;
        const downloadData: DownloadProgress = {
          modelName,
          displayName: 'Модель розпізнавання мовлення',
          progress,
          downloadedMb: downloaded_mb ?? 0,
          totalMb: total_mb ?? 670,
          speedMbps: speed_mbps ?? 0,
          status: status === 'cancelled'
            ? 'cancelled'
            : status === 'completed'
              ? 'completed'
              : 'downloading',
        };

        updateDownload(modelName, downloadData);
        if (downloadData.status === 'cancelled') {
          cleanupDownload(modelName, 6000);
        }
      }
    );

    const unlistenComplete = listen<{ modelName: string }>(
      'parakeet-model-download-complete',
      (event) => {
        const { modelName } = event.payload;
        const downloadData: DownloadProgress = {
          modelName,
          displayName: 'Модель розпізнавання мовлення',
          progress: 100,
          downloadedMb: 670,
          totalMb: 670,
          speedMbps: 0,
          status: 'completed',
        };
        updateDownload(modelName, downloadData);
        // Clean up after 4 seconds (completion toast duration is 3s + 1s buffer)
        cleanupDownload(modelName, 4000);
      }
    );

    const unlistenError = listen<{ modelName: string; error: string }>(
      'parakeet-model-download-error',
      (event) => {
        const { modelName, error } = event.payload;
        const downloadData: DownloadProgress = {
          modelName,
          displayName: 'Модель розпізнавання мовлення',
          progress: 0,
          downloadedMb: 0,
          totalMb: 670,
          speedMbps: 0,
          status: 'error',
          error: categorizeError(error),
        };
        updateDownload(modelName, downloadData);
        // Clean up after 11 seconds (error toast duration is 10s + 1s buffer)
        cleanupDownload(modelName, 11000);
      }
    );

    return () => {
      unlistenProgress.then((fn) => fn());
      unlistenComplete.then((fn) => fn());
      unlistenError.then((fn) => fn());
    };
  }, [updateDownload, cleanupDownload]);

  // Listen to Built-in AI summary model download events
  useEffect(() => {
    const unlisten = listen<{
      model: string;
      progress: number;
      downloaded_mb?: number;
      total_mb?: number;
      speed_mbps?: number;
      status: string;
      error?: string;
    }>('builtin-ai-download-progress', (event) => {
      const { model, progress, downloaded_mb, total_mb, speed_mbps, status, error } = event.payload;

      const downloadData: DownloadProgress = {
        modelName: model,
        displayName: `Модель підсумку (${getSummaryModelDisplayName(model)})`,
        progress: progress ?? 0,
        downloadedMb: downloaded_mb ?? 0,
        totalMb: getDownloadTotalMb(total_mb, model),
        speedMbps: speed_mbps ?? 0,
        unitLabel: 'МБ',
        status: status === 'completed' || progress >= 100
          ? 'completed'
          : status === 'cancelled'
            ? 'cancelled'
            : status === 'error'
              ? 'error'
              : 'downloading',
        error: status === 'error' ? categorizeError(error || 'Не вдалося завантажити') : undefined,
      };

      updateDownload(model, downloadData);

      // Clean up finished downloads after delay to prevent endless toasts
      if (downloadData.status === 'completed') {
        cleanupDownload(model, 4000);  // 3s toast + 1s buffer
      } else if (downloadData.status === 'error') {
        cleanupDownload(model, 11000); // 10s toast + 1s buffer
      } else if (downloadData.status === 'cancelled') {
        cleanupDownload(model, 6000);  // 5s toast + 1s buffer
      }
    });

    return () => {
      unlisten.then((fn) => fn());
    };
  }, [updateDownload, cleanupDownload]);

  return { downloads };
}

// Component to initialize download toast listeners at app level
export function DownloadProgressToastProvider() {
  useDownloadProgressToast();
  return null;
}
