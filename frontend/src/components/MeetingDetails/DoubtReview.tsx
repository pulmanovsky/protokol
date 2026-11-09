'use client';

import { useEffect, useMemo, useRef, useState } from 'react';
import { Play, X } from 'lucide-react';
import { PLAY_FROM_EVENT, mmss } from '@/lib/timeline';

export interface Doubt {
  segmentId: string;
  wordIndex: number;
  word: string;
  seconds: number;
  probability: number;
  /** Replaced on its own by a dictionary term: what was heard */
  heard?: string | null;
}

interface Props {
  doubts: Doubt[];
  /** Line text without the speaker, by line id */
  lineText: (segmentId: string) => string | undefined;
  /** The word is right as it is */
  onAccept: (d: Doubt) => Promise<void>;
  /** The word should be `word` (the line is saved through the normal edit, so the app learns) */
  onCorrect: (d: Doubt, word: string) => Promise<void>;
  onClose: () => void;
}

/**
 * «Перевірте»: the words Whisper was not sure of, one by one, with the
 * sentence around them. ↑/↓ moves, Enter keeps the word (or saves the
 * correction typed in the field), ▶ plays from a second before it.
 */
export function DoubtReview({ doubts, lineText, onAccept, onCorrect, onClose }: Props) {
  const [index, setIndex] = useState(0);
  const [draft, setDraft] = useState('');
  const [busy, setBusy] = useState(false);
  const inputRef = useRef<HTMLInputElement | null>(null);
  const current = doubts[Math.min(index, doubts.length - 1)];

  useEffect(() => {
    if (index >= doubts.length && doubts.length > 0) setIndex(doubts.length - 1);
  }, [doubts.length, index]);
  useEffect(() => {
    setDraft(current?.word ?? '');
    inputRef.current?.focus();
    inputRef.current?.select();
  }, [current?.segmentId, current?.wordIndex, current?.word]);

  const context = useMemo(() => {
    if (!current) return null;
    const words = (lineText(current.segmentId) ?? '').split(/\s+/).filter(Boolean);
    const from = Math.max(0, current.wordIndex - 8);
    const to = Math.min(words.length, current.wordIndex + 9);
    return { before: words.slice(from, current.wordIndex).join(' '), word: words[current.wordIndex] ?? current.word, after: words.slice(current.wordIndex + 1, to).join(' '), cutStart: from > 0, cutEnd: to < words.length };
  }, [current, lineText]);

  const play = () => current && window.dispatchEvent(new CustomEvent(PLAY_FROM_EVENT, { detail: { seconds: Math.max(0, current.seconds - 1) } }));

  const confirm = async () => {
    if (!current || busy) return;
    setBusy(true);
    try {
      const value = draft.trim();
      if (value && value !== current.word) await onCorrect(current, value);
      else await onAccept(current);
    } finally {
      setBusy(false);
    }
  };

  const onKey = (e: React.KeyboardEvent) => {
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      setIndex((i) => Math.min(doubts.length - 1, i + 1));
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      setIndex((i) => Math.max(0, i - 1));
    } else if (e.key === 'Enter') {
      e.preventDefault();
      void confirm();
    } else if (e.key === 'Escape') {
      e.preventDefault();
      onClose();
    } else if (e.key === ' ' && e.altKey) {
      e.preventDefault();
      play();
    }
  };

  return (
    <div className="absolute inset-0 z-20 flex flex-col bg-[var(--card-bg)]" onKeyDown={onKey}>
      <div className="flex items-center justify-between px-5 pt-5 pb-3">
        <div>
          <h2 className="text-[15px] font-bold text-gray-900">Перевірте</h2>
          <p className="text-xs text-[color:var(--muted-text)]">
            {doubts.length === 0 ? 'Усе перевірено' : `${Math.min(index + 1, doubts.length)} з ${doubts.length} · слова, у яких розпізнавання не певне`}
          </p>
        </div>
        <button type="button" aria-label="Закрити" onClick={onClose} className="tool-btn h-8 w-8">
          <X className="h-4 w-4" />
        </button>
      </div>

      {current && context ? (
        <div className="flex flex-col gap-3 px-5">
          <div className="rounded-xl bg-[var(--selected)] p-4 text-[15px] leading-[1.6] text-gray-900">
            {context.cutStart && '… '}
            {context.before}{' '}
            <mark className="rounded bg-[var(--blue-tint)] px-0.5 text-gray-900 underline decoration-dotted decoration-[var(--blue)] underline-offset-4">{context.word}</mark>{' '}
            {context.after}
            {context.cutEnd && ' …'}
          </div>
          {current.heard && (
            <p className="text-xs text-[color:var(--muted-text)]">
              Розпізнано як «{current.heard}», виправлено за словником на «{current.word}».
            </p>
          )}
          <div className="flex items-center gap-2">
            <button type="button" onClick={play} className="settings-button" title="Слухати з секунди раніше (⌥ пробіл)">
              <Play className="h-3.5 w-3.5" /> {mmss(Math.max(0, current.seconds - 1))}
            </button>
            <input
              ref={inputRef}
              type="text"
              aria-label="Слово"
              value={draft}
              onChange={(e) => setDraft(e.target.value)}
              className="h-8 flex-1 rounded-lg border border-[var(--card-border)] bg-[var(--card-bg)] px-2.5 text-sm text-gray-900 outline-none focus:border-[var(--blue)]"
            />
            <button type="button" disabled={busy} onClick={() => void confirm()} className="settings-button !bg-[var(--blue)] !text-white">
              {draft.trim() && draft.trim() !== current.word ? 'Виправити' : 'Правильно'}
            </button>
          </div>
          <p className="text-xs text-[color:var(--muted-text)]">Enter — {draft.trim() !== current.word ? 'виправити' : 'залишити як є'}, ↑/↓ — інше слово, Esc — закрити.</p>
        </div>
      ) : (
        <p className="px-5 text-sm text-[color:var(--muted-text)]">Сумнівних слів немає.</p>
      )}

      <ul className="mt-3 flex-1 overflow-y-auto px-3 pb-4 custom-scrollbar" aria-label="Сумнівні слова">
        {doubts.map((d, i) => (
          <li key={`${d.segmentId}:${d.wordIndex}`}>
            <button
              type="button"
              onClick={() => setIndex(i)}
              data-active={i === index}
              className="nav-row h-8 w-full justify-between px-2.5 text-sm"
            >
              <span className="truncate">
                {d.word}
                {d.heard && <span className="text-[color:var(--muted-text)]"> ← {d.heard}</span>}
              </span>
              <span className="text-xs tabular-nums text-[color:var(--muted-text)]">{mmss(d.seconds)}</span>
            </button>
          </li>
        ))}
      </ul>
    </div>
  );
}
