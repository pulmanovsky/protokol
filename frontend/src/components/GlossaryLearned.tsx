'use client';

import { useCallback, useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { toast } from '@/lib/notice';

interface LearnedTerm {
  term: string;
  context: string;
  source: 'edit' | 'auto' | string;
  count: number;
  meetings: number;
  last_seen: string;
  example: string;
  wrong: string[];
}

/**
 * What the dictionary learned on its own: names and terms repeated across
 * meetings («вивчено») and the secretary's corrections («з ваших правок»).
 * Any entry can be removed.
 */
export function GlossaryLearned({ version, onChanged }: { version: number; onChanged?: () => void }) {
  const [items, setItems] = useState<LearnedTerm[] | null>(null);
  const [scan, setScan] = useState<{ meetings: number; learned: number } | null>(null);

  const load = useCallback(() => {
    invoke<LearnedTerm[]>('api_learned_terms').then(setItems).catch(() => setItems([]));
  }, []);

  // Read all meetings each time the page opens (fast, local)
  useEffect(() => {
    invoke<{ meetings: number; learned: number }>('api_learn_from_meetings')
      .then((result) => {
        setScan(result);
        if (result.learned > 0) onChanged?.();
      })
      .catch(() => {})
      .finally(load);
  }, [load, onChanged]);

  useEffect(load, [load, version]);

  const forget = async (term: string) => {
    try {
      await invoke('api_forget_term', { term });
      toast.success(`«${term}» прибрано зі словника`);
      onChanged?.();
      load();
    } catch (error) {
      toast.error('Не вдалося прибрати', { description: String(error) });
    }
  };

  return (
    <section className="card p-6 flex flex-col gap-3">
      <div className="flex flex-col gap-1">
        <h2 className="text-[15px] font-bold text-gray-900">Вивчено автоматично</h2>
        <p className="text-sm text-[color:var(--muted-text)]">
          Імена, назви та терміни з ваших записів і ваших правок. Розпізнавання пише їх правильно, а
          модель знає, хто є хто. Помилкове можна прибрати.
          {scan && ` Проаналізовано записів: ${scan.meetings}.`}
        </p>
      </div>
      {items === null ? null : items.length === 0 ? (
        <p className="text-sm text-[color:var(--muted-text)]">Поки нічого: словник наповниться після кількох записів.</p>
      ) : (
        <ul className="flex flex-col divide-y divide-[var(--hairline,rgba(0,0,0,0.08))]">
          {items.map((item) => (
            <li key={item.term} className="py-2.5 flex items-start justify-between gap-3">
              <div className="flex flex-col gap-0.5 min-w-0">
                <span className="text-sm font-semibold text-gray-900">
                  {item.term}
                  {item.context && <span className="font-normal text-[color:var(--muted-text)]"> — {item.context}</span>}
                </span>
                <span className="text-[12px] text-[color:var(--muted-text)]">
                  {item.source === 'edit' ? 'з ваших правок' : 'вивчено'}
                  {item.meetings > 0 && ` · записів: ${item.meetings}`}
                  {item.count > 0 && ` · згадок: ${item.count}`}
                  {item.wrong?.length > 0 && ` · виправляє: ${item.wrong.join(', ')}`}
                </span>
              </div>
              <button
                type="button"
                onClick={() => forget(item.term)}
                className="h-7 px-2 text-[12px] text-[color:var(--muted-text)] hover:underline flex-none"
              >
                Прибрати
              </button>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
