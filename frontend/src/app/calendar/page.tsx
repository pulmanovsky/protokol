'use client';

import { useCallback, useEffect, useMemo, useState } from 'react';
import Link from 'next/link';
import { invoke } from '@tauri-apps/api/core';
import { RefreshCw } from 'lucide-react';
import { toast } from '@/lib/notice';
import { useSidebar } from '@/components/Sidebar/SidebarProvider';
import { CalendarEvent, PENDING_MEETING_TITLE_KEY, formatTime } from '@/components/CalendarReminder';

interface CalendarStatus {
  settings: { enabled: boolean; ics_url: string | null };
  has_imported_file: boolean;
  last_refresh_at: string | null;
  last_error: string | null;
  upcoming: CalendarEvent[];
}

const DAYS_SHOWN = 3;

function dayKey(d: Date): string {
  return `${d.getFullYear()}-${d.getMonth()}-${d.getDate()}`;
}

function dayTitle(offset: number, date: Date): string {
  const label = date.toLocaleDateString('uk-UA', { weekday: 'long', day: 'numeric', month: 'long' });
  if (offset === 0) return `Сьогодні, ${label}`;
  if (offset === 1) return `Завтра, ${label}`;
  return label.charAt(0).toUpperCase() + label.slice(1);
}

function serviceName(url: string | null): string | null {
  if (!url) return null;
  try {
    const host = new URL(url).hostname;
    if (host.includes('meet.google')) return 'Google Meet';
    if (host.includes('zoom')) return 'Zoom';
    if (host.includes('teams')) return 'Microsoft Teams';
    return host.replace(/^www\./, '');
  } catch {
    return null;
  }
}

export default function CalendarPage() {
  const { handleRecordingToggle } = useSidebar();
  const [status, setStatus] = useState<CalendarStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const [now, setNow] = useState(() => new Date());

  useEffect(() => {
    invoke<CalendarStatus>('api_calendar_status')
      .then(setStatus)
      .catch((error) => console.error('Failed to load calendar', error));
    const timer = setInterval(() => setNow(new Date()), 30_000);
    return () => clearInterval(timer);
  }, []);

  const refresh = useCallback(async () => {
    setBusy(true);
    try {
      setStatus(await invoke<CalendarStatus>('api_calendar_refresh'));
    } catch (error) {
      toast.error('Не вдалося оновити календар', { description: String(error) });
    } finally {
      setBusy(false);
    }
  }, []);

  const configured = !!status && (!!status.settings?.ics_url || status.has_imported_file);

  const days = useMemo(() => {
    const events = status?.upcoming ?? [];
    return Array.from({ length: DAYS_SHOWN }, (_, offset) => {
      const date = new Date(now);
      date.setDate(now.getDate() + offset);
      const key = dayKey(date);
      return {
        offset,
        date,
        events: events
          .filter((e) => dayKey(new Date(e.start)) === key)
          .sort((a, b) => a.start.localeCompare(b.start)),
      };
    });
  }, [status, now]);

  const record = (event: CalendarEvent) => {
    sessionStorage.setItem(PENDING_MEETING_TITLE_KEY, event.title);
    handleRecordingToggle();
  };

  return (
    <div className="h-full overflow-y-auto custom-scrollbar">
      <div className="flex flex-col gap-6 p-8">
        <header data-tauri-drag-region data-notice-anchor className="flex items-center justify-between gap-4 flex-wrap">
          <div className="flex flex-col gap-0.5">
            <h1 data-tauri-drag-region className="text-[30px] font-bold tracking-[-0.6px] text-gray-900">Календар</h1>
            <span data-tauri-drag-region className="text-sm text-[color:var(--muted-text)]">Зустрічі на сьогодні та наступні два дні</span>
          </div>
          {configured && (
            <div className="flex items-center gap-2.5">
              <Link href="/settings?section=calendar" className="secondary-button h-10 px-4 text-sm font-semibold">
                Налаштувати календар
              </Link>
              <button type="button" onClick={refresh} disabled={busy} className="secondary-button h-10 px-4 gap-2 text-sm font-semibold disabled:opacity-50">
                <RefreshCw className={`w-4 h-4 ${busy ? 'animate-spin' : ''}`} />
                Оновити
              </button>
            </div>
          )}
        </header>

        {status && !configured ? (
          <section className="card px-6 py-12 flex flex-col items-center gap-3 text-center">
            <h2 className="text-lg font-bold text-gray-900">Календар не підключено</h2>
            <p className="text-sm text-[color:var(--muted-text)] max-w-md">
              Додайте посилання на календар (iCal) або файл .ics, і тут з’являться ваші зустрічі з кнопкою для запису.
            </p>
            <Link href="/settings?section=calendar" className="accent-button h-11 px-5 text-[15px] font-semibold mt-2">
              Налаштувати календар
            </Link>
          </section>
        ) : (
          days.map(({ offset, date, events }) => (
            <section key={offset} aria-label={dayTitle(offset, date)} className="flex flex-col gap-2">
              <h2 className="text-[15px] font-bold text-gray-900">{dayTitle(offset, date)}</h2>
              <div className="card py-2">
                {events.length === 0 ? (
                  <p className="px-6 py-3 text-sm text-[color:var(--muted-text)]">Зустрічей немає</p>
                ) : (
                  <ul>
                    {events.map((event) => {
                      const past = new Date(event.end ?? event.start).getTime() < now.getTime();
                      const service = serviceName(event.meeting_url) ?? event.location;
                      return (
                        <li key={event.uid} className="grid grid-cols-[96px_minmax(0,1fr)_auto] gap-4 items-center px-6 min-h-14 py-2 border-b border-[color:var(--hairline)] last:border-b-0">
                          <span className={`text-sm tabular-nums ${past ? 'text-[color:var(--muted-text)]' : 'font-semibold text-gray-900'}`}>
                            {formatTime(event.start)}
                            {event.end ? `–${formatTime(event.end)}` : ''}
                          </span>
                          <span className="flex flex-col min-w-0">
                            <span className={`text-[15px] truncate ${past ? 'text-[color:var(--muted-text)]' : 'font-semibold text-gray-900'}`}>{event.title}</span>
                            {service && <span className="text-[13px] text-[color:var(--muted-text)] truncate">{service}</span>}
                          </span>
                          {past ? (
                            <span className="text-[13px] text-[color:var(--muted-text)]">Минула</span>
                          ) : (
                            <button type="button" onClick={() => record(event)} className="secondary-button h-9 px-3.5 gap-2 text-[13px] font-semibold">
                              <span className="w-2 h-2 rounded-full record-dot" aria-hidden="true" />
                              Записати
                            </button>
                          )}
                        </li>
                      );
                    })}
                  </ul>
                )}
              </div>
            </section>
          ))
        )}
      </div>
    </div>
  );
}
