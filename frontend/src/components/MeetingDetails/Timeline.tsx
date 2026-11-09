'use client';

import { useEffect, useMemo, useRef, useState } from 'react';
import { ChevronDown, ChevronUp, Pause, Play } from 'lucide-react';
import { mmss } from '@/lib/timeline';
import { followPlayhead, readPlayhead } from '@/lib/playhead';
import { RateMenu } from './RateMenu';
import { perfAdd, perfCount, perfOn } from '@/lib/perf';
import { invoke } from '@tauri-apps/api/core';
import { clampView, drawWave, silences, type View, type Waveform } from './waveform';

export interface TimelineLine {
  id: string;
  start: number;
  end: number;
  speaker: string;
}

interface Props {
  lines: TimelineLine[];
  colorOf: (speaker: string) => string;
  duration: number;
  current: number;
  playing: boolean;
  rate: number;
  /** The words chosen in the text, as a stretch of time */
  band: [number, number] | null;
  /** For the waveform of the simple timeline */
  meetingId?: string;
  onToggle: () => void;
  onSeek: (seconds: number) => void;
  onSkip: (delta: number) => void;
  onRate: (rate: number) => void;
  /** A stretch chosen by dragging on the timeline */
  onChooseRange: (start: number, end: number) => void;
  /** The border between two people's neighbouring lines moved to `t` */
  onMoveBorder: (leftId: string, rightId: string, t: number) => void;
  /** The line being heard (outlined) */
  playingId?: string;
  /** Pointing at a line's bar / clicking it */
  onHoverLine?: (id: string | null) => void;
  onLineClick?: (id: string) => void;
  /** Dragging the playhead (scrubbing): the text follows */
  onScrub?: (t: number) => void;
}

const WINDOW_S = 120;
const LABEL_W = 170;
const LANE_H = 28;
const RULER_H = 22;
const RATES = [0.75, 1, 1.5, 2];

/**
 * The recording under the transcript: the only play control (▶, ⌥Пробіл,
 * ⏯), and one lane per person with their lines in their colour. A click
 * moves the playhead without playing; dragging chooses those words in
 * the text; the border between two people can be dragged.
 */
export function Timeline({ meetingId, lines, colorOf, duration, current, playing, rate, band, onToggle, onSeek, onSkip, onRate, onChooseRange, onMoveBorder, playingId, onHoverLine, onLineClick, onScrub }: Props) {
  perfCount('render:timeline');
  const people = useMemo(() => {
    const out: string[] = [];
    for (const l of lines) if (!out.includes(l.speaker)) out.push(l.speaker);
    return out;
  }, [lines]);
  const scrollRef = useRef<HTMLDivElement>(null);
  // Compact (one lane for everyone) or expanded (a lane per person); remembered
  const [expanded, setExpandedState] = useState(() => {
    try {
      return window.localStorage.getItem('protokol.timeline.expanded') === '1';
    } catch {
      return false;
    }
  });
  const setExpanded = (v: boolean) => {
    setExpandedState(v);
    try {
      window.localStorage.setItem('protokol.timeline.expanded', v ? '1' : '0');
    } catch {
      // not remembered: fine
    }
  };
  const [width, setWidth] = useState(800);
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setWidth(el.clientWidth));
    ro.observe(el);
    setWidth(el.clientWidth);
    return () => ro.disconnect();
  }, [expanded]);
  const total = Math.max(duration, 1);
  // Zoom: the seconds shown at once (pinch on the trackpad)
  const [windowS, setWindowS] = useState(WINDOW_S);
  const pps = Math.max(1, width) / Math.min(windowS, total);
  const [scrollLeft, setScrollLeft] = useState(0);
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const onWheel = (e: WheelEvent) => {
      if (!e.ctrlKey) return;
      e.preventDefault();
      setWindowS((w) => Math.max(15, Math.min(Math.max(30, total), w * (1 + e.deltaY * 0.01))));
    };
    el.addEventListener('wheel', onWheel, { passive: false });
    return () => el.removeEventListener('wheel', onWheel);
  }, [total]);
  const inner = Math.max(width, total * pps);
  const x = (t: number) => t * pps;
  const tAt = (clientX: number) => {
    const el = scrollRef.current!;
    const box = el.getBoundingClientRect();
    return Math.max(0, Math.min(total, (clientX - box.left + el.scrollLeft) / pps));
  };
  // While playing the playhead moves every frame from the player's clock
  const headRef = useRef<HTMLSpanElement>(null);
  const ppsRef = useRef(1);
  ppsRef.current = pps;
  useEffect(() => {
    if (!playing) return;
    let shown = -1;
    return followPlayhead((t) => {
      // Moved only when it moves by a pixel
      const px = Math.round(t * ppsRef.current);
      if (px === shown || !headRef.current) return;
      shown = px;
      // Moved by the compositor alone (no layout, no repaint of the lanes)
      const t0 = perfOn() ? performance.now() : 0;
      headRef.current.style.transform = `translateX(${px}px)`;
      if (t0) perfAdd('playhead', performance.now() - t0, true);
    });
  }, [playing]);
  // The playhead stays in view
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const px = x(current);
    if (px < el.scrollLeft + 20 || px > el.scrollLeft + el.clientWidth - 40) el.scrollLeft = Math.max(0, px - el.clientWidth * 0.3);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [current, pps]);

  // Click = move the playhead; drag = scrub (the playhead follows the
  // pointer, like a video); ⇧+drag = choose the words of a stretch
  const [draft, setDraft] = useState<[number, number] | null>(null);
  const [scrubT, setScrubT] = useState<number | null>(null);
  const down = useRef<{ x: number; t: number; shift: boolean; moved: boolean } | null>(null);
  const lastScrub = useRef(0);
  const scrubTo = (t: number, final = false) => {
    setScrubT(t);
    if (headRef.current) headRef.current.style.transform = `translateX(${Math.round(t * pps)}px)`;
    const now = performance.now();
    if (final || now - lastScrub.current > 60) {
      lastScrub.current = now;
      (onScrub ?? onSeek)(t);
    }
  };
  const onDown = (e: React.PointerEvent) => {
    if ((e.target as HTMLElement).closest('.tl-border')) return;
    e.currentTarget.setPointerCapture(e.pointerId);
    down.current = { x: e.clientX, t: tAt(e.clientX), shift: e.shiftKey && expanded, moved: false };
  };
  const onMove = (e: React.PointerEvent) => {
    const d = down.current;
    if (!d || e.buttons !== 1) return;
    if (!d.moved && Math.abs(e.clientX - d.x) <= 4) return;
    d.moved = true;
    const t = tAt(e.clientX);
    if (d.shift) setDraft([Math.min(t, d.t), Math.max(t, d.t)]);
    else scrubTo(t);
  };
  const onUp = (e: React.PointerEvent) => {
    const d = down.current;
    if (!d) return;
    const bar = (e.target as HTMLElement).closest('[data-line]');
    if (d.moved && d.shift && draft) onChooseRange(draft[0], draft[1]);
    else if (d.moved) scrubTo(tAt(e.clientX), true);
    else if (bar && onLineClick) onLineClick(bar.getAttribute('data-line')!);
    else onSeek(tAt(e.clientX));
    down.current = null;
    setDraft(null);
    setScrubT(null);
  };
  // The whole meeting in a thin strip: where the zoomed part is; drag to go anywhere
  const overRef = useRef<HTMLDivElement>(null);
  const overDrag = (e: React.PointerEvent) => {
    const box = overRef.current!.getBoundingClientRect();
    const t = Math.max(0, Math.min(total, ((e.clientX - box.left) / box.width) * total));
    scrubTo(t, e.type === 'pointerup');
    const el = scrollRef.current;
    if (el) el.scrollLeft = Math.max(0, t * pps - el.clientWidth / 2);
  };
  void LABEL_W;
  // The simple timeline: one thin bar for the whole meeting; click or drag to go there
  const scrubRef = useRef<HTMLDivElement>(null);
  const shownViewRef = useRef<View>({ start: 0, length: 1 });
  const r0 = (el: HTMLElement) => Math.max(1, el.getBoundingClientRect().width);
  const scrubDrag = (e: React.PointerEvent) => {
    const box = scrubRef.current!.getBoundingClientRect();
    const f = Math.max(0, Math.min(1, (e.clientX - box.left) / box.width));
    scrubTo(Math.max(0, Math.min(total, shownView.start + f * shownView.length)), e.type === 'pointerup');
  };
  // Zoom: the part of the recording shown (the whole of it until zoomed)
  const [view, setView] = useState<View | null>(null);
  useEffect(() => setView(null), [meetingId]);
  const shownView: View = view ?? { start: 0, length: total };
  const zoomed = !!view && view.length < total - 0.5;
  const zoomBy = (factor: number, at = 0.5) =>
    setView((v) => {
      const now = v ?? { start: 0, length: total };
      const length = now.length * factor;
      if (length >= total) return null;
      return clampView({ start: now.start + (now.length - length) * at, length }, total);
    });
  // The buttons keep the playhead where it is on the bar (the middle when it is out of view)
  const zoomAtHead = (factor: number) => {
    const t = scrubT ?? current;
    const inView = t >= shownView.start && t <= shownView.start + shownView.length;
    zoomBy(factor, inView ? (t - shownView.start) / shownView.length : 0.5);
  };
  const panBy = (seconds: number) => setView((v) => (v ? clampView({ start: v.start + seconds, length: v.length }, total) : v));
  // While playing the view turns the page when the playhead reaches its edge
  useEffect(() => {
    if (!playing) return;
    setView((v) => {
      if (!v || (current >= v.start && current <= v.start + v.length * 0.97)) return v;
      return clampView({ start: current - v.length * 0.05, length: v.length }, total);
    });
  }, [current, playing, total]);
  shownViewRef.current = shownView;
  const playedPart = Math.max(0, Math.min(1, ((scrubT ?? current) - shownView.start) / shownView.length));
  const headVisible = (scrubT ?? current) >= shownView.start && (scrubT ?? current) <= shownView.start + shownView.length;
  const shownPct = `${100 * playedPart}%`;
  // The waveform: asked for once, drawn once per size and theme; the played
  // part is a blue copy clipped by a width, so playback draws nothing
  const [wave, setWave] = useState<Waveform | null>(null);
  useEffect(() => {
    setWave(null);
    if (!meetingId) return;
    let live = true;
    invoke<Waveform | null>('api_waveform', { meetingId })
      .then((w) => live && setWave(w && w.data.length ? w : null))
      .catch(() => {});
    return () => {
      live = false;
    };
  }, [meetingId]);
  const greyRef = useRef<HTMLCanvasElement>(null);
  const blueRef = useRef<HTMLCanvasElement>(null);
  useEffect(() => {
    const box = scrubRef.current;
    if (!box || !wave || expanded) return;
    const draw = () => {
      const { width, height } = box.getBoundingClientRect();
      if (width < 1 || !greyRef.current || !blueRef.current) return;
      const css = getComputedStyle(box);
      const silent = css.getPropertyValue('--wave-silent').trim() || 'rgba(127,127,127,.35)';
      drawWave(greyRef.current, wave, shownView, width, height, css.getPropertyValue('--wave-bar').trim() || 'rgba(127,127,127,.6)', silent);
      drawWave(blueRef.current, wave, shownView, width, height, css.getPropertyValue('--blue').trim() || '#2563eb', silent);
    };
    draw();
    const ro = new ResizeObserver(draw);
    ro.observe(box);
    // The theme is a class on the root element (ThemeContext)
    const attrs = new MutationObserver(draw);
    attrs.observe(document.documentElement, { attributes: true, attributeFilter: ['class'] });
    return () => {
      ro.disconnect();
      attrs.disconnect();
    };
  }, [wave, expanded, shownView.start, shownView.length]);
  // Pinch or ⌘-scroll zooms around the pointer; a sideways scroll moves the view
  useEffect(() => {
    const box = scrubRef.current;
    if (!box || !wave || expanded) return;
    const onWheel = (e: WheelEvent) => {
      if (e.ctrlKey || e.metaKey) {
        e.preventDefault();
        const r = box.getBoundingClientRect();
        zoomBy(Math.exp(e.deltaY * 0.012), Math.max(0, Math.min(1, (e.clientX - r.left) / r.width)));
      } else if (Math.abs(e.deltaX) > Math.abs(e.deltaY)) {
        e.preventDefault();
        panBy((e.deltaX / r0(box)) * shownViewRef.current.length);
      }
    };
    box.addEventListener('wheel', onWheel, { passive: false });
    return () => box.removeEventListener('wheel', onWheel);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [wave, expanded, total]);
  const quiet = useMemo(() => (wave ? silences(wave) : []), [wave]);
  const scrubber = (
    <div
      ref={scrubRef}
      className={`tl-scrub${wave ? ' tl-scrub-wave' : ''}`}
      role="slider"
      tabIndex={0}
      aria-label="Місце в записі"
      aria-valuemin={0}
      aria-valuemax={Math.round(total)}
      aria-valuenow={Math.round(scrubT ?? current)}
      aria-valuetext={mmss(scrubT ?? current)}
      onPointerDown={(e) => {
        e.currentTarget.setPointerCapture(e.pointerId);
        scrubDrag(e);
      }}
      onPointerMove={(e) => e.buttons === 1 && scrubDrag(e)}
      onPointerUp={(e) => {
        scrubDrag(e);
        setScrubT(null);
      }}
      onKeyDown={(e) => {
        if (e.key === 'ArrowLeft' || e.key === 'ArrowRight') {
          e.preventDefault();
          onSkip(e.key === 'ArrowLeft' ? -5 : 5);
        }
      }}
    >
      {wave ? (
        <>
          <canvas ref={greyRef} className="tl-wave" aria-hidden="true" />
          <span className="tl-wave-played" style={{ width: shownPct }}>
            <canvas ref={blueRef} className="tl-wave" aria-hidden="true" />
          </span>
          {quiet.map((q) => {
            const from = Math.max(q.from, shownView.start);
            const to = Math.min(q.to, shownView.start + shownView.length);
            if (to <= from) return null;
            return (
              <span
                key={q.from}
                className="tl-wave-quiet"
                style={{ left: `${(100 * (from - shownView.start)) / shownView.length}%`, width: `${(100 * (to - from)) / shownView.length}%` }}
                title={`тиша · ${mmss(q.to - q.from)}`}
              />
            );
          })}
          {zoomed && (
            <span className="tl-wave-range" aria-hidden="true">
              {mmss(shownView.start)}–{mmss(shownView.start + shownView.length)}
            </span>
          )}
        </>
      ) : (
        <span className="tl-scrub-track">
          <span className="tl-scrub-fill" style={{ width: shownPct }} />
        </span>
      )}
      {headVisible && <span className="tl-scrub-head" style={{ left: shownPct }} />}
    </div>
  );
  // Borders between neighbouring lines of two people
  const borders = useMemo(() => {
    const out: { left: TimelineLine; right: TimelineLine }[] = [];
    for (let i = 0; i + 1 < lines.length; i++) {
      const a = lines[i];
      const b = lines[i + 1];
      if (a.speaker !== b.speaker && Math.abs(b.start - a.end) < 0.6) out.push({ left: a, right: b });
    }
    return out;
  }, [lines]);
  const [dragBorder, setDragBorder] = useState<{ left: string; right: string; t: number } | null>(null);

  const ticks = useMemo(() => {
    const step = WINDOW_S >= 120 ? 30 : 10;
    const out: number[] = [];
    for (let t = 0; t <= total; t += step) out.push(t);
    return out;
  }, [total]);
  const shownBand = draft ?? band;

  const overview = (
    <div
      ref={overRef}
      className="tl-overview"
      title="Увесь запис: потягніть, щоб перейти в будь-яке місце"
      onPointerDown={(e) => {
        e.currentTarget.setPointerCapture(e.pointerId);
        overDrag(e);
      }}
      onPointerMove={(e) => e.buttons === 1 && overDrag(e)}
      onPointerUp={(e) => {
        overDrag(e);
        setScrubT(null);
      }}
    >
      {lines.map((l) => (
        <span key={l.id} style={{ left: `${(100 * l.start) / total}%`, width: `${Math.max(0.15, (100 * (l.end - l.start)) / total)}%`, background: colorOf(l.speaker) }} />
      ))}
      <span className="tl-overview-window" style={{ left: `${(100 * scrollLeft) / Math.max(1, inner)}%`, width: `${(100 * Math.max(1, width)) / Math.max(1, inner)}%` }} />
      <span className="tl-overview-head" style={{ left: `${(100 * (scrubT ?? current)) / total}%` }} />
    </div>
  );
  const bar = (l: TimelineLine) => (
    <span
      key={l.id}
      data-line={l.id}
      title={l.speaker}
      className={`tl-bar ${l.id === playingId ? 'tl-bar-playing' : ''}`}
      style={{ left: x(l.start), width: Math.max(2, x(l.end) - x(l.start)), background: colorOf(l.speaker) }}
      onPointerEnter={() => onHoverLine?.(l.id)}
      onPointerLeave={() => onHoverLine?.(null)}
    />
  );
  const track = (
    <div className="tl-scroll custom-scrollbar" ref={scrollRef} onPointerDown={onDown} onPointerMove={onMove} onPointerUp={onUp} onScroll={(e) => setScrollLeft(e.currentTarget.scrollLeft)}>
      <div className="tl-inner" style={{ width: inner }}>
        {expanded && (
          <div className="tl-ruler">
            {ticks.map((t, k) => (
              // The first and a last label at the track's ends sit inside it, not half cut off
              <span key={t} style={{ left: x(t), transform: k === 0 ? 'none' : total - t < 4 ? 'translateX(-100%)' : undefined }}>
                {mmss(t)}
              </span>
            ))}
          </div>
        )}
        {expanded ? (
          people.map((p) => (
            <div key={p} className="tl-lane">
              {lines.filter((l) => l.speaker === p).map(bar)}
            </div>
          ))
        ) : (
          <div className="tl-lane tl-lane-one">{lines.map(bar)}</div>
        )}
        {expanded &&
          borders.map(({ left, right }) => {
            const lane = people.indexOf(left.speaker);
            const lane2 = people.indexOf(right.speaker);
            const top = RULER_H + Math.min(lane, lane2) * LANE_H;
            const height = (Math.abs(lane - lane2) + 1) * LANE_H - 4;
            const t = dragBorder?.left === left.id ? dragBorder.t : left.end;
            return (
              <span
                key={left.id}
                className="tl-border"
                title="Потягніть, щоб посунути межу між людьми"
                style={{ left: x(t) - 3, top, height }}
                onPointerDown={(e) => {
                  e.stopPropagation();
                  e.currentTarget.setPointerCapture(e.pointerId);
                  setDragBorder({ left: left.id, right: right.id, t: left.end });
                }}
                onPointerMove={(e) => {
                  if (dragBorder?.left === left.id && e.buttons === 1) setDragBorder({ ...dragBorder, t: Math.max(left.start + 0.2, Math.min(right.end - 0.2, tAt(e.clientX))) });
                }}
                onPointerUp={(e) => {
                  e.stopPropagation();
                  if (dragBorder?.left === left.id && Math.abs(dragBorder.t - left.end) > 0.3) onMoveBorder(left.id, right.id, dragBorder.t);
                  setDragBorder(null);
                }}
              />
            );
          })}
        {shownBand && <span className="tl-band" style={{ left: x(shownBand[0]), width: Math.max(2, x(shownBand[1]) - x(shownBand[0])) }} />}
        <span ref={headRef} className="tl-playhead" style={{ transform: `translateX(${Math.round(x(scrubT ?? current))}px)` }}>
          {scrubT != null && <span className="tl-bubble">{mmss(scrubT)}</span>}
        </span>
      </div>
    </div>
  );
  const transport = (
    <div className="tl-transport">
      <button type="button" className="tl-play" onClick={onToggle} aria-label={playing ? 'Пауза' : 'Слухати'} title="Слухати / пауза · ⌥Пробіл або ⏯">
        {playing ? <Pause className="h-3.5 w-3.5" fill="currentColor" /> : <Play className="h-3.5 w-3.5 translate-x-[1px]" fill="currentColor" />}
      </button>
      <span className="tl-time">
        {mmss(scrubT ?? current)} <span>/ {mmss(total)}</span>
      </span>
      <button type="button" className="tl-btn" aria-label="5 секунд назад" onClick={() => onSkip(-5)}>
        −5 с
      </button>
      <RateMenu rate={rate} rates={RATES} onRate={onRate} />
      <button type="button" className="tl-btn" aria-label="5 секунд вперед" onClick={() => onSkip(5)}>
        +5 с
      </button>
    </div>
  );
  const chevron = (
    <button type="button" className="tl-chevron" onClick={() => setExpanded(!expanded)} aria-label={expanded ? 'Згорнути таймлайн' : 'Розгорнути таймлайн'} title={expanded ? 'Згорнути до простої смуги' : 'Розгорнути: доріжка для кожної людини'}>
      {expanded ? <ChevronDown className="h-4 w-4" /> : <ChevronUp className="h-4 w-4" />}
    </button>
  );
  const expandedH = Math.min(typeof window === 'undefined' ? 400 : window.innerHeight * 0.4, 44 + 22 + RULER_H + people.length * LANE_H + 16);

  return (
    <footer className={`tl ${expanded ? 'tl-expanded' : 'tl-compact'}`} style={{ height: expanded ? expandedH : 52 }}>
      {expanded ? (
        <>
          <div className="tl-top">
            {transport}
            <div className="flex-1" />
            {chevron}
          </div>
          <div className="tl-overview-row">
            <span className="tl-overview-label">Увесь запис</span>
            {overview}
          </div>
          <div className="tl-lanes">
            <div className="tl-names">
              <span className="tl-ruler-gap" />
              {people.map((p) => (
                <span key={p} className="tl-name" title={p}>
                  <span className="tl-dot" style={{ background: colorOf(p) }} />
                  {p}
                </span>
              ))}
            </div>
            {track}
          </div>
        </>
      ) : (
        <div className="tl-top tl-top-compact">
          {transport}
          {scrubber}
          {wave && (
            <div className="tl-zoom">
              <button type="button" aria-label="Зменшити масштаб" title="Зменшити · щипок або ⌘ + прокрутка" disabled={!view} onClick={() => zoomAtHead(2)}>
                −
              </button>
              <button type="button" aria-label="Збільшити масштаб" title="Збільшити · щипок або ⌘ + прокрутка" disabled={shownView.length <= Math.min(10, total) + 0.01} onClick={() => zoomAtHead(0.5)}>
                +
              </button>
            </div>
          )}
          {chevron}
        </div>
      )}
    </footer>
  );
}

export const TIMELINE_LABEL_W = LABEL_W;
