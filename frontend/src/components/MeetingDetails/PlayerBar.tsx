'use client';

import { Pause, Play, Repeat, RotateCcw, RotateCw } from 'lucide-react';
import { mmss } from '@/lib/timeline';
import { SPEAKER_COLORS } from '@/lib/speakerColors';

export interface PlayerState {
  playing: boolean;
  /** Where the recording is (seconds) */
  current: number;
  duration: number;
  rate: number;
  /** Who is speaking at `current` */
  speaker?: string;
  /** One line is played over and over */
  looping?: boolean;
}

interface Props extends PlayerState {
  onToggle: () => void;
  onSeek: (seconds: number) => void;
  onSkip: (delta: number) => void;
  onRate: (rate: number) => void;
  onStopLoop?: () => void;
  /** «Повтор рядка»: the line being heard (or in focus) over and over */
  onRepeat?: () => void;
  /** Speaker turns for the timeline: [start, end, speaker index] */
  turns?: [number, number, number][];
  /** The line being heard: [start, end] */
  line?: [number, number];
}

export const RATES = [1, 1.25, 1.5, 2];

/**
 * The recording's player under the transcript: 5 seconds back and forward,
 * a timeline coloured by who speaks (the line being heard marked), and
 * «Повтор рядка» and the speed. Space plays or pauses, ←/→ move 5 seconds.
 */
export function PlayerBar({ playing, current, duration, rate, speaker, looping, onToggle, onSeek, onSkip, onRate, onStopLoop, onRepeat, turns, line }: Props) {
  const total = Math.max(duration, current, 1);
  const pct = Math.min(100, (current / total) * 100);
  const nextRate = RATES[(RATES.indexOf(rate) + 1) % RATES.length] ?? 1;

  const seekAt = (e: React.PointerEvent<HTMLDivElement>) => {
    const box = e.currentTarget.getBoundingClientRect();
    onSeek(Math.max(0, Math.min(1, (e.clientX - box.left) / box.width)) * total);
  };

  return (
    <div data-notice-anchor-bottom className="player-bar flex-none border-t border-[var(--hairline,rgba(127,127,127,0.18))] px-3 py-2 flex items-center gap-2" role="group" aria-label="Програвач запису">
      <button type="button" className="player-icon" onClick={() => onSkip(-5)} aria-label="На 5 секунд назад" title="−5 с · ←">
        <RotateCcw className="h-4 w-4" />
      </button>
      <button
        type="button"
        onClick={onToggle}
        className="flex h-10 w-10 flex-none items-center justify-center rounded-full bg-[var(--blue)] text-white hover:brightness-105"
        aria-label={playing ? 'Пауза' : 'Слухати'}
        title="Пробіл"
      >
        {playing ? <Pause className="h-4 w-4" fill="currentColor" /> : <Play className="h-4 w-4 translate-x-[1px]" fill="currentColor" />}
      </button>
      <button type="button" className="player-icon" onClick={() => onSkip(5)} aria-label="На 5 секунд вперед" title="+5 с · →">
        <RotateCw className="h-4 w-4" />
      </button>
      <span className="w-[44px] flex-none text-[12px] tabular-nums text-gray-900">{mmss(current)}</span>
      <div className="flex min-w-[60px] flex-1 flex-col gap-1">
        <span className="truncate text-[11px] leading-none text-[color:var(--muted-text)]">
          {speaker ? <span className="font-semibold text-gray-900">{speaker}</span> : 'Запис'}
          {line ? ` · рядок ${mmss(line[0])}–${mmss(line[1])}` : ''}
          {looping ? ' · повтор увімкнено' : ''}
        </span>
        <div
          className="relative h-2 cursor-pointer overflow-hidden rounded-full bg-[var(--selected)]"
          role="slider"
          aria-label="Місце в записі"
          aria-valuemin={0}
          aria-valuemax={Math.round(total)}
          aria-valuenow={Math.round(current)}
          tabIndex={0}
          onPointerDown={(e) => {
            e.currentTarget.setPointerCapture(e.pointerId);
            seekAt(e);
          }}
          onPointerMove={(e) => {
            if (e.buttons === 1) seekAt(e);
          }}
        >
          {turns?.map(([a, b, k], i) => (
            <span
              key={i}
              className="absolute top-0 h-full opacity-60"
              style={{ left: `${(a / total) * 100}%`, width: `${Math.max(0.15, ((b - a) / total) * 100)}%`, background: SPEAKER_COLORS[k % SPEAKER_COLORS.length] }}
            />
          ))}
          {line && (
            <span
              className="absolute top-0 h-full rounded-sm ring-2 ring-[var(--blue)]"
              style={{ left: `${(line[0] / total) * 100}%`, width: `${Math.max(0.4, ((line[1] - line[0]) / total) * 100)}%` }}
            />
          )}
          <span className="absolute top-0 h-full w-[2px] -translate-x-1/2 bg-gray-900" style={{ left: `${pct}%` }} />
        </div>
      </div>
      <span className="w-[44px] flex-none text-right text-[12px] tabular-nums text-[color:var(--muted-text)]">{duration > 0 ? mmss(duration) : '--:--'}</span>
      {onRepeat && (
        <button
          type="button"
          onClick={looping ? onStopLoop : onRepeat}
          className={`inline-flex h-8 flex-none items-center gap-1.5 rounded-full px-3 text-[12px] font-semibold ${looping ? 'bg-[var(--blue)] text-white' : 'bg-[var(--selected)] text-gray-900'}`}
          title="Повтор рядка · R"
        >
          <Repeat className="h-3.5 w-3.5" /> <span className="hidden @[700px]:inline">Повтор рядка</span> <kbd>R</kbd>
        </button>
      )}
      <button type="button" onClick={() => onRate(nextRate)} className="h-8 flex-none rounded-full bg-[var(--selected)] px-3 text-[12px] font-semibold tabular-nums text-gray-900" aria-label="Швидкість" title="Швидкість">
        {rate}×
      </button>
    </div>
  );
}
