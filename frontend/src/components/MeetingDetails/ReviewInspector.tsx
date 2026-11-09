'use client';

import { useEffect, useMemo, useState } from 'react';
import { ChevronLeft, ChevronRight, Play, Users } from 'lucide-react';
import { mmss } from '@/lib/timeline';

/** One place to check: a voice change inside a line, an unnamed voice, an unsure word */
export interface QueueItem {
  key: string;
  kind: 'voice' | 'name' | 'word';
  segmentId: string;
  seconds: number;
  title: string;
  description: string;
  /** The words of the line around the place */
  text: string;
  primary: string;
}

export interface Person {
  name: string;
  color: string;
  lines: number;
  seconds: number;
  unnamed: boolean;
}

const KINDS: { value: 'all' | QueueItem['kind']; label: string }[] = [
  { value: 'all', label: 'Усе' },
  { value: 'voice', label: 'Голос' },
  { value: 'name', label: 'Без імені' },
  { value: 'word', label: 'Слова' },
];

interface Props {
  items: QueueItem[];
  people: Person[];
  /** Shows the place: the line in focus, played from there */
  onGo: (item: QueueItem) => void;
  onPrimary: (item: QueueItem) => void;
  /** «Ні» for a voice change, «Вірно» for a word */
  onDismiss: (item: QueueItem) => void;
  onPlayPerson: (name: string) => void;
  onNamePerson: (name: string, anchor: DOMRect) => void;
  onOpenSpeakers: () => void;
}

/**
 * The right column of «Стенограма»: «Перевірте» goes through the places
 * worth a listen one by one (with «Далі / Назад»); «Люди» lists the voices
 * of the meeting. The first nine people get keys 1–9 for the line in focus.
 */
export function ReviewInspector({ items, people, onGo, onPrimary, onDismiss, onPlayPerson, onNamePerson, onOpenSpeakers }: Props) {
  const [tab, setTab] = useState<'queue' | 'people'>('queue');
  const [kind, setKind] = useState<'all' | QueueItem['kind']>('all');
  const shown = useMemo(() => (kind === 'all' ? items : items.filter((i) => i.kind === kind)), [items, kind]);
  const [index, setIndex] = useState(0);
  useEffect(() => {
    if (index >= shown.length) setIndex(Math.max(0, shown.length - 1));
  }, [shown.length, index]);
  const current = shown[index];
  const go = (k: number) => {
    const next = shown[(k + shown.length) % shown.length];
    if (!next) return;
    setIndex((k + shown.length) % shown.length);
    onGo(next);
  };
  const count = (k: 'all' | QueueItem['kind']) => (k === 'all' ? items.length : items.filter((i) => i.kind === k).length);

  return (
    <aside className="flex w-[340px] flex-none flex-col border-l border-[var(--hairline,rgba(127,127,127,0.18))] min-h-0" aria-label="Перевірка">
      <div className="flex gap-1 px-3 pt-3">
        <button type="button" className="filter-chip !h-8 flex-1 !text-[13px]" data-on={tab === 'queue'} onClick={() => setTab('queue')}>
          Перевірте · {items.length}
        </button>
        <button type="button" className="filter-chip !h-8 flex-1 !text-[13px]" data-on={tab === 'people'} onClick={() => setTab('people')}>
          Люди · {people.length}
        </button>
      </div>

      {tab === 'queue' ? (
        <div className="flex min-h-0 flex-1 flex-col gap-3 overflow-y-auto px-3 py-3 custom-scrollbar">
          {current ? (
            <div className="inspector-card">
              <div className="flex items-center justify-between text-[11px] font-semibold text-[#c27a00]">
                <span>
                  {index + 1} з {shown.length}
                </span>
                <button type="button" className="tabular-nums hover:underline" onClick={() => onGo(current)}>
                  {mmss(current.seconds)}
                </button>
              </div>
              <div className="mt-1 text-[14px] font-semibold text-gray-900">{current.title}</div>
              <div className="mt-0.5 text-[12px] text-[color:var(--muted-text)]">{current.description}</div>
              {current.text && <div className="mt-2 rounded-lg bg-[var(--card-bg)] px-2 py-1.5 text-[13px] leading-[1.5] text-gray-900">{current.text}</div>}
              <div className="mt-2.5 flex flex-wrap items-center gap-1.5">
                <button type="button" className="voice-accept" onClick={() => onPrimary(current)}>
                  {current.primary}
                </button>
                {current.kind !== 'name' && (
                  <button type="button" className="line-tool" onClick={() => onDismiss(current)}>
                    {current.kind === 'voice' ? 'Ні, одна людина' : 'Вірно'}
                  </button>
                )}
                <button type="button" className="line-tool" onClick={() => go(index + 1)}>
                  Пропустити
                </button>
              </div>
              <div className="mt-2 flex items-center justify-between">
                <button type="button" className="line-tool" onClick={() => go(index - 1)}>
                  <ChevronLeft className="h-3.5 w-3.5" /> Назад
                </button>
                <button type="button" className="line-tool" onClick={() => go(index + 1)}>
                  Далі <ChevronRight className="h-3.5 w-3.5" />
                </button>
              </div>
            </div>
          ) : (
            <div className="rounded-xl bg-[var(--selected)] px-3 py-4 text-center text-[13px] text-[color:var(--muted-text)]">
              {items.length === 0 ? 'Сумнівних місць не знайдено. Прослухайте й позначте рядки перевіреними (V).' : 'У цьому фільтрі нічого немає.'}
            </div>
          )}
          <div className="flex flex-wrap gap-1">
            {KINDS.map((k) => (
              <button key={k.value} type="button" className="filter-chip" data-on={kind === k.value} onClick={() => setKind(k.value)}>
                {k.label} · {count(k.value)}
              </button>
            ))}
          </div>
          <div className="flex flex-col">
            {shown.map((item, k) => (
              <button
                key={item.key}
                type="button"
                onClick={() => {
                  setIndex(k);
                  onGo(item);
                }}
                className={`flex gap-2 rounded-lg px-2 py-1.5 text-left text-[12px] hover:bg-[var(--selected)] ${k === index ? 'bg-[var(--selected)]' : ''}`}
              >
                <span className="w-10 flex-none tabular-nums text-[color:var(--muted-text)]">{mmss(item.seconds)}</span>
                <span className="min-w-0 flex-1 truncate text-gray-900">
                  <span className="font-semibold">{item.title}</span> · {item.text}
                </span>
              </button>
            ))}
          </div>
        </div>
      ) : (
        <div className="flex min-h-0 flex-1 flex-col overflow-y-auto px-3 py-3 custom-scrollbar">
          {people.map((p) => (
            <div key={p.name} className="flex items-center gap-2 rounded-lg px-2 py-1.5 hover:bg-[var(--selected)]">
              <span className="h-2.5 w-2.5 flex-none rounded-full" style={{ background: p.color }} />
              <span className="min-w-0 flex-1">
                <span className="block truncate text-[13px] font-semibold text-gray-900">{p.name}</span>
                <span className="block text-[11px] text-[color:var(--muted-text)]">
                  {p.lines} рядк. · {Math.max(1, Math.round(p.seconds / 60))} хв
                </span>
              </span>
              {p.unnamed && (
                <button type="button" className="name-pill" onClick={(e) => onNamePerson(p.name, e.currentTarget.getBoundingClientRect())}>
                  Назвати…
                </button>
              )}
              <button type="button" className="row-icon" title="Послухати" onClick={() => onPlayPerson(p.name)}>
                <Play className="h-3.5 w-3.5" fill="currentColor" />
              </button>
            </div>
          ))}
          <button type="button" className="line-tool mt-2 self-start" onClick={onOpenSpeakers}>
            <Users className="h-3.5 w-3.5" /> Об'єднати, розділити, ролі…
          </button>
        </div>
      )}

      <div className="border-t border-[var(--hairline,rgba(127,127,127,0.18))] px-3 py-2.5">
        <div className="mb-1.5 text-[11px] font-semibold text-[color:var(--muted-text)]">Швидкий вибір людини для рядка у фокусі</div>
        <div className="grid grid-cols-2 gap-x-2 gap-y-1">
          {people.slice(0, 9).map((p, k) => (
            <div key={p.name} className="flex min-w-0 items-center gap-1.5 text-[12px] text-gray-900">
              <kbd>{k + 1}</kbd>
              <span className="h-2 w-2 flex-none rounded-full" style={{ background: p.color }} />
              <span className="truncate">{p.name}</span>
            </div>
          ))}
        </div>
      </div>
    </aside>
  );
}
