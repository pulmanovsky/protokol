import { invoke } from '@tauri-apps/api/core';

/** «Підсвічувати кожне слово» changed in Settings (detail: on) */
export const WORD_HIGHLIGHT_EVENT = 'protokol:word-highlight';

/** A word of a line with the moment it sounds in the recording */
export interface LineWord {
  word: string;
  start: number;
  end: number;
  prob: number;
}

/** Words with times per line of a meeting (empty map on failure) */
export async function loadLineWords(meetingId: string): Promise<Map<string, LineWord[]>> {
  try {
    const all = await invoke<{ id: string; words: LineWord[] }[]>('get_line_words', { meetingId });
    return new Map(all.map((l) => [l.id, l.words]));
  } catch {
    return new Map();
  }
}

/** A line's times where it sounds; `stored` are the line's own (as saved) */
export function effectiveRows<R extends { id: string; start: number; end: number }>(rows: R[], words: Map<string, { start: number; end: number }[]>): (R & { stored: { start: number; end: number } })[] {
  const out = rows.map((r) => {
    const w = words.get(r.id);
    const first = w?.[0]?.start;
    const last = w?.[w.length - 1]?.end;
    return {
      ...r,
      stored: { start: r.start, end: r.end },
      start: first !== undefined && Number.isFinite(first) ? Math.min(r.start, first) : r.start,
      end: last !== undefined && Number.isFinite(last) ? Math.max(r.end, last) : r.end,
    };
  });
  // In order, and not over the neighbours
  for (let i = 1; i < out.length; i++) out[i].start = Math.max(out[i].start, out[i - 1].start);
  for (let i = 0; i < out.length; i++) {
    const next = out[i + 1];
    if (next) out[i].end = Math.min(out[i].end, next.start);
    out[i].end = Math.max(out[i].end, out[i].start);
  }
  return out;
}
