'use client';

import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWindow, currentMonitor } from '@tauri-apps/api/window';
import { LogicalPosition } from '@tauri-apps/api/dpi';
import { meterPercent, readStoredSources, SILENCE_RMS, useMicMute, useCallMute } from '@/lib/audioSources';
import { BAR_WEIGHTS, Bars, CallIcon, Chevron, DoneIcon, formatClock, MicIcon, PauseIcon, PlayIcon, Ring, StopIcon, WarnIcon, XIcon } from '@/components/capsule/parts';
import { answerPrompt } from '@/components/MeetPrompt';
import { useMeetLive, type MeetPrompt } from '@/lib/meet';

const HEIGHT = 44;
const EDGE_MARGIN = 16;
const POSITION_KEY = 'protokol.island.position';
const COLLAPSED_KEY = 'protokol.island.collapsed';
/** A source that sends nothing for this long is shown as missing */
const MISSING_AFTER_MS = 10_000;
/** «Стенограма готова» stays this long, then the island closes */
const READY_FOR_MS = 10_000;
/** No speaker recognition after this long: the transcript is ready as it is */
const NO_SPEAKERS_AFTER_MS = 30_000;

type Side = 'left' | 'right';
type Phase = 'recording' | 'processing' | 'ready';

interface RecordingState {
  is_recording: boolean;
  is_paused: boolean;
  active_duration: number | null;
}

interface Levels {
  mic_rms: number;
  system_rms: number;
  mic_active: boolean;
  system_active: boolean;
}

function readJson<T>(key: string): T | null {
  try {
    const raw = localStorage.getItem(key);
    return raw ? (JSON.parse(raw) as T) : null;
  } catch {
    return null;
  }
}

function writeJson(key: string, value: unknown) {
  try {
    localStorage.setItem(key, JSON.stringify(value));
  } catch {
    // Position is a convenience only
  }
}

async function screenArea() {
  const monitor = await currentMonitor();
  if (!monitor) return null;
  const scale = monitor.scaleFactor;
  return {
    scale,
    x: monitor.position.x / scale,
    y: monitor.position.y / scale,
    w: monitor.size.width / scale,
    h: monitor.size.height / scale,
  };
}

/** Snaps the island to the nearest left or right screen edge and remembers the spot */
async function snapToEdge(width: number): Promise<Side | null> {
  const area = await screenArea();
  if (!area) return null;
  const win = getCurrentWindow();
  const pos = (await win.outerPosition()).toLogical(area.scale);
  const side: Side = pos.x + width / 2 < area.x + area.w / 2 ? 'left' : 'right';
  const x = side === 'left' ? area.x + EDGE_MARGIN : area.x + area.w - width - EDGE_MARGIN;
  const y = Math.min(Math.max(pos.y, area.y + EDGE_MARGIN), area.y + area.h - HEIGHT - EDGE_MARGIN);
  if (Math.abs(x - pos.x) > 1 || Math.abs(y - pos.y) > 1) {
    await win.setPosition(new LogicalPosition(x, y));
  }
  writeJson(POSITION_KEY, { side, yRatio: (y - area.y) / area.h });
  return side;
}

/** Restores the remembered edge and height on this screen */
async function restorePosition(width: number) {
  const saved = readJson<{ side: Side; yRatio: number }>(POSITION_KEY);
  const area = await screenArea();
  if (!saved || !area) return;
  const x = saved.side === 'left' ? area.x + EDGE_MARGIN : area.x + area.w - width - EDGE_MARGIN;
  const y = Math.min(Math.max(area.y + saved.yRatio * area.h, area.y + EDGE_MARGIN), area.y + area.h - HEIGHT - EDGE_MARGIN);
  await getCurrentWindow().setPosition(new LogicalPosition(x, y));
}

// ---------------------------------------------------------------------------

export default function IslandPage() {
  const [phase, setPhase] = useState<Phase>('recording');
  const [state, setState] = useState<RecordingState>({ is_recording: true, is_paused: false, active_duration: 0 });
  const [seconds, setSeconds] = useState(0);
  const [levels, setLevels] = useState<Levels>({ mic_rms: 0, system_rms: 0, mic_active: true, system_active: true });
  const [shift, setShift] = useState(0);
  const [missing, setMissing] = useState({ mic: false, system: false });
  const [collapsed, setCollapsed] = useState(false);
  const [nativeGlass, setNativeGlass] = useState(false);
  const [side, setSide] = useState<Side>('right');
  const [minutesLeft, setMinutesLeft] = useState<number | null>(null);
  const [sources] = useState(readStoredSources);
  const [muted, setMuted] = useMicMute();
  const [callMuted, setCallMuted] = useCallMute();
  // A Google Meet offer while neither Protokol window is in front (meet/live.rs)
  const [prompt, setPrompt] = useState<MeetPrompt | null>(null);

  const capsuleRef = useRef<HTMLDivElement>(null);
  const widthRef = useRef(0);
  const placedRef = useRef(false);
  const lastSeenRef = useRef({ mic: Date.now(), system: Date.now() });
  // macOS sends no call sound while no app plays any, so silence on the call
  // channel means something only during a Google Meet call with others in
  // Protokol's window, where Meet keeps playing
  const meet = useMeetLive();
  const callExpected = meet.open && meet.inCall && meet.participants >= 2;
  const speakersStartRef = useRef<number | null>(null);
  const meetingRef = useRef<string | null>(null);

  // Transparent page behind the capsule; native glass when the window has it
  useEffect(() => {
    document.documentElement.style.background = 'transparent';
    document.body.style.background = 'transparent';
    setCollapsed(readJson<boolean>(COLLAPSED_KEY) ?? false);
    setSide(readJson<{ side: Side }>(POSITION_KEY)?.side ?? 'right');
    invoke<boolean>('island_has_native_glass')
      .then((has) => setNativeGlass(has === true))
      .catch(() => setNativeGlass(false));
  }, []);

  useEffect(() => {
    invoke<MeetPrompt | null>('meet_prompt_current')
      .then((p) => setPrompt(p ?? null))
      .catch(() => {});
    const un = listen<MeetPrompt | null>('meet-prompt', (e) => setPrompt(e.payload ?? null));
    return () => {
      un.then((f) => f());
    };
  }, []);

  // The window always matches the capsule, anchored to its screen edge
  useLayoutEffect(() => {
    const el = capsuleRef.current;
    if (!el) return;
    const fit = () => {
      const width = Math.ceil(el.getBoundingClientRect().width);
      if (!width || width === widthRef.current) return;
      widthRef.current = width;
      invoke('island_set_width', { width, anchor: side })
        .then(async () => {
          if (placedRef.current) return;
          placedRef.current = true;
          if (readJson(POSITION_KEY)) await restorePosition(width);
          else await snapToEdge(width);
        })
        .catch(() => {});
    };
    fit();
    const observer = new ResizeObserver(fit);
    observer.observe(el);
    return () => observer.disconnect();
  }, [phase, collapsed, side, prompt?.kind]);

  // Recording time from the backend, ticking locally in between
  useEffect(() => {
    if (phase !== 'recording') return;
    const sync = () =>
      invoke<RecordingState>('get_recording_state')
        .then((s) => {
          setState(s);
          if (s.active_duration != null) setSeconds(s.active_duration);
        })
        .catch(() => {});
    sync();
    const poll = setInterval(sync, 5000);
    return () => clearInterval(poll);
  }, [phase]);

  useEffect(() => {
    if (phase !== 'recording' || state.is_paused) return;
    const tick = setInterval(() => setSeconds((v) => v + 1), 1000);
    return () => clearInterval(tick);
  }, [phase, state.is_paused]);

  // Levels and the recording lifecycle
  useEffect(() => {
    const resetSeen = () => {
      lastSeenRef.current = { mic: Date.now(), system: Date.now() };
    };
    const subscriptions = [
      listen<Levels>('live-audio-levels', (e) => {
        const now = Date.now();
        if (e.payload.mic_active) lastSeenRef.current.mic = now;
        if (e.payload.system_active) lastSeenRef.current.system = now;
        setLevels(e.payload);
        setShift((v) => (v + 1) % BAR_WEIGHTS.length);
      }),
      listen('recording-paused', () => setState((s) => ({ ...s, is_paused: true }))),
      listen('recording-resumed', () => {
        resetSeen();
        setState((s) => ({ ...s, is_paused: false }));
      }),
      listen('recording-started', () => {
        resetSeen();
        speakersStartRef.current = null;
        meetingRef.current = null;
        setMinutesLeft(null);
        setSeconds(0);
        setState({ is_recording: true, is_paused: false, active_duration: 0 });
        setPhase('recording');
      }),
      listen('recording-stopped', () => {
        setMinutesLeft(null);
        setPhase('processing');
      }),
      listen<{ meeting_id: string; progress_percentage: number }>('speakers-progress', (e) => {
        if (meetingRef.current && meetingRef.current !== e.payload.meeting_id) return;
        meetingRef.current = e.payload.meeting_id;
        const pct = e.payload.progress_percentage;
        if (speakersStartRef.current === null) speakersStartRef.current = Date.now();
        const elapsed = (Date.now() - speakersStartRef.current) / 60000;
        setMinutesLeft(pct >= 20 && pct < 100 ? Math.max(1, Math.round((elapsed * (100 - pct)) / pct)) : null);
      }),
      listen<{ meeting_id: string }>('speakers-done', (e) => {
        if (meetingRef.current && meetingRef.current !== e.payload.meeting_id) return;
        setPhase((p) => (p === 'processing' ? 'ready' : p));
      }),
    ];
    return () => {
      subscriptions.forEach((p) => p.then((fn) => fn()));
    };
  }, []);

  // A source that has sent nothing for 10 s is shown as missing (the call:
  // only while a Meet call with others should be heard)
  const callExpectedRef = useRef(callExpected);
  useEffect(() => {
    if (callExpected && !callExpectedRef.current) lastSeenRef.current.system = Date.now();
    callExpectedRef.current = callExpected;
  }, [callExpected]);
  useEffect(() => {
    if (phase !== 'recording') return;
    const check = setInterval(() => {
      const now = Date.now();
      const running = !state.is_paused;
      const mic = running && sources.micEnabled && !muted && now - lastSeenRef.current.mic > MISSING_AFTER_MS;
      const system = running && sources.systemEnabled && !callMuted && callExpectedRef.current && now - lastSeenRef.current.system > MISSING_AFTER_MS;
      setMissing((m) => (m.mic === mic && m.system === system ? m : { mic, system }));
    }, 1000);
    return () => clearInterval(check);
  }, [phase, state.is_paused, sources, muted, callMuted]);

  // Without speaker recognition the transcript is ready once it is saved
  useEffect(() => {
    if (phase !== 'processing') return;
    const timer = setTimeout(() => {
      if (speakersStartRef.current === null) setPhase('ready');
    }, NO_SPEAKERS_AFTER_MS);
    return () => clearTimeout(timer);
  }, [phase]);

  // «Стенограма готова» closes on its own
  useEffect(() => {
    if (phase !== 'ready') return;
    const timer = setTimeout(() => invoke('island_action', { action: 'close' }).catch(() => {}), READY_FOR_MS);
    return () => clearTimeout(timer);
  }, [phase]);

  // Snap to the nearest edge after the user drags it
  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | null = null;
    const unlisten = getCurrentWindow().onMoved(() => {
      if (timer) clearTimeout(timer);
      timer = setTimeout(() => {
        snapToEdge(widthRef.current)
          .then((s) => s && setSide(s))
          .catch(() => {});
      }, 350);
    });
    return () => {
      if (timer) clearTimeout(timer);
      unlisten.then((fn) => fn());
    };
  }, []);

  const toggleCollapsed = useCallback(() => {
    setCollapsed((c) => {
      writeJson(COLLAPSED_KEY, !c);
      return !c;
    });
  }, []);

  const focusMain = () => invoke('island_focus_main').catch(() => {});
  const togglePause = () =>
    invoke('island_action', { action: state.is_paused ? 'resume' : 'pause' })
      .then(() => setState((s) => ({ ...s, is_paused: !s.is_paused })))
      .catch(() => {});
  const stop = () => invoke('island_action', { action: 'stop' }).catch(() => {});
  const openTranscript = () => {
    focusMain();
    invoke('island_action', { action: 'close' }).catch(() => {});
  };

  const capsuleClass = `isl ${nativeGlass ? '' : 'isl-fallback'}`;
  // The capsule hugs the screen edge it is snapped to
  const row = { display: 'flex', justifyContent: side === 'right' ? 'flex-end' : 'flex-start' } as const;
  // The chevron points away from the edge to open and towards it to fold
  const outwards = side === 'right' ? 'left' : 'right';
  const inwards = side === 'right' ? 'right' : 'left';

  // ---------- A Google Meet offer ----------
  if (prompt) {
    return (
      <div style={row}>
        <div ref={capsuleRef} className={`${capsuleClass} isl-prompt`} role="alertdialog" data-tauri-drag-region>
          {prompt.kind === 'start' ? (
            <>
              <span className="isl-text" data-tauri-drag-region>
                Нарада в Meet почалась
              </span>
              <span className="isl-sub" data-tauri-drag-region>
                · запис не йде
              </span>
              <button type="button" className="isl-btn isl-btn-ghost isl-x" onClick={() => answerPrompt('no')} title="Не цього разу" aria-label="Не цього разу">
                <XIcon />
              </button>
              <button type="button" className="isl-pill isl-pill-red" onClick={() => answerPrompt('start')}>
                <i aria-hidden="true" />
                Почати запис
              </button>
            </>
          ) : (
            <>
              <span className="isl-text" data-tauri-drag-region>
                Нарада в Meet завершилась
              </span>
              <button type="button" className="isl-pill" onClick={() => answerPrompt('no')}>
                Продовжити
              </button>
              <button type="button" className="isl-pill isl-pill-red" onClick={() => answerPrompt('stop')}>
                Завершити запис
              </button>
            </>
          )}
        </div>
      </div>
    );
  }

  // ---------- After «Завершити» ----------
  if (phase !== 'recording') {
    return (
      <div style={row}>
        <div
          ref={capsuleRef}
          className={capsuleClass}
          style={phase === 'processing' ? { paddingRight: 16 } : undefined}
          role="status"
          data-tauri-drag-region
        >
          {phase === 'processing' ? (
            <>
              <Ring />
              <span className="isl-text" data-tauri-drag-region>
                Обробка запису
              </span>
              {minutesLeft !== null && (
                <span className="isl-sub" data-tauri-drag-region>
                  ще ≈ {minutesLeft} хв
                </span>
              )}
            </>
          ) : (
            <>
              <DoneIcon />
              <span className="isl-text" data-tauri-drag-region>
                Стенограма готова
              </span>
              <button type="button" className="isl-open" onClick={openTranscript}>
                Відкрити
              </button>
            </>
          )}
        </div>
      </div>
    );
  }

  // ---------- Recording ----------
  const paused = state.is_paused;
  const micOff = !sources.micEnabled;
  const callOff = !sources.systemEnabled;
  const micMuted = muted && !micOff;
  const micPercent = levels.mic_rms < SILENCE_RMS ? 0 : meterPercent(levels.mic_rms);
  const callPercent = levels.system_rms < SILENCE_RMS ? 0 : meterPercent(levels.system_rms);
  const callIsMuted = callMuted && !callOff;

  const micTitle = micOff
    ? 'Мікрофон не записується: його вимкнено перед записом'
    : micMuted
      ? 'Мікрофон вимкнено. Натисніть, щоб увімкнути (⇧⌘M)'
      : missing.mic
        ? 'Звук з мікрофона не надходить понад 10 с. Перевірте мікрофон у Параметрах.'
        : 'Мікрофон записується. Натисніть, щоб вимкнути (⇧⌘M)';
  const callTitle = callOff
    ? 'Звук дзвінка не записується: його вимкнено перед записом'
    : callIsMuted
      ? 'Звук дзвінка вимкнено. Натисніть, щоб увімкнути'
    : missing.system
      ? 'Звук дзвінка Google Meet не надходить понад 10 с. Перевірте дозвіл «Запис екрана й аудіо системи» в Параметрах системи → Приватність і безпека.'
      : 'Звук дзвінка записується. Натисніть, щоб вимкнути';

  const dot = <span className={`isl-dot ${paused ? 'isl-dot-paused' : ''}`} aria-label={paused ? 'Пауза' : 'Іде запис'} data-tauri-drag-region />;
  const clock = (
    <button type="button" className={`isl-time ${seconds >= 3600 ? 'isl-time-hours' : ''} ${paused ? 'isl-time-paused' : ''}`} onClick={focusMain} title="Відкрити Протокол">
      {formatClock(seconds)}
    </button>
  );

  if (collapsed) {
    const micClass = micMuted ? 'isl-mini-muted' : missing.mic ? 'isl-src-warn' : micOff ? 'isl-src-off' : '';
    const callClass = callIsMuted ? 'isl-mini-muted' : missing.system ? 'isl-src-warn' : callOff ? 'isl-src-off' : '';
    return (
      <div style={row}>
        <div ref={capsuleRef} className={`${capsuleClass} isl-compact`} data-tauri-drag-region>
          {dot}
          {clock}
          <span className={`isl-mini ${micClass}`} role="img" aria-label={micTitle} title={micTitle} data-tauri-drag-region>
            {missing.mic ? <WarnIcon /> : <MicIcon off={micMuted || micOff} size={15} />}
          </span>
          <span className={`isl-mini ${callClass}`} role="img" aria-label={callTitle} title={callTitle} data-tauri-drag-region>
            {missing.system ? <WarnIcon /> : <CallIcon off={callOff || callIsMuted} size={15} />}
          </span>
          <button type="button" className="isl-btn isl-btn-ghost" onClick={toggleCollapsed} title="Розгорнути" aria-label="Розгорнути">
            <Chevron towards={outwards} />
          </button>
        </div>
      </div>
    );
  }

  return (
    <div style={row}>
      <div
        ref={capsuleRef}
        className={capsuleClass}
        data-tauri-drag-region
      >
        {dot}
        {clock}
        <span className="isl-sep" data-tauri-drag-region />

        {paused ? (
          <span className="isl-src isl-src-off isl-src-paused" data-tauri-drag-region>
            Пауза
          </span>
        ) : (
          <>
            {/* Microphone: a click mutes or unmutes it */}
            <button
              type="button"
              className={`isl-src ${micMuted ? 'isl-src-muted' : ''} ${missing.mic ? 'isl-src-warn' : ''} ${micOff ? 'isl-src-off' : ''}`}
              onClick={() => {
                if (micOff) return;
                if (missing.mic) focusMain();
                else setMuted(!muted);
              }}
              disabled={micOff}
              aria-pressed={micMuted}
              aria-label={micTitle}
              title={micTitle}
            >
              {missing.mic ? <WarnIcon /> : <MicIcon off={micMuted || micOff} />}
              {micOff || micMuted ? null : <Bars percent={missing.mic ? 0 : micPercent} shift={shift} />}
            </button>

            {/* Call audio: a click mutes or unmutes it, as for the microphone */}
            <button
              type="button"
              className={`isl-src ${callIsMuted ? 'isl-src-muted' : ''} ${missing.system ? 'isl-src-warn' : ''} ${callOff ? 'isl-src-off' : ''}`}
              onClick={() => {
                if (callOff) return;
                if (missing.system) focusMain();
                else setCallMuted(!callMuted);
              }}
              disabled={callOff}
              aria-pressed={callIsMuted}
              aria-label={callTitle}
              title={callTitle}
            >
              {missing.system ? <WarnIcon /> : <CallIcon off={callOff || callIsMuted} />}
              {callOff || callIsMuted ? null : <Bars percent={missing.system ? 0 : callPercent} shift={(shift + 2) % BAR_WEIGHTS.length} />}
            </button>
          </>
        )}

        <span className="isl-sep" data-tauri-drag-region />
        <button
          type="button"
          className="isl-btn"
          onClick={togglePause}
          title={paused ? 'Продовжити запис' : 'Пауза'}
          aria-label={paused ? 'Продовжити запис' : 'Пауза'}
        >
          {paused ? <PlayIcon /> : <PauseIcon />}
        </button>
        <button type="button" className="isl-btn isl-btn-stop" onClick={stop} title="Завершити запис" aria-label="Завершити запис">
          <StopIcon />
        </button>
        <button type="button" className="isl-btn isl-btn-ghost" onClick={toggleCollapsed} title="Згорнути" aria-label="Згорнути">
          <Chevron towards={inwards} />
        </button>
      </div>
    </div>
  );
}
