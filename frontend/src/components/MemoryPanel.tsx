'use client';

import { useCallback, useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { toast } from '@/lib/notice';
import { useSidebar } from '@/components/Sidebar/SidebarProvider';
import { plural } from '@/lib/plural';

interface Rule {
  from: string;
  to: string;
  learnedAt?: string | null;
  meeting?: string | null;
  applied: number;
}
interface Correction {
  wrong: string;
  right: string;
  enabled: boolean;
}
interface LearnedMeeting {
  examples: number;
}
interface Summary {
  words: number;
  voices: number;
  documents: number;
  applied: [string, number][];
}

const day = (iso?: string | null) => (iso ? new Date(iso).toLocaleDateString('uk-UA', { day: '2-digit', month: '2-digit' }) : '');

/**
 * «Пам'ять»: what the app keeps from your fixes. Words (rules: heard → written),
 * voices (people known by voice), the document (examples from approved
 * documents), and the latest rules with where they came from and how often
 * they were applied since.
 */
export function MemoryPanel({ version, onChanged }: { version: number; onChanged: () => void }) {
  const { meetings } = useSidebar();
  const [corrections, setCorrections] = useState<Correction[]>([]);
  const [rules, setRules] = useState<Rule[]>([]);
  const [voices, setVoices] = useState(0);
  const [examples, setExamples] = useState(0);
  const [today, setToday] = useState<Summary | null>(null);
  const [all, setAll] = useState<Summary | null>(null);

  const load = useCallback(() => {
    invoke<Correction[]>('api_corrections').then(setCorrections).catch(() => setCorrections([]));
    invoke<Rule[]>('api_learned_rules').then(setRules).catch(() => setRules([]));
    invoke<number>('api_voice_profile_count').then(setVoices).catch(() => setVoices(0));
    invoke<LearnedMeeting[]>('api_learned_meetings').then((m) => setExamples(m.reduce((n, x) => n + x.examples, 0))).catch(() => setExamples(0));
    const midnight = new Date();
    midnight.setHours(0, 0, 0, 0);
    invoke<Summary>('api_learned_summary', { since: midnight.toISOString() }).then(setToday).catch(() => setToday(null));
    invoke<Summary>('api_learned_summary', { since: null }).then(setAll).catch(() => setAll(null));
  }, []);
  useEffect(load, [load, version]);

  const forget = async (wrong: string) => {
    try {
      await invoke('api_delete_correction', { wrong });
      toast.success(`Правило «${wrong}» прибрано з пам'яті`);
      load();
      onChanged();
    } catch (e) {
      toast.error(`${e}`);
    }
  };

  // Every rule: those with a history first (newest), then older ones from the corrections file
  const known = new Set(rules.map((r) => `${r.from.toLowerCase()}→${r.to}`));
  const rows: Rule[] = [
    ...rules.filter((r) => corrections.some((c) => c.wrong.toLowerCase() === r.from.toLowerCase())),
    ...corrections.filter((c) => c.enabled && !known.has(`${c.wrong.toLowerCase()}→${c.right}`)).map((c) => ({ from: c.wrong, to: c.right, applied: 0 })),
  ];
  const ruleCount = corrections.filter((c) => c.enabled).length;
  const appliedAll = (all?.applied ?? []).reduce((n, [, k]) => n + k, 0);
  const titleOf = (id?: string | null) => meetings.find((m) => m.id === id)?.title;

  return (
    <div className="flex flex-col gap-6">
      <div className="memory-tiles">
        <section className="card memory-tile">
          <span className="memory-tile-label">Слова</span>
          <b>{ruleCount}</b>
          <span>{plural(ruleCount, 'правило', 'правила', 'правил')}{today?.words ? ` · +${today.words} сьогодні` : ''}</span>
          <span>{appliedAll > 0 ? `Застосовано ${appliedAll} ${plural(appliedAll, 'раз', 'рази', 'разів')}` : 'Ще не застосовувались'}</span>
        </section>
        <section className="card memory-tile">
          <span className="memory-tile-label">Голоси</span>
          <b>{voices}</b>
          <span>{plural(voices, 'людина', 'людини', 'людей')}{today?.voices ? ` · +${today.voices} сьогодні` : ''}</span>
          <span>Упізнаються в наступних записах</span>
        </section>
        <section className="card memory-tile">
          <span className="memory-tile-label">Документ</span>
          {examples > 0 ? (
            <>
              <b>{examples}</b>
              <span>{plural(examples, 'приклад', 'приклади', 'прикладів')} із затверджених документів</span>
            </>
          ) : (
            <span className="memory-tile-empty">З&apos;являться після «Затвердити»</span>
          )}
        </section>
      </div>

      <section className="card p-0 overflow-hidden">
        <h2 className="px-5 pt-4 pb-2 text-[15px] font-semibold">Останні правила</h2>
        {rows.length === 0 ? (
          <p className="px-5 pb-5 text-sm text-[color:var(--muted-text)]">
            Поки порожньо. Виправте слово в стенограмі з позначкою «Запам&apos;ятати для наступних записів», і правило з&apos;явиться тут.
          </p>
        ) : (
          <div className="overflow-x-auto">
            <table className="memory-table">
              <thead>
                <tr>
                  <th>Почуто → записується</th>
                  <th>Звідки</th>
                  <th>Застосовано</th>
                  <th aria-label="Дії" />
                </tr>
              </thead>
              <tbody>
                {rows.slice(0, 200).map((r) => (
                  <tr key={`${r.from}→${r.to}`}>
                    <td>
                      <span className="memory-from">{r.from}</span> → <b>{r.to}</b>
                    </td>
                    <td className="memory-muted">{r.learnedAt ? [titleOf(r.meeting), day(r.learnedAt)].filter(Boolean).join(' · ') : '—'}</td>
                    <td className="memory-muted tabular-nums">{r.applied > 0 ? `${r.applied} ${plural(r.applied, 'раз', 'рази', 'разів')}` : '—'}</td>
                    <td>
                      <button type="button" className="memory-forget" onClick={() => void forget(r.from)}>
                        Забути
                      </button>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </section>
    </div>
  );
}
