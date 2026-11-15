'use client';

import React, { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { plural } from '@/lib/plural';
import { Quality, QualityMeeting, dayText, num } from '@/lib/learning';

/** Meetings in the chart and in each tile */
const CHART_MEETINGS = 16;
const TILE_MEETINGS = 5;

/** Corrected words per 100 over several meetings, longer meetings weighing more */
function pooled(ms: QualityMeeting[]): number {
  const words = ms.reduce((n, m) => n + m.words, 0);
  const corrected = ms.reduce((n, m) => n + (m.corrected * m.words) / 100, 0);
  return words ? (100 * corrected) / words : 0;
}

function lastText(n: number) {
  return n === 1 ? 'Остання нарада' : `Останні ${n} ${plural(n, 'нарада', 'наради', 'нарад')}`;
}

function Tile({ label, value, muted = false }: { label: string; value: string; muted?: boolean }) {
  return (
    <div className="learn-tile">
      <span className="text-xs text-[color:var(--muted-text)]">{label}</span>
      <span className="learn-tile-value" style={muted ? { color: 'var(--muted-text)' } : undefined}>
        {value}
      </span>
    </div>
  );
}

function Chart({ meetings, trainings }: { meetings: QualityMeeting[]; trainings: string[] }) {
  const [hover, setHover] = useState<number | null>(null);
  const top = Math.max(10, Math.ceil(Math.max(...meetings.map((m) => m.corrected)) / 5) * 5);
  const step = top > 30 ? 10 : 5;
  const ticks: number[] = [];
  for (let v = 0; v <= top; v += step) ticks.push(v);
  // A training is marked before the first meeting on or after its day
  const marks = new Map<number, string>();
  for (const t of trainings) {
    const at = meetings.findIndex((m) => m.date >= t);
    if (at > 0 && !marks.has(at)) marks.set(at, t);
  }
  const everyOther = meetings.length > 10;
  const tuned = meetings.filter((m) => m.tuned).length;

  return (
    <section className="learn-card" aria-labelledby="quality-chart-title">
      <h3 id="quality-chart-title" className="text-[14px] font-semibold text-gray-900">
        Виправлено слів за нарадами, %
      </h3>
      <div className="flex gap-4 text-xs text-[color:var(--muted-text)]" style={{ marginLeft: 36 }}>
        <span>
          <span className="learn-swatch learn-bar-base" aria-hidden="true" />
          Звичайна модель
        </span>
        <span>
          <span className="learn-swatch learn-bar-tuned" aria-hidden="true" />
          Донавчена модель
        </span>
      </div>
      <div
        className="relative"
        style={{ height: 200, marginLeft: 36, marginTop: 14 }}
        role="img"
        aria-label={`Частка виправлених слів на ${meetings.length} останніх перевірених нарадах; донавчена модель розпізнавала ${tuned} з них`}
      >
        {ticks.map((v) => (
          <React.Fragment key={v}>
            <div className="absolute left-0 right-0 h-px bg-[var(--hairline)]" style={{ bottom: `${(100 * v) / top}%` }} aria-hidden="true" />
            <span
              className="absolute text-[11px] tabular-nums text-[color:var(--muted-text)] text-right"
              style={{ left: -36, width: 28, bottom: `${(100 * v) / top}%`, transform: 'translateY(50%)' }}
              aria-hidden="true"
            >
              {v === 0 ? '0' : `${v}%`}
            </span>
          </React.Fragment>
        ))}
        <div className="absolute inset-0 flex items-end gap-2.5 px-1.5">
          {meetings.map((m, i) => (
            <div
              key={`${m.date}-${i}`}
              className="relative flex h-full flex-1 flex-col justify-end"
              onMouseEnter={() => setHover(i)}
              onMouseLeave={() => setHover((h) => (h === i ? null : h))}
            >
              {marks.has(i) && (
                <>
                  <div className="absolute bottom-0 border-l border-dashed border-[color:var(--muted-text)]" style={{ left: -6, top: -10 }} aria-hidden="true" />
                  <span className="absolute whitespace-nowrap text-[11px] text-[color:var(--muted-text)]" style={{ left: -2, top: -14 }} aria-hidden="true">
                    Донавчання {dayText(marks.get(i)!)}
                  </span>
                </>
              )}
              <div
                className={m.tuned ? 'learn-bar-tuned' : 'learn-bar-base'}
                style={{ height: `${Math.max(1, (100 * m.corrected) / top)}%`, borderRadius: '4px 4px 0 0', opacity: hover == null || hover === i ? 1 : 0.55 }}
              />
              {hover === i && (
                <div
                  className="pointer-events-none absolute z-10 whitespace-nowrap rounded-lg border border-[var(--hairline)] bg-[var(--surface-bg)] px-2.5 py-1.5 text-xs shadow-lg"
                  style={{ bottom: `calc(${Math.max(1, (100 * m.corrected) / top)}% + 6px)`, left: '50%', transform: 'translateX(-50%)' }}
                  role="tooltip"
                >
                  <div className="font-semibold tabular-nums">
                    {dayText(m.date)} · {num(m.corrected)}%
                  </div>
                  <div className="text-[color:var(--muted-text)]">
                    {m.tuned ? 'Донавчена модель' : 'Звичайна модель'} · {m.words} {plural(m.words, 'слово', 'слова', 'слів')}
                  </div>
                </div>
              )}
            </div>
          ))}
        </div>
      </div>
      <div className="flex gap-2.5 px-1.5 text-[11px] tabular-nums text-[color:var(--muted-text)]" style={{ marginLeft: 36 }} aria-hidden="true">
        {meetings.map((m, i) => (
          <span key={`${m.date}-${i}`} className="flex-1 text-center">
            {everyOther && (meetings.length - 1 - i) % 2 ? '' : dayText(m.date)}
          </span>
        ))}
      </div>
      <table className="sr-only">
        <caption>Виправлено слів за нарадами</caption>
        <thead>
          <tr>
            <th>Дата</th>
            <th>Виправлено, %</th>
            <th>Модель</th>
          </tr>
        </thead>
        <tbody>
          {meetings.map((m, i) => (
            <tr key={`${m.date}-${i}`}>
              <td>{dayText(m.date)}</td>
              <td>{num(m.corrected)}</td>
              <td>{m.tuned ? 'донавчена' : 'звичайна'}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </section>
  );
}

/** Settings → Пам'ять → «Якість розпізнавання» */
export function LearningQuality() {
  const [q, setQ] = useState<Quality | null>(null);
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    invoke<Quality>('api_learning_quality')
      .then(setQ)
      .catch(() => setFailed(true));
  }, []);

  const lead = (
    <p className="settings-help" style={{ marginTop: -8 }}>
      Частка слів стенограми, які довелося виправити під час перевірки. Рахуються лише перевірені наради.
    </p>
  );
  if (failed) {
    return (
      <div className="flex flex-col gap-6">
        {lead}
        <p className="settings-help">Не вдалося порахувати. Спробуйте відкрити сторінку ще раз.</p>
      </div>
    );
  }
  if (!q) return lead;
  if (q.meetings.length === 0) {
    return (
      <div className="flex flex-col gap-6">
        {lead}
        <p className="settings-help">Перевірених нарад ще немає. Числа з'являться після перевірки першої стенограми.</p>
      </div>
    );
  }

  const tuned = q.meetings.filter((m) => m.tuned).slice(-TILE_MEETINGS);
  const firstTuned = q.meetings.findIndex((m) => m.tuned);
  const before = (firstTuned < 0 ? q.meetings : q.meetings.slice(0, firstTuned)).filter((m) => !m.tuned).slice(-TILE_MEETINGS);
  const shown = q.meetings.slice(-CHART_MEETINGS);

  return (
    <div className="flex flex-col gap-5">
      {lead}
      <div className="grid grid-cols-2 gap-3">
        {tuned.length > 0 ? (
          <>
            <Tile label={`${lastText(tuned.length)}, донавчена модель`} value={`${num(pooled(tuned))}%`} />
            {before.length > 0 ? (
              <Tile label="До донавчання, звичайна модель" value={`${num(pooled(before))}%`} muted />
            ) : (
              <Tile label="До донавчання, звичайна модель" value="—" muted />
            )}
          </>
        ) : (
          <>
            <Tile label={`${lastText(before.length)}, звичайна модель`} value={`${num(pooled(before))}%`} />
            <Tile label="Донавчена модель ще не працювала" value="—" muted />
          </>
        )}
      </div>
      <Chart meetings={shown} trainings={q.trainings} />
    </div>
  );
}
