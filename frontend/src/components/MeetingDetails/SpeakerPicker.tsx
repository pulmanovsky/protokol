'use client';

import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Check, Search, UserPlus } from 'lucide-react';
import { toast } from '@/lib/notice';

/** Which lines get the new speaker */
export type SpeakerScope = 'line' | 'following' | 'all';

const SCOPES: { value: SpeakerScope; label: string }[] = [
  { value: 'line', label: 'Лише цей рядок' },
  { value: 'following', label: 'Цей і наступні до зміни голосу' },
  { value: 'all', label: 'Усі рядки цього голосу' },
];

interface Props {
  meetingId: string;
  /** The line's speaker now */
  current?: string;
  /** Where it was opened (the speaker name or the 👤 button) */
  anchor: DOMRect;
  onPick: (name: string, scope: SpeakerScope) => void;
  onClose: () => void;
  /** For chosen words: no choice of which lines */
  wordsOnly?: boolean;
}

/**
 * «Хто це говорив?» for a line: the meeting's speakers, then people known
 * from earlier meetings, a search, and a new name. The scope says whether
 * the change is for this line, the lines that follow in the same voice, or
 * every line of the voice (the same as renaming it in «Спікери»).
 */
export function SpeakerPicker({ meetingId, current, anchor, onPick, onClose, wordsOnly = false }: Props) {
  const [here, setHere] = useState<string[]>([]);
  const [known, setKnown] = useState<string[]>([]);
  const [query, setQuery] = useState('');
  // A name the voice only resembles («Ім'я?»): one step to confirm it
  const guess = current && /(\(\?\)|\?)\s*$/.test(current) ? current.replace(/\s*(\(\?\)|\?)\s*$/, '').trim() : null;
  const [scope, setScope] = useState<SpeakerScope>(guess ? 'all' : 'line');
  const box = useRef<HTMLDivElement>(null);
  const input = useRef<HTMLInputElement>(null);
  // Focus without scrolling the transcript under it
  useEffect(() => input.current?.focus({ preventScroll: true }), []);
  const [pos, setPos] = useState<{ top: number; left: number; maxHeight?: number }>({ top: anchor.bottom + 6, left: anchor.left });

  useEffect(() => {
    invoke<{ here: string[]; known: string[] }>('get_speaker_choices', { meetingId })
      .then((c) => {
        setHere(c.here);
        setKnown(c.known);
      })
      .catch(() => {});
  }, [meetingId]);

  // Kept inside the window
  useLayoutEffect(() => {
    const el = box.current;
    if (!el) return;
    const width = el.getBoundingClientRect().width;
    // The natural height, not one already shortened
    const height = el.scrollHeight;
    const left = Math.max(8, Math.min(anchor.left, window.innerWidth - width - 8));
    // Below the name when it fits, else above; when neither fits (a low
    // window) the roomier side, the list shorter, never over the title bar
    const TITLE = 56;
    const below = anchor.bottom + 6;
    const roomBelow = window.innerHeight - 8 - below;
    const roomAbove = anchor.top - 6 - TITLE;
    if (height <= roomBelow) setPos({ top: below, left });
    else if (height <= roomAbove) setPos({ top: anchor.top - height - 6, left });
    else if (roomBelow >= roomAbove) setPos({ top: below, left, maxHeight: roomBelow });
    else setPos({ top: TITLE, left, maxHeight: roomAbove });
  }, [anchor, here.length, known.length]);

  useEffect(() => {
    const away = (e: MouseEvent) => {
      if (box.current && !box.current.contains(e.target as Node)) onClose();
    };
    const key = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
    };
    document.addEventListener('mousedown', away);
    document.addEventListener('keydown', key);
    return () => {
      document.removeEventListener('mousedown', away);
      document.removeEventListener('keydown', key);
    };
  }, [onClose]);

  const q = query.trim().toLowerCase();
  const match = (n: string) => !q || n.toLowerCase().includes(q);
  const shownHere = useMemo(() => here.filter((n) => n !== current && n !== guess && match(n)), [here, current, guess, q]);
  const shownKnown = useMemo(() => known.filter((n) => n !== current && n !== guess && match(n)).slice(0, 30), [known, current, guess, q]);
  const typed = query.trim().replace(/\s+/g, ' ');
  const isNew = typed.length > 1 && ![...here, ...known].some((n) => n.toLowerCase() === typed.toLowerCase());
  const first = shownHere[0] ?? shownKnown[0];

  const row = (name: string) => (
    <button key={name} type="button" onClick={() => onPick(name, scope)} className="nav-row h-8 w-full truncate px-2.5 text-left text-sm">
      {name}
    </button>
  );

  // A name from earlier recordings that is not wanted: no longer offered
  // (back in «Спікери → Усі збережені» or by typing it again)
  const hideName = async (name: string) => {
    try {
      await invoke('set_saved_speaker', { name, recognize: null, suggest: false, role: null });
      setKnown((k) => k.filter((n) => n !== name));
      toast.success(`«${name}» більше не пропонується`, {
        action: {
          label: 'Скасувати',
          onClick: async () => {
            await invoke('set_saved_speaker', { name, recognize: null, suggest: true, role: null }).catch(() => {});
            setKnown((k) => (k.includes(name) ? k : [...k, name].sort((a, b) => a.localeCompare(b, 'uk'))));
          },
        },
      });
    } catch (e) {
      toast.error(`Не вдалося зберегти: ${e}`);
    }
  };
  const knownRow = (name: string) => (
    <div key={name} className="group relative">
      {row(name)}
      <button
        type="button"
        onClick={() => hideName(name)}
        title="Більше не пропонувати це ім'я"
        className="absolute right-1 top-1/2 hidden -translate-y-1/2 rounded-md bg-[var(--card-bg)] px-1.5 py-0.5 text-[11px] text-[color:var(--muted-text)] hover:text-gray-900 group-hover:block"
      >
        Не пропонувати
      </button>
    </div>
  );

  return (
    <div
      ref={box}
      role="dialog"
      aria-label="Змінити спікера"
      className="speaker-picker fixed z-50 flex w-[280px] flex-col rounded-xl border border-[var(--card-border)] bg-[var(--card-bg)] shadow-xl"
      style={{ top: pos.top, left: pos.left, maxHeight: pos.maxHeight, overflow: pos.maxHeight ? 'hidden' : undefined }}
    >
      <div className="px-3 pt-2.5 pb-1.5 text-[11px] font-medium text-[color:var(--muted-text)]">
        {wordsOnly ? 'Хто сказав виділені слова' : 'Хто говорив'}{current && !wordsOnly ? <> замість «<span className="text-gray-900">{current}</span>»</> : ''}?
      </div>
      {guess && !wordsOnly && (
        <button type="button" onClick={() => onPick(guess, scope)} className="speaker-confirm mx-2 mb-1.5 flex h-8 items-center gap-2 rounded-lg px-2.5 text-left text-sm font-medium">
          <Check className="h-3.5 w-3.5 flex-none" />
          <span className="truncate">Так, це {guess}</span>
          <kbd className="ml-auto text-[11px] font-normal opacity-80">↩</kbd>
        </button>
      )}
      <div className="mx-2 mb-1.5 flex h-8 items-center gap-1.5 rounded-lg bg-[var(--selected)] px-2">
        <Search className="h-3.5 w-3.5 flex-none text-[color:var(--muted-text)]" />
        <input
          ref={input}
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') {
              e.preventDefault();
              if (guess && !typed) onPick(guess, scope);
              else if (isNew && !first) onPick(typed, scope);
              else if (first) onPick(first, scope);
            }
          }}
          placeholder={guess ? "Знайти або вписати ім'я" : "Пошук або нове ім'я"}
          aria-label="Пошук людини"
          className="min-w-0 flex-1 bg-transparent text-sm text-gray-900 outline-none placeholder:text-[color:var(--muted-text)]"
        />
      </div>
      <div className="max-h-[220px] min-h-0 flex-1 overflow-y-auto px-1">
        {shownHere.length > 0 && (
          <>
            <div className="px-2.5 pt-1 pb-0.5 text-[10px] font-semibold uppercase tracking-wide text-[color:var(--muted-text)]">У цьому записі</div>
            {shownHere.map(row)}
          </>
        )}
        {shownKnown.length > 0 && (
          <>
            <div className="px-2.5 pt-1.5 pb-0.5 text-[10px] font-semibold uppercase tracking-wide text-[color:var(--muted-text)]">Відомі люди</div>
            {shownKnown.map(knownRow)}
          </>
        )}
        {isNew ? (
          <button type="button" onClick={() => onPick(typed, scope)} className="nav-row flex h-8 w-full items-center gap-2 px-2.5 text-left text-sm text-[var(--blue)]">
            <UserPlus className="h-3.5 w-3.5 flex-none" />
            <span className="truncate">Нова людина: «{typed}»</span>
          </button>
        ) : (
          shownHere.length + shownKnown.length === 0 && (
            <div className="flex h-8 items-center gap-2 px-2.5 text-sm text-[color:var(--muted-text)]">
              <UserPlus className="h-3.5 w-3.5 flex-none" />
              Нова людина — введіть ім'я
            </div>
          )
        )}
      </div>
      {!wordsOnly && (
      <div className="mt-1 border-t border-[var(--hairline,rgba(0,0,0,0.08))] p-1" role="radiogroup" aria-label="Для яких рядків">
        {SCOPES.filter((s) => s.value !== 'all' || current).map((s) => (
          <button
            key={s.value}
            type="button"
            role="radio"
            aria-checked={scope === s.value}
            onClick={() => setScope(s.value)}
            className="nav-row flex h-7 w-full items-center gap-2 px-2 text-left text-[12px]"
          >
            <span className="flex h-3.5 w-3.5 flex-none items-center justify-center text-[var(--blue)]">{scope === s.value && <Check className="h-3.5 w-3.5" />}</span>
            <span className={scope === s.value ? 'text-gray-900' : 'text-[color:var(--muted-text)]'}>{s.label}</span>
          </button>
        ))}
      </div>
      )}
    </div>
  );
}
