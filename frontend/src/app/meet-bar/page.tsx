'use client';

import { FormEvent, useEffect, useMemo, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { meterPercent, SILENCE_RMS, useCallMute, useMicMute } from '@/lib/audioSources';
import { BAR_WEIGHTS, Bars, CallIcon, CameraIcon, formatClock, MicIcon, PauseIcon, PlayIcon, StopIcon } from '@/components/capsule/parts';
import { isMeetUrl, participantsText, useMeetLive, type MeetPrompt } from '@/lib/meet';
import type { CalendarEvent } from '@/components/CalendarReminder';

/**
 * Protokol's toolbar at the top of the Google Meet window (meet/mod.rs): the
 * meeting, who speaks, the recording capsule (the same one as the floating
 * island) and «Протокол». Until a meeting is chosen the page also fills the
 * window below the toolbar as the start screen, so Google's white start
 * page is not shown. It is the app's page: the Meet page cannot reach it.
 */

interface RecordingState {
  is_recording: boolean;
  is_stopping?: boolean;
  is_paused: boolean;
  active_duration: number | null;
}

interface Levels {
  mic_rms: number;
  system_rms: number;
}

interface Rec {
  recording: boolean;
  paused: boolean;
  /** «Завершити запис» was pressed: the recording is being finished */
  stopping: boolean;
  seconds: number;
}

const IDLE: Rec = { recording: false, paused: false, stopping: false, seconds: 0 };

function useRecording(): [Rec, () => void] {
  const [rec, setRec] = useState<Rec>(IDLE);
  useEffect(() => {
    const sync = () =>
      invoke<RecordingState>('get_recording_state')
        .then((s) =>
          setRec((r) =>
            // While it stops the time is gone: the last one shown stays
            s.is_recording && (s.is_stopping || s.active_duration == null)
              ? { ...r, recording: true, stopping: true }
              : { recording: s.is_recording, paused: s.is_paused, stopping: false, seconds: s.active_duration ?? 0 },
          ),
        )
        .catch(() => {});
    sync();
    const poll = setInterval(sync, 5000);
    const subs = [
      listen('recording-started', () => setRec({ ...IDLE, recording: true })),
      listen('recording-stopped', () => setRec(IDLE)),
      listen('recording-paused', () => setRec((r) => ({ ...r, paused: true }))),
      listen('recording-resumed', () => setRec((r) => ({ ...r, paused: false }))),
    ];
    return () => {
      clearInterval(poll);
      subs.forEach((p) => p.then((f) => f()));
    };
  }, []);
  useEffect(() => {
    if (!rec.recording || rec.paused || rec.stopping) return;
    const tick = setInterval(() => setRec((r) => ({ ...r, seconds: r.seconds + 1 })), 1000);
    return () => clearInterval(tick);
  }, [rec.recording, rec.paused, rec.stopping]);
  const markStopping = () => setRec((r) => (r.recording ? { ...r, stopping: true } : r));
  return [rec, markStopping];
}

function useLevels() {
  const [levels, setLevels] = useState<Levels>({ mic_rms: 0, system_rms: 0 });
  const [shift, setShift] = useState(0);
  useEffect(() => {
    const un = listen<Levels>('live-audio-levels', (e) => {
      setLevels(e.payload);
      setShift((v) => (v + 1) % BAR_WEIGHTS.length);
    });
    return () => {
      un.then((f) => f());
    };
  }, []);
  return { levels, shift };
}

function useStartScreen() {
  const [start, setStart] = useState(false);
  useEffect(() => {
    invoke<boolean>('meet_start_screen')
      .then(setStart)
      .catch(() => {});
    const un = listen<boolean>('meet-start-screen', (e) => setStart(e.payload === true));
    return () => {
      un.then((f) => f());
    };
  }, []);
  return start;
}

/** An offer to start the recording waits (meet/live.rs): the capsule's button shows it */
function useStartOffer() {
  const [waiting, setWaiting] = useState(false);
  useEffect(() => {
    invoke<MeetPrompt | null>('meet_prompt_current')
      .then((p) => setWaiting(p?.kind === 'start'))
      .catch(() => {});
    const un = listen<MeetPrompt | null>('meet-prompt', (e) => setWaiting(e.payload?.kind === 'start'));
    return () => {
      un.then((f) => f());
    };
  }, []);
  return waiting;
}

const focusMain = () => invoke('island_focus_main').catch(() => {});

export default function MeetBar() {
  const live = useMeetLive();
  const [rec, markStopping] = useRecording();
  const { levels, shift } = useLevels();
  const start = useStartScreen();
  const offer = useStartOffer();
  const [muted, setMuted] = useMicMute();
  const [callMuted, setCallMuted] = useCallMute();

  useEffect(() => {
    document.documentElement.style.background = 'transparent';
    document.body.style.background = 'transparent';
  }, []);

  const title = live.title || 'Google Meet';
  let sub: string;
  let warn = false;
  if (start || !live.open) {
    sub = 'Протокол';
  } else if (!live.inCall) {
    sub = 'Google Meet · ще не в нараді';
  } else if (!live.audioOk) {
    sub = 'Клацніть у нараді, щоб Протокол бачив, хто говорить';
    warn = true;
  } else {
    sub = `${participantsText(live.participants)}${live.speaking.length ? ` · говорить: ${live.speaking.join(', ')}` : ''}`;
  }

  // Meet's microphone and Protokol's are one button (meet/live.rs)
  const meetMicOff = live.meetMic === true;
  const micShownOff = rec.recording ? muted : meetMicOff;
  const toggleMic = () => {
    if (rec.recording) setMuted(!muted);
    else invoke('set_mic_muted', { muted: !meetMicOff }).catch(() => {});
  };
  const micTitle = micShownOff
    ? meetMicOff
      ? 'Мікрофон вимкнено в Meet: ваш голос не записується. Натисніть, щоб увімкнути (⇧⌘M)'
      : 'Мікрофон вимкнено: ваш голос не записується. Натисніть, щоб увімкнути (⇧⌘M)'
    : 'Мікрофон увімкнено і записується. Натисніть, щоб вимкнути (⇧⌘M)';

  const micPercent = levels.mic_rms < SILENCE_RMS ? 0 : meterPercent(levels.mic_rms);
  const callPercent = levels.system_rms < SILENCE_RMS ? 0 : meterPercent(levels.system_rms);

  const micButton = (withBars: boolean) => (
    <button
      type="button"
      className={`mcap-src ${micShownOff ? 'mcap-muted' : ''}`}
      onClick={toggleMic}
      aria-pressed={micShownOff}
      aria-label={micTitle}
      title={micTitle}
    >
      <MicIcon off={micShownOff} size={15} />
      {micShownOff ? meetMicOff && <span>Вимкнено в Meet</span> : withBars && <Bars percent={micPercent} shift={shift} />}
    </button>
  );

  // Meet's camera, from Protokol's toolbar (meet/live.rs meet_set_camera)
  const camOff = live.meetCam === true;
  const camTitle = camOff ? 'Камеру вимкнено. Натисніть, щоб увімкнути' : 'Камера увімкнена: учасники вас бачать. Натисніть, щоб вимкнути';
  const camButton = live.meetCam !== null && !start && (
    <button
      type="button"
      className={`mcap-src mcap-cam ${camOff ? 'mcap-cam-off' : 'mcap-cam-on'}`}
      onClick={() => invoke('meet_set_camera', { off: !camOff }).catch(() => {})}
      aria-pressed={!camOff}
      aria-label={camTitle}
      title={camTitle}
    >
      <CameraIcon off={camOff} size={15} />
    </button>
  );

  const capsule = rec.stopping ? (
    <div className="mcap" role="status">
      <span className="isl-dot isl-dot-paused" aria-hidden="true" />
      <span className={`mcap-time ${rec.seconds >= 3600 ? 'mcap-time-hours' : ''}`}>{formatClock(rec.seconds)}</span>
      <span className="mcap-sep" />
      <span className="mcap-paused">Завершення запису…</span>
    </div>
  ) : rec.recording ? (
    <div className="mcap">
      <span className={`isl-dot ${rec.paused ? 'isl-dot-paused' : ''}`} aria-label={rec.paused ? 'Пауза' : 'Іде запис'} />
      <span className={`mcap-time ${rec.seconds >= 3600 ? 'mcap-time-hours' : ''}`}>{formatClock(rec.seconds)}</span>
      <span className="mcap-sep" />
      {rec.paused ? (
        <span className="mcap-paused">Пауза</span>
      ) : (
        <>
          {micButton(true)}
          <button
            type="button"
            className={`mcap-src ${callMuted ? 'mcap-muted' : ''}`}
            onClick={() => setCallMuted(!callMuted)}
            aria-pressed={callMuted}
            title={callMuted ? 'Звук дзвінка вимкнено. Натисніть, щоб увімкнути' : 'Звук дзвінка записується. Натисніть, щоб вимкнути'}
            aria-label={callMuted ? 'Звук дзвінка вимкнено' : 'Звук дзвінка записується'}
          >
            <CallIcon off={callMuted} size={15} />
            {!callMuted && <Bars percent={callPercent} shift={(shift + 2) % BAR_WEIGHTS.length} />}
          </button>
          {camButton}
        </>
      )}
      <button
        type="button"
        className="mcap-btn"
        onClick={() => invoke('island_action', { action: rec.paused ? 'resume' : 'pause' }).catch(() => {})}
        title={rec.paused ? 'Продовжити запис' : 'Пауза'}
        aria-label={rec.paused ? 'Продовжити запис' : 'Пауза'}
      >
        {rec.paused ? <PlayIcon /> : <PauseIcon />}
      </button>
      <button
        type="button"
        className="mcap-btn mcap-stop"
        onClick={() => {
          markStopping();
          invoke('island_action', { action: 'stop' }).catch(() => {});
        }}
        title="Завершити запис"
        aria-label="Завершити запис"
      >
        <StopIcon />
      </button>
    </div>
  ) : (
    <div className="mcap mcap-idle">
      {live.meetMic !== null && !start && micButton(false)}
      {camButton}
      <button type="button" className={`mcap-rec ${offer ? 'mcap-rec-ask' : ''}`} onClick={() => invoke('meet_start_recording').catch(() => {})}>
        <i aria-hidden="true" />
        Почати запис
      </button>
    </div>
  );

  return (
    <div className="mshell">
      <header className="mbar" data-tauri-drag-region>
        <div className="mbar-title" data-tauri-drag-region>
          <b data-tauri-drag-region>{title}</b>
          <span className={warn ? 'mbar-warn' : ''} data-tauri-drag-region>
            {sub}
          </span>
        </div>
        {capsule}
        <button type="button" className="mbar-btn" onClick={focusMain} title="Відкрити головне вікно Протоколу">
          Протокол
        </button>
      </header>
      {start && <StartScreen />}
    </div>
  );
}

// ---------------------------------------------------------------------------
// The start screen: a new meeting, a link or code, today's Meet meetings
// ---------------------------------------------------------------------------

function sameDay(a: Date, b: Date) {
  return a.getFullYear() === b.getFullYear() && a.getMonth() === b.getMonth() && a.getDate() === b.getDate();
}

function hhmm(iso: string) {
  return new Date(iso).toLocaleTimeString('uk-UA', { hour: '2-digit', minute: '2-digit' });
}

function StartScreen() {
  const [link, setLink] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [events, setEvents] = useState<CalendarEvent[]>([]);
  const [signedIn, setSignedIn] = useState<boolean | null>(null);
  const [now, setNow] = useState(() => new Date());

  useEffect(() => {
    invoke<{ upcoming: CalendarEvent[] }>('api_calendar_status')
      .then((s) => setEvents(s?.upcoming ?? []))
      .catch(() => {});
    invoke<{ signedIn: boolean | null }>('get_meet_data')
      .then((d) => setSignedIn(d.signedIn))
      .catch(() => {});
    const un = listen<boolean>('meet-signed-in', (e) => setSignedIn(e.payload === true));
    const tick = setInterval(() => setNow(new Date()), 30_000);
    return () => {
      clearInterval(tick);
      un.then((f) => f());
    };
  }, []);

  const today = useMemo(
    () =>
      events
        .filter((e) => isMeetUrl(e.meeting_url) && sameDay(new Date(e.start), now) && new Date(e.end ?? e.start).getTime() >= now.getTime())
        .sort((a, b) => a.start.localeCompare(b.start)),
    [events, now],
  );
  // Joining never starts a recording: the offer to start it comes once the call starts
  const go = async (target: string, title?: string) => {
    setError(null);
    try {
      await invoke('meet_go', { target, title: title ?? null });
    } catch (e) {
      setError(`${e}`);
    }
  };

  const join = (e: FormEvent) => {
    e.preventDefault();
    if (!link.trim()) {
      setError('Вставте посилання на нараду або її код');
      return;
    }
    void go(link.trim());
  };

  return (
    <main className="mstart">
      <h1 className="mstart-h">Нарада в Google Meet</h1>
      <div className="mstart-cards">
        <section className="mstart-card">
          <h2>
            <svg width="18" height="18" viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
              <rect x="2" y="5" width="11" height="10" rx="2" />
              <path d="M13 9l5-3v8l-5-3zM7.5 7.5v5M5 10h5" />
            </svg>
            Нова нарада
          </h2>
          <p>Google створить нараду і покаже посилання, яке можна надіслати учасникам.</p>
          <button type="button" className="mstart-btn mstart-blue" onClick={() => void go('new')}>
            Створити
          </button>
        </section>
        <section className="mstart-card">
          <h2>
            <svg width="18" height="18" viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
              <path d="M8 12l4-4M9 5.5l1.5-1.5a3.5 3.5 0 0 1 5 5L14 10.5M11 14.5L9.5 16a3.5 3.5 0 0 1-5-5L6 9.5" />
            </svg>
            Приєднатися
          </h2>
          <form className="mstart-join" onSubmit={join}>
            <label>
              Посилання або код наради
              <input
                type="text"
                value={link}
                onChange={(e) => setLink(e.target.value)}
                placeholder="abc-defg-hij"
                autoComplete="off"
                spellCheck={false}
              />
            </label>
            <button type="submit" className="mstart-btn">
              Приєднатися
            </button>
          </form>
        </section>
      </div>
      {error && <p className="mstart-error">{error}</p>}

      {today.length > 0 && (
        <section className="mstart-list" aria-label="Сьогодні в календарі">
          <h3>Сьогодні в календарі</h3>
          {today.map((e) => {
            const started = new Date(e.start).getTime() <= now.getTime();
            const minutesAgo = Math.max(0, Math.round((now.getTime() - new Date(e.start).getTime()) / 60_000));
            return (
              <div key={e.uid} className="mstart-row">
                <span className="mstart-time">{hhmm(e.start)}</span>
                <span className="mstart-name">
                  <b>{e.title}</b>
                  <span>{started ? (minutesAgo > 0 ? `Google Meet · почалась ${minutesAgo} хв тому` : 'Google Meet · починається зараз') : 'Google Meet'}</span>
                </span>
                <button type="button" className="mstart-btn" onClick={() => void go(e.meeting_url ?? '', e.title)}>
                  Приєднатися
                </button>
              </div>
            );
          })}
        </section>
      )}

      <p className="mstart-foot">
        {signedIn === false ? (
          <button type="button" className="mstart-btn mstart-blue" onClick={() => void go('signin')}>
            Увійти в Google
          </button>
        ) : signedIn ? (
          'Вхід у Google виконано. Окремо від Chrome і Safari.'
        ) : (
          'Вхід у Google виконується тут, окремо від Chrome і Safari.'
        )}
      </p>
    </main>
  );
}
