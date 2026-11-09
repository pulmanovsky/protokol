'use client';

import { useCallback, useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { toast } from '@/lib/notice';
import { Switch } from './ui/switch';

interface Correction {
  wrong: string;
  right: string;
  enabled: boolean;
}

/**
 * «Вивчені виправлення»: what the user corrected in transcripts and asked
 * to remember. New recordings get these replacements right after
 * recognition; the right spellings also help recognition itself.
 */
export function LearnedCorrections({ version }: { version: number }) {
  const [items, setItems] = useState<Correction[] | null>(null);

  const load = useCallback(() => {
    invoke<Correction[]>('api_corrections').then(setItems).catch(() => setItems([]));
  }, []);
  useEffect(load, [load, version]);

  const toggle = async (c: Correction) => {
    try {
      await invoke('api_set_correction_enabled', { wrong: c.wrong, enabled: !c.enabled });
      load();
    } catch (e) {
      toast.error(`${e}`);
    }
  };
  const remove = async (c: Correction) => {
    try {
      await invoke('api_delete_correction', { wrong: c.wrong });
      toast.success(`Прибрано з пам'яті: «${c.wrong}» → «${c.right}»`);
      load();
    } catch (e) {
      toast.error(`${e}`);
    }
  };

  return (
    <section className="card p-6 flex flex-col gap-3">
      <div className="flex flex-col gap-1">
        <h2 className="text-[15px] font-bold text-gray-900">Вивчені виправлення</h2>
        <p className="text-sm text-[color:var(--muted-text)]">
          Те, що ви виправили у стенограмі й попросили запам'ятати. У нових записах це виправляється одразу після
          розпізнавання. Щоб виправити рядок, двічі клацніть по ньому у стенограмі.
        </p>
      </div>
      {items === null ? null : items.length === 0 ? (
        <p className="text-sm text-[color:var(--muted-text)]">Поки нічого.</p>
      ) : (
        <ul className="flex flex-col divide-y divide-[var(--hairline,rgba(0,0,0,0.08))]">
          {items.map((c) => (
            <li key={c.wrong} className="py-2.5 flex items-center justify-between gap-3">
              <span className={`text-sm min-w-0 ${c.enabled ? 'text-gray-900' : 'text-[color:var(--muted-text)]'}`}>
                <span className="line-through decoration-[color:var(--muted-text)]">{c.wrong}</span>
                <span className="mx-1.5 text-[color:var(--muted-text)]">→</span>
                <span className="font-semibold">{c.right}</span>
              </span>
              <span className="flex items-center gap-3 flex-none">
                <Switch checked={c.enabled} onCheckedChange={() => toggle(c)} aria-label={c.enabled ? 'Вимкнути' : 'Увімкнути'} />
                <button type="button" onClick={() => remove(c)} className="h-7 px-2 text-[12px] text-[color:var(--muted-text)] hover:underline">
                  Забути
                </button>
              </span>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
