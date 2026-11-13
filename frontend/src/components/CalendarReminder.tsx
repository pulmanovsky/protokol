'use client';

import { useEffect, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import { toast } from '@/lib/notice';
import { usePathname, useRouter } from 'next/navigation';
import { SourcePicker } from '@/components/audio/SourcePicker';
import { isMeetUrl, loadMeetSettings, openCallLink } from '@/lib/meet';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';

export interface CalendarEvent {
  uid: string;
  title: string;
  start: string;
  end: string | null;
  meeting_url: string | null;
  location: string | null;
}

interface ReminderPayload {
  event: CalendarEvent;
  minutes_until: number;
}

/** Read by useRecordingStart to name the meeting after the calendar event. */
export const PENDING_MEETING_TITLE_KEY = 'pendingMeetingTitle';

export function formatTime(iso: string): string {
  return new Date(iso).toLocaleTimeString('uk-UA', { hour: '2-digit', minute: '2-digit' });
}

/**
 * Shows "a meeting is starting" when the calendar reminder fires. Nothing is
 * recorded unless «Почати запис» is pressed: a Google Meet link opens in
 * Protokol's Meet window, which offers to start the recording once the call
 * starts there.
 */
export function CalendarReminder() {
  const [reminder, setReminder] = useState<ReminderPayload | null>(null);
  // The last reminder stays drawn while the dialog fades out, so the closing
  // frame never shows an empty «Почати запис» variant
  const [last, setLast] = useState<ReminderPayload | null>(null);
  const pathname = usePathname();
  const router = useRouter();
  // Settings → Google Meet, read when the reminder comes
  const [offerStart, setOfferStart] = useState(true);

  useEffect(() => {
    const unlisten = listen<ReminderPayload>('calendar-meeting-reminder', (e) => {
      setReminder(e.payload);
      setLast(e.payload);
      loadMeetSettings().then((s) => setOfferStart(s.remindStart));
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  /** «Почати запис»: the recording is named after the calendar event */
  const startRecording = () => {
    if (!reminder) return;
    const { event } = reminder;
    setReminder(null);
    sessionStorage.setItem(PENDING_MEETING_TITLE_KEY, event.title);
    if (pathname === '/') {
      window.dispatchEvent(new CustomEvent('start-recording-from-sidebar'));
    } else {
      sessionStorage.setItem('autoStartRecording', 'true');
      router.push('/');
    }
  };

  /** Opens the call without recording: a Meet link in Protokol's Meet window, other services in their own app */
  const openCall = async () => {
    if (!reminder?.event.meeting_url) return;
    const { event } = reminder;
    setReminder(null);
    try {
      await openCallLink(event.meeting_url!, event.title);
    } catch (error) {
      toast.error(`${error}`);
    }
  };

  const shown = reminder ?? last;
  const event = shown?.event;
  const meet = isMeetUrl(event?.meeting_url);
  const otherCall = !meet && !!event?.meeting_url;
  const when =
    shown && shown.minutes_until > 0
      ? `Починається через ${shown.minutes_until} хв`
      : event
        ? `Почалася о ${formatTime(event.start)}`
        : '';

  const start = event ? new Date(event.start) : null;
  const month = start ? start.toLocaleDateString('uk-UA', { month: 'short' }).replace('.', '').toUpperCase() : '';
  const timeRange = event ? `${formatTime(event.start)}${event.end ? ` — ${formatTime(event.end)}` : ''}` : '';
  const service = event?.meeting_url ? callServiceName(event.meeting_url) : event?.location;

  return (
    <Dialog open={reminder !== null} onOpenChange={(open) => !open && setReminder(null)}>
      <DialogContent className="max-w-[480px] gap-4">
        <DialogHeader className="flex-row items-center gap-3.5 space-y-0 text-left">
          <div className="w-12 h-12 flex-none rounded-[10px] bg-[#fff] shadow-[0_2px_6px_rgba(30,40,80,0.12)] flex flex-col items-center overflow-hidden" aria-hidden="true">
            <span className="w-full text-center text-[9px] font-bold text-white bg-[#E5352B] py-0.5">{month}</span>
            <span className="text-xl font-semibold leading-[1.3] text-[#1D1D1F]">{start?.getDate()}</span>
          </div>
          <div className="flex flex-col gap-0.5 min-w-0">
            <span className="text-[13px] font-semibold text-[color:var(--muted-text)]">{when}</span>
            <DialogTitle className="text-[22px] font-bold tracking-[-0.3px] text-gray-900 truncate">{event?.title}</DialogTitle>
            <span className="text-sm text-[color:var(--muted-text)] tabular-nums">
              {timeRange}
              {service ? ` · ${service}` : ''}
            </span>
          </div>
        </DialogHeader>
        <DialogDescription className="text-[15px] leading-[1.5] text-gray-800">
          {!meet
            ? 'Записати цю зустріч, щоб потім отримати стенограму та документ?'
            : offerStart
              ? 'Google Meet відкриється у вікні Протоколу. Коли нарада почнеться, з\'явиться пропозиція почати запис.'
              : 'Google Meet відкриється у вікні Протоколу. Запис починається кнопкою «Почати запис» над нарадою.'}
        </DialogDescription>
        <div className="flex flex-col gap-2">
          {meet ? (
            <button type="button" onClick={() => void openCall()} className="accent-button h-12 text-[15px] font-semibold">
              Приєднатися в Протоколі
            </button>
          ) : (
            <>
              <div className="flex justify-center">
                <SourcePicker align="center" />
              </div>
              <button type="button" onClick={startRecording} className="record-button justify-center h-12 gap-2.5 text-[15px] font-semibold">
                <span className="w-2.5 h-2.5 rounded-full bg-[#fff]" aria-hidden="true" />
                Почати запис
              </button>
            </>
          )}
          <div className={`grid gap-2 ${otherCall ? 'grid-cols-2' : ''}`}>
            {otherCall && (
              <button type="button" onClick={() => void openCall()} className="secondary-button h-11 text-sm font-semibold">
                Відкрити дзвінок
              </button>
            )}
            <button type="button" onClick={() => setReminder(null)} className="secondary-button h-11 text-sm font-semibold text-[color:var(--muted-text)]">
              Не цього разу
            </button>
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}

function callServiceName(url: string): string | null {
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
