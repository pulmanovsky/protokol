'use client';

import { useEffect, useMemo, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useRouter } from 'next/navigation';
import { useSidebar } from '@/components/Sidebar/SidebarProvider';
import { CalendarEvent, PENDING_MEETING_TITLE_KEY, formatTime } from '@/components/CalendarReminder';
import { PermissionWarning } from '@/components/PermissionWarning';
import { usePermissionCheck } from '@/hooks/usePermissionCheck';
import { SourcePicker } from '@/components/audio/SourcePicker';
import { Upload } from 'lucide-react';
import { useConfig } from '@/contexts/ConfigContext';
import { useImportDialog } from '@/contexts/ImportDialogContext';

interface CalendarStatusLite {
  upcoming: CalendarEvent[];
}

const RECENT_LIMIT = 5;

function isSameDay(a: Date, b: Date): boolean {
  return a.getFullYear() === b.getFullYear() && a.getMonth() === b.getMonth() && a.getDate() === b.getDate();
}

/** "через 12 хв" badge parts for the next meeting */
function countdown(start: Date, now: Date): { top: string; value: string } {
  const minutes = Math.round((start.getTime() - now.getTime()) / 60000);
  if (minutes <= 0) return { top: 'ЗАРАЗ', value: formatTime(start.toISOString()) };
  if (minutes < 60) return { top: 'ЧЕРЕЗ', value: `${minutes} хв` };
  const hours = Math.round(minutes / 60);
  if (hours < 24) return { top: 'ЧЕРЕЗ', value: `${hours} год` };
  return { top: start.toLocaleDateString('uk-UA', { weekday: 'short' }).toUpperCase(), value: formatTime(start.toISOString()) };
}

function callService(url: string | null): string | null {
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

function meetingDate(iso: string | undefined, now: Date): string {
  if (!iso) return '';
  const date = new Date(iso);
  if (isSameDay(date, now)) return `сьогодні, ${formatTime(iso)}`;
  const yesterday = new Date(now);
  yesterday.setDate(now.getDate() - 1);
  if (isSameDay(date, yesterday)) return `вчора, ${formatTime(iso)}`;
  return date.toLocaleDateString('uk-UA', { day: 'numeric', month: 'long' });
}

export function TodayHome() {
  const { betaFeatures } = useConfig();
  const { openImportDialog } = useImportDialog();
  const router = useRouter();
  const { meetings, handleRecordingToggle, setCurrentMeeting } = useSidebar();
  const { checkPermissions, isChecking, hasSystemAudio, hasMicrophone } = usePermissionCheck();
  const [events, setEvents] = useState<CalendarEvent[]>([]);
  const [now, setNow] = useState(() => new Date());

  useEffect(() => {
    invoke<CalendarStatusLite>('api_calendar_status')
      .then((status) => setEvents(status?.upcoming ?? []))
      .catch((error) => console.error('Failed to load calendar', error));
    const timer = setInterval(() => setNow(new Date()), 30_000);
    return () => clearInterval(timer);
  }, []);

  const todayEvents = useMemo(
    () => events.filter((e) => isSameDay(new Date(e.start), now)).sort((a, b) => a.start.localeCompare(b.start)),
    [events, now],
  );

  // Next = the first event that has not ended yet
  const next = useMemo(
    () =>
      events
        .filter((e) => new Date(e.end ?? e.start).getTime() >= now.getTime())
        .sort((a, b) => a.start.localeCompare(b.start))[0] ?? null,
    [events, now],
  );

  const recent = meetings.slice(0, RECENT_LIMIT);

  const recordEvent = (event: CalendarEvent) => {
    sessionStorage.setItem(PENDING_MEETING_TITLE_KEY, event.title);
    handleRecordingToggle();
  };

  const openCall = (url: string) => {
    invoke('open_external_url', { url }).catch((error) => console.error('Failed to open meeting link', error));
  };

  const today = now.toLocaleDateString('uk-UA', { weekday: 'long', day: 'numeric', month: 'long' });

  return (
    <div className="flex-1 min-w-0 h-full overflow-y-auto custom-scrollbar">
      <div className="flex flex-col gap-6 p-8">
        <header data-tauri-drag-region data-notice-anchor className="flex items-center justify-between gap-4 flex-wrap">
          <div className="flex flex-col gap-0.5">
            <h1 data-tauri-drag-region className="text-[30px] font-bold tracking-[-0.6px] text-gray-900">Сьогодні</h1>
            <span data-tauri-drag-region className="text-sm text-[color:var(--muted-text)]">{today}</span>
          </div>
          <div className="flex items-center gap-2 flex-wrap">
            {betaFeatures.importAndRetranscribe && (
              <button type="button" onClick={() => openImportDialog()} className="secondary-button h-11 px-4 gap-2 text-sm font-semibold">
                <Upload className="w-4 h-4" />
                Імпортувати аудіо
              </button>
            )}
            <SourcePicker />
          <button type="button" onClick={handleRecordingToggle} className="record-button h-11 px-5 gap-2.5 text-[15px] font-semibold">
              <span className="w-2.5 h-2.5 rounded-full bg-[#fff]" aria-hidden="true" />
              Почати запис
            </button>
          </div>
        </header>

        {!isChecking && (!hasMicrophone || !hasSystemAudio) && (
          <PermissionWarning
            hasMicrophone={hasMicrophone}
            hasSystemAudio={hasSystemAudio}
            onRecheck={checkPermissions}
            isRechecking={isChecking}
          />
        )}

        {next && (
          <section aria-label="Наступна зустріч" className="card p-6 flex items-center justify-between gap-6 flex-wrap">
            <div className="flex items-center gap-4 min-w-0">
              <div className="w-16 h-16 rounded-[10px] bg-[var(--system-fill)] flex flex-col items-center justify-center flex-shrink-0">
                <span className="text-[11px] font-bold text-[color:var(--muted-text)]">{countdown(new Date(next.start), now).top}</span>
                <span className="text-xl font-bold leading-tight text-gray-900 tabular-nums">{countdown(new Date(next.start), now).value}</span>
              </div>
              <div className="flex flex-col gap-1 min-w-0">
                <span className="text-[13px] font-semibold text-[color:var(--muted-text)]">Наступна зустріч</span>
                <span className="text-2xl font-bold tracking-[-0.4px] text-gray-900 truncate">{next.title}</span>
                <span className="text-sm text-[color:var(--muted-text)] tabular-nums">
                  {formatTime(next.start)}
                  {next.end ? ` — ${formatTime(next.end)}` : ''}
                  {callService(next.meeting_url) ? ` · ${callService(next.meeting_url)}` : next.location ? ` · ${next.location}` : ''}
                </span>
              </div>
            </div>
            <div className="flex gap-2.5 flex-wrap">
              {next.meeting_url && (
                <button type="button" onClick={() => openCall(next.meeting_url!)} className="secondary-button h-10 px-4 text-sm font-semibold">
                  Відкрити дзвінок
                </button>
              )}
              <button type="button" onClick={() => recordEvent(next)} className="record-button h-10 px-4 gap-2 text-sm font-semibold">
                <span className="w-2 h-2 rounded-full bg-white" aria-hidden="true" />
                Записати цю зустріч
              </button>
            </div>
          </section>
        )}

        <div className="grid gap-4 grid-cols-1 md:grid-cols-[minmax(260px,320px)_minmax(0,1fr)]">
          <section aria-label="З календаря" className="card px-5 py-4 flex flex-col">
            <h2 className="text-[15px] font-bold text-gray-900 mb-2">З календаря</h2>
            {todayEvents.length === 0 ? (
              <p className="py-2 text-sm text-[color:var(--muted-text)]">Немає зустрічей у календарі</p>
            ) : (
              todayEvents.map((event) => {
                const past = new Date(event.end ?? event.start).getTime() < now.getTime();
                const isNext = next?.uid === event.uid;
                return (
                  <div key={event.uid} className="flex gap-3.5 py-2.5 border-b border-[color:var(--hairline)] last:border-b-0">
                    <span className={`w-12 text-sm tabular-nums ${isNext ? 'font-bold text-gray-900' : 'text-[color:var(--muted-text)]'}`}>
                      {formatTime(event.start)}
                    </span>
                    <span className={`text-sm truncate ${past ? 'text-[color:var(--muted-text)]' : isNext ? 'font-semibold text-gray-900' : 'text-gray-900'}`}>
                      {event.title}
                    </span>
                  </div>
                );
              })
            )}
          </section>

          <section aria-label="Останні записи" className="card px-5 py-4 flex flex-col min-w-0">
            <h2 className="text-[15px] font-bold text-gray-900 mb-2">Останні записи</h2>
            {recent.length === 0 ? (
              <p className="py-2 text-sm text-[color:var(--muted-text)]">Ще немає записів</p>
            ) : (
              recent.map((meeting) => (
                <button
                  key={meeting.id}
                  type="button"
                  onClick={() => {
                    setCurrentMeeting({ id: meeting.id, title: meeting.title });
                    router.push(`/meeting-details?id=${meeting.id}`);
                  }}
                  className="grid grid-cols-[minmax(0,1fr)_auto_112px] gap-3 items-center py-2.5 text-left border-b border-[color:var(--hairline)] last:border-b-0 hover:opacity-80"
                >
                  <span className="text-sm font-semibold text-gray-900 truncate">{meeting.title}</span>
                  <span className="text-[13px] text-[color:var(--muted-text)]">{meetingDate(meeting.createdAt, now)}</span>
                  <span className="justify-self-end">
                    {meeting.status === 'approved' ? (
                      <span className="inline-flex items-center h-[26px] px-2.5 rounded-full bg-[var(--green-tint)] text-xs font-semibold text-[color:var(--green-text)]">Затверджено</span>
                    ) : (
                      <span className="inline-flex items-center h-[26px] px-2.5 rounded-full bg-[var(--control-fill)] text-xs font-semibold text-[color:var(--muted-text)]">Чернетка</span>
                    )}
                  </span>
                </button>
              ))
            )}
          </section>
        </div>
      </div>
    </div>
  );
}
