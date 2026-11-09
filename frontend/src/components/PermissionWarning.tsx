import React from 'react';
import { AlertTriangle, Mic, Speaker, RefreshCw } from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import { useIsLinux } from '@/hooks/usePlatform';

interface PermissionWarningProps {
  hasMicrophone: boolean;
  hasSystemAudio: boolean;
  onRecheck: () => void;
  isRechecking?: boolean;
}

export function PermissionWarning({
  hasMicrophone,
  hasSystemAudio,
  onRecheck,
  isRechecking = false
}: PermissionWarningProps) {
  const isLinux = useIsLinux();

  // Don't show on Linux - permission handling is not needed
  if (isLinux) {
    return null;
  }

  // Don't show if both permissions are granted
  if (hasMicrophone && hasSystemAudio) {
    return null;
  }

  const isMacOS = navigator.userAgent.includes('Mac');

  const openMicrophoneSettings = async () => {
    if (isMacOS) {
      try {
        await invoke('open_system_settings', { preferencePane: 'Privacy_Microphone' });
      } catch (error) {
        console.error('Failed to open microphone settings:', error);
      }
    }
  };

  const openScreenRecordingSettings = async () => {
    if (isMacOS) {
      try {
        await invoke('open_system_settings', { preferencePane: 'Privacy_ScreenCapture' });
      } catch (error) {
        console.error('Failed to open screen recording settings:', error);
      }
    }
  };

  const title = !hasMicrophone && !hasSystemAudio
    ? 'Потрібні дозволи на мікрофон і запис екрана'
    : !hasMicrophone
      ? 'Потрібен дозвіл на мікрофон'
      : 'Звук дзвінка не записуватиметься';

  return (
    <section aria-label="Дозволи" className="card px-5 py-4 flex flex-col gap-3">
      <div className="flex items-start gap-3">
        <AlertTriangle className="h-5 w-5 flex-none mt-0.5 text-[color:var(--muted-text)]" aria-hidden="true" />
        <div className="flex flex-col gap-1 min-w-0">
          <h2 className="text-[15px] font-bold text-gray-900">{title}</h2>
          <p className="text-sm leading-[1.5] text-[color:var(--muted-text)]">
            {!hasMicrophone
              ? 'Без мікрофона запис не вийде. Перевірте, що мікрофон підключений і застосунку дозволено ним користуватися.'
              : 'Мікрофон працює, але звук з комп\'ютера (голоси учасників дзвінка) недоступний. Для нього потрібен дозвіл «Запис екрана й аудіо системи».'}
          </p>
        </div>
      </div>
      <div className="flex flex-wrap gap-2 pl-8">
        {isMacOS && !hasMicrophone && (
          <button type="button" onClick={openMicrophoneSettings} className="secondary-button h-9 px-3.5 gap-2 text-[13px] font-semibold">
            <Mic className="h-4 w-4" />
            Налаштування мікрофона
          </button>
        )}
        {isMacOS && !hasSystemAudio && (
          <button type="button" onClick={openScreenRecordingSettings} className="secondary-button h-9 px-3.5 gap-2 text-[13px] font-semibold">
            <Speaker className="h-4 w-4" />
            Дозвіл на запис екрана
          </button>
        )}
        <button
          type="button"
          onClick={onRecheck}
          disabled={isRechecking}
          className="secondary-button h-9 px-3.5 gap-2 text-[13px] font-semibold disabled:opacity-50"
        >
          <RefreshCw className={`h-4 w-4 ${isRechecking ? 'animate-spin' : ''}`} />
          Перевірити знову
        </button>
      </div>
    </section>
  );
}
