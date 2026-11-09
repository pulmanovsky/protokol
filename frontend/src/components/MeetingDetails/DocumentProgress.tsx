"use client";

import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';

/** The generator's step in words: «Report summary 3/23» → «Стискаю доповіді: 3 з 23» */
function describe(stage: string): string {
  const m = stage.match(/^(.*?)\s*(\d+)\/(\d+)$/);
  const [name, n, total] = m ? [m[1], m[2], m[3]] : [stage, '', ''];
  const of = n ? `: ${n} з ${total}` : '';
  if (name.startsWith('Overall summary')) return 'Загальний підсумок';
  if (name.startsWith('Report summary')) return `Підсумовування виступів${of}`;
  if (name.startsWith('Reports')) return `Поділ на доповіді${of}`;
  if (name.startsWith('Decisions and tasks')) return 'Пошук рішень і завдань';
  if (name.startsWith('Decisions')) return 'Пошук рішень';
  if (name.startsWith('Tasks')) return 'Пошук завдань';
  if (name.startsWith('Speaker names')) return 'Підписування спікерів';
  if (name.startsWith('Control')) return `Перевірка завдань попередньої наради${of}`;
  return 'Підготовка документа';
}

/** Progress of the document being written, from the generator's current step */
export function DocumentProgress() {
  const [stage, setStage] = useState('');
  useEffect(() => {
    const tick = () => invoke<string>('api_summary_stage').then(setStage).catch(() => {});
    tick();
    const id = window.setInterval(tick, 1500);
    return () => window.clearInterval(id);
  }, []);
  const m = stage.match(/(\d+)\/(\d+)$/);
  const pct = m ? Math.round((100 * Number(m[1])) / Number(m[2])) : null;
  return (
    <div className="w-72 text-center" role="status">
      <p className="text-sm font-semibold text-gray-900">Створення документа</p>
      <p className="mt-1 text-sm text-[color:var(--muted-text)]">{stage ? describe(stage) : 'Підготовка документа'}…</p>
      <div className="mt-3 h-1 w-full overflow-hidden rounded-full bg-[var(--selected)]" aria-hidden>
        <div
          className={`h-1 rounded-full bg-[var(--blue)] transition-all duration-500 ${pct === null ? 'animate-pulse w-1/3' : ''}`}
          style={pct === null ? undefined : { width: `${Math.max(4, pct)}%` }}
        />
      </div>
      <p className="mt-2 text-xs text-[color:var(--muted-text)]">Займає кілька хвилин. Можна працювати далі.</p>
    </div>
  );
}
