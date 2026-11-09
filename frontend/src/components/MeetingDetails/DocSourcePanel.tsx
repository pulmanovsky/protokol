'use client';

import { useEffect, useMemo, useState } from 'react';
import { ArrowRight, Play, X } from 'lucide-react';
import { Transcript } from '@/types';
import { mmss } from '@/lib/timeline';
import { LineWord, effectiveRows, loadLineWords } from '@/lib/lineTimes';

interface Props {
  meetingId: string;
  /** The moment the clicked mark points to (seconds) */
  seconds: number;
  /** A point without a source (⟨?⟩): its text */
  unknown?: string;
  transcripts: Transcript[];
  /** The document as text, to find what was left out near this moment */
  markdown: string;
  onListen: (start: number, end: number) => void;
  onOpenInTranscript: (seconds: number) => void;
  onAdd: (quote: string) => void;
  onDismiss: (quote: string) => void;
  onClose: () => void;
}

/** «Не враховано» quotes of the «ВИСТУПИЛИ» block that holds this moment */
function leftOut(markdown: string, seconds: number): string[] {
  const at = mmss(seconds);
  const blocks = markdown.split(/(?=\*\*ВИСТУПИЛИ:\*\*)/);
  const block = blocks.find((b) => b.startsWith('**ВИСТУПИЛИ:**') && b.split('\n')[0].includes(at)) ?? blocks.find((b) => b.startsWith('**ВИСТУПИЛИ:**') && b.includes(at));
  if (!block) return [];
  const line = block.split('\n').find((l) => l.includes('Не враховано'));
  if (!line) return [];
  return [...line.matchAll(/«([^«»]+)»/g)].map((m) => m[1]);
}

/**
 * «Джерело пункту»: what was said at the moment a mark of the document
 * points to — who, when, the words (to listen to and to open in the
 * transcript) — and what that person said that the document left out.
 */
export function DocSourcePanel({ meetingId, seconds, unknown, transcripts, markdown, onListen, onOpenInTranscript, onAdd, onDismiss, onClose }: Props) {
  const lines = useMemo(() => {
    const sorted = [...transcripts].sort((a, b) => (a.audio_start_time ?? 0) - (b.audio_start_time ?? 0));
    let i = sorted.findIndex((t) => (t.audio_start_time ?? 0) > seconds + 0.5) - 1;
    if (i < 0) i = sorted.length - 1;
    i = Math.max(0, i);
    const who = sorted[i]?.speaker;
    const out = [sorted[i]];
    for (let k = i + 1; k < sorted.length && out.length < 4 && sorted[k].speaker === who; k++) out.push(sorted[k]);
    return out.filter(Boolean);
  }, [transcripts, seconds]);
  // Listened to where the words sound (the lines' stored times may be seconds off)
  const [words, setWords] = useState<Map<string, LineWord[]>>(new Map());
  useEffect(() => {
    let alive = true;
    void loadLineWords(meetingId).then((m) => alive && setWords(m));
    return () => {
      alive = false;
    };
  }, [meetingId]);
  const sounding = useMemo(() => {
    const sorted = [...transcripts]
      .sort((a, b) => (a.audio_start_time ?? 0) - (b.audio_start_time ?? 0))
      .map((t, i, all) => ({ id: t.id, start: t.audio_start_time ?? 0, end: t.audio_end_time ?? all[i + 1]?.audio_start_time ?? (t.audio_start_time ?? 0) + 5 }));
    return new Map(effectiveRows(sorted, words).map((r) => [r.id, r]));
  }, [transcripts, words]);
  const first = lines[0];
  const last = lines[lines.length - 1];
  const start = (first && sounding.get(first.id)?.start) ?? first?.audio_start_time ?? seconds;
  const end = (last && sounding.get(last.id)?.end) ?? last?.audio_end_time ?? start + 10;
  // The transcript finds the line by its stored time
  const storedStart = first?.audio_start_time ?? seconds;
  const missed = useMemo(() => leftOut(markdown, seconds), [markdown, seconds]);

  return (
    <aside className="flex w-[340px] flex-none flex-col gap-3 overflow-y-auto border-l border-[var(--hairline,rgba(127,127,127,0.18))] px-4 py-4 custom-scrollbar" aria-label="Джерело пункту">
      <div className="flex items-center justify-between">
        <span className="text-[14px] font-bold text-gray-900">Джерело пункту</span>
        <button type="button" className="row-icon" onClick={onClose} aria-label="Закрити">
          <X className="h-4 w-4" />
        </button>
      </div>
      {unknown && (
        <div className="inspector-card text-[12px] leading-[1.5] text-gray-900">
          <div className="mb-1 text-[13px] font-semibold">Без джерела</div>
          У словах доповідача не знайшлося нічого схожого на цей пункт — можливо, модель його домислила. Послухайте виступ нижче й виправте або видаліть пункт.
          <div className="mt-1.5 text-[color:var(--muted-text)]">{unknown}</div>
        </div>
      )}
      {first ? (
        <>
          <div className="text-[12px] text-[color:var(--muted-text)]">
            <span className="font-semibold text-gray-900">{first.speaker ?? 'Хтось'}</span> · {mmss(start)}–{mmss(end)}
          </div>
          <div className="rounded-xl bg-[var(--selected)] px-3 py-2 text-[13px] leading-[1.55] text-gray-900">
            {lines.map((l) => (
              <p key={l.id} className="mb-1 last:mb-0">
                {l.text}
              </p>
            ))}
          </div>
          <div className="flex flex-wrap gap-1.5">
            <button type="button" className="line-tool" onClick={() => onListen(start, end)}>
              <Play className="h-3 w-3" fill="currentColor" /> Прослухати
            </button>
            <button type="button" className="line-tool" onClick={() => onOpenInTranscript(storedStart)}>
              Відкрити в стенограмі <ArrowRight className="h-3 w-3" />
            </button>
          </div>
          <p className="text-[11px] text-[color:var(--muted-text)]">Числа, дати й імена в документі перевіряються за стенограмою під час створення.</p>
        </>
      ) : (
        <p className="text-[13px] text-[color:var(--muted-text)]">У стенограмі немає рядка на цей час.</p>
      )}
      {missed.length > 0 && (
        <div className="inspector-card">
          <div className="text-[13px] font-semibold text-gray-900">Не потрапило в документ · {missed.length}</div>
          <div className="mt-2 flex flex-col gap-2">
            {missed.map((q) => (
              <div key={q} className="rounded-lg bg-[var(--card-bg)] px-2 py-1.5 text-[12px] leading-[1.5] text-gray-900">
                «{q}»
                <div className="mt-1.5 flex gap-1.5">
                  <button type="button" className="voice-accept !h-6 !text-[11px]" onClick={() => onAdd(q)}>
                    Додати в документ
                  </button>
                  <button type="button" className="line-tool !h-6 !text-[11px]" onClick={() => onDismiss(q)}>
                    Не важливо
                  </button>
                </div>
              </div>
            ))}
          </div>
        </div>
      )}
    </aside>
  );
}
