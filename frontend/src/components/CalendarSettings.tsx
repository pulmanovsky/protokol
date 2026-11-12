'use client';

import { useCallback, useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { CalendarDays, RefreshCw, Video } from 'lucide-react';
import { SettingsGroup, SettingsRow, SettingsBlock } from '@/components/settings/SettingsGroup';
import { toast } from '@/lib/notice';
import { Switch } from '@/components/ui/switch';
import { CalendarEvent, formatTime } from '@/components/CalendarReminder';

interface CalendarSettingsData {
  enabled: boolean;
  ics_url: string | null;
  remind_minutes_before: number;
  bring_window_to_front: boolean;
}

interface CalendarStatus {
  settings: CalendarSettingsData;
  has_imported_file: boolean;
  last_refresh_at: string | null;
  last_error: string | null;
  upcoming: CalendarEvent[];
}

const REMIND_OPTIONS = [0, 1, 2, 5, 10];

function errorText(error: unknown): string {
  return typeof error === 'string' ? error : error instanceof Error ? error.message : String(error);
}

function dayLabel(iso: string): string {
  const date = new Date(iso);
  const today = new Date();
  const tomorrow = new Date();
  tomorrow.setDate(today.getDate() + 1);
  if (date.toDateString() === today.toDateString()) return 'сьогодні';
  if (date.toDateString() === tomorrow.toDateString()) return 'завтра';
  return date.toLocaleDateString('uk-UA', { day: 'numeric', month: 'long' });
}

/**
 * Calendar reminders: the app reads the user's calendar and, before each
 * meeting, asks whether to start recording.
 */
export function CalendarSettings() {
  const [status, setStatus] = useState<CalendarStatus | null>(null);
  const [url, setUrl] = useState('');
  const [busy, setBusy] = useState(false);
  const [pickedFileName, setPickedFileName] = useState<string | null>(null);

  const apply = useCallback((next: CalendarStatus) => {
    setStatus(next);
    setUrl(next.settings.ics_url ?? '');
  }, []);

  useEffect(() => {
    invoke<CalendarStatus>('api_calendar_status')
      .then(apply)
      .catch((error) => console.error('Failed to load calendar settings', error));
  }, [apply]);

  const save = async (patch: Partial<CalendarSettingsData>, successMessage?: string) => {
    if (!status) return;
    setBusy(true);
    try {
      const next = await invoke<CalendarStatus>('api_calendar_save_settings', {
        settings: { ...status.settings, ics_url: url.trim() || null, ...patch },
      });
      apply(next);
      if (successMessage) toast.success(successMessage);
    } catch (error) {
      toast.error('Не вдалося оновити календар', { description: errorText(error) });
      invoke<CalendarStatus>('api_calendar_status').then(setStatus).catch(() => {});
    } finally {
      setBusy(false);
    }
  };

  const importFile = async (file: File | undefined) => {
    if (!file) return;
    setBusy(true);
    try {
      const bytes = Array.from(new Uint8Array(await file.arrayBuffer()));
      apply(await invoke<CalendarStatus>('api_calendar_import_file', { fileBytes: bytes }));
      toast.success('Календар з файлу додано');
    } catch (error) {
      toast.error('Не вдалося прочитати файл', { description: errorText(error) });
    } finally {
      setBusy(false);
    }
  };

  const removeFile = async () => {
    try {
      apply(await invoke<CalendarStatus>('api_calendar_remove_file'));
    } catch (error) {
      toast.error('Не вдалося видалити файл', { description: errorText(error) });
    }
  };

  const refresh = async () => {
    setBusy(true);
    try {
      apply(await invoke<CalendarStatus>('api_calendar_refresh'));
    } catch (error) {
      toast.error('Не вдалося оновити календар', { description: errorText(error) });
      invoke<CalendarStatus>('api_calendar_status').then(setStatus).catch(() => {});
    } finally {
      setBusy(false);
    }
  };

  if (!status) return null;
  const { settings } = status;

  const configured = !!settings.ics_url || status.has_imported_file;

  return (
    <div className="flex flex-col gap-6">
      <SettingsGroup
        title="Джерело календаря"
        help={
          <details>
            <summary className="cursor-pointer">Де взяти посилання</summary>
            <ul className="mt-1.5 list-disc pl-5 space-y-1">
              <li>
                Google Calendar: calendar.google.com → Налаштування → ваш календар → «Інтеграція календаря» → «Секретна
                адреса у форматі iCal».
              </li>
              <li>Outlook / Microsoft 365: Налаштування → Календар → «Спільні календарі» → «Опублікувати календар» → посилання ICS.</li>
              <li>Посилання дає доступ до вашого календаря, тож нікому його не пересилайте. Воно зберігається лише на цьому комп’ютері.</li>
            </ul>
          </details>
        }
      >
        <SettingsBlock>
          <label htmlFor="ics-url" className="block text-sm mb-2">Посилання на календар (iCal)</label>
          <div className="flex gap-2">
            <input
              id="ics-url"
              type="text"
              value={url}
              onChange={(e) => setUrl(e.target.value)}
              placeholder="https://calendar.google.com/calendar/ical/…/basic.ics"
              className="flex-1 min-w-0 h-8 px-2.5 text-[13px] font-mono"
            />
            <button type="button" onClick={() => save({}, 'Посилання збережено')} disabled={busy} className="settings-button disabled:opacity-50">
              Зберегти
            </button>
          </div>
        </SettingsBlock>
        <SettingsRow
          label={
            <span className="flex flex-col min-w-0">
              <span>Файл календаря (.ics), без інтернету</span>
              <span className="text-xs text-[color:var(--muted-text)] truncate">
                {pickedFileName ?? (status.has_imported_file ? 'Файл імпортовано' : 'Файл не вибрано')}
                {status.has_imported_file && settings.ics_url ? ' · використовується посилання' : ''}
              </span>
            </span>
          }
        >
          {status.has_imported_file && (
            <button type="button" onClick={() => { setPickedFileName(null); removeFile(); }} className="settings-button">
              Видалити
            </button>
          )}
          {/* Native file input hidden behind a styled button */}
          <label className={`settings-button cursor-pointer ${busy ? 'opacity-50 pointer-events-none' : ''}`}>
            Вибрати файл .ics
            <input
              type="file"
              accept=".ics,text/calendar"
              disabled={busy}
              onChange={(e) => {
                const file = e.target.files?.[0];
                if (file) setPickedFileName(file.name);
                importFile(file);
                e.target.value = '';
              }}
              className="sr-only"
            />
          </label>
        </SettingsRow>
        <SettingsRow
          label={
            <span className="flex flex-col min-w-0">
              <span>Оновлення</span>
              <span className={`text-xs truncate ${status.last_error ? 'text-[#E5352B]' : 'text-[color:var(--muted-text)]'}`}>
                {status.last_error
                  ? status.last_error
                  : status.last_refresh_at
                    ? `Оновлено о ${formatTime(status.last_refresh_at)}; перевірка кожні 5 хвилин`
                    : 'Ще не оновлювалось'}
              </span>
            </span>
          }
        >
          <button type="button" onClick={refresh} disabled={busy || !configured} className="settings-button disabled:opacity-50">
            <RefreshCw size={13} className={busy ? 'animate-spin' : ''} />
            Оновити
          </button>
        </SettingsRow>
      </SettingsGroup>

      <SettingsGroup title="Перед нарадою" help="Перед кожною зустріччю з календаря з'являється нагадування з пропозицією почати запис. Чи нагадувати взагалі, задається в «Сповіщеннях».">
        <SettingsRow label="Нагадувати за" htmlFor="remind-before">
          <select
            id="remind-before"
            value={settings.remind_minutes_before}
            disabled={busy}
            onChange={(e) => save({ remind_minutes_before: Number(e.target.value) })}
            className="h-7 px-2 text-[13px]"
          >
            {REMIND_OPTIONS.map((m) => (
              <option key={m} value={m}>
                {m === 0 ? 'на початку' : `${m} хв`}
              </option>
            ))}
          </select>
        </SettingsRow>
        <SettingsRow label="Показувати вікно поверх інших">
          <Switch
            checked={settings.bring_window_to_front}
            disabled={busy}
            onCheckedChange={(bring_window_to_front) => save({ bring_window_to_front })}
          />
        </SettingsRow>
      </SettingsGroup>
    </div>
  );
}
