/**
 * Which transcript line a moment of the recording belongs to: the last line
 * that starts at or before it (lines need not be sorted). -1 when the moment
 * is before every line.
 */
export function segmentIndexAt(segments: { timestamp: number }[], seconds: number): number {
  let best = -1;
  let bestStart = -Infinity;
  // Times in the document are whole seconds («30:04»): a line that starts at
  // 30:04.83 is shown as 30:04 and is the one meant
  const second = Math.floor(seconds + 1e-6);
  segments.forEach((s, i) => {
    if (Math.floor(s.timestamp) <= second && s.timestamp > bestStart) {
      best = i;
      bestStart = s.timestamp;
    }
  });
  return best;
}

/** Event: show this moment in the transcript (scroll and highlight its line) */
export const JUMP_TO_TIME_EVENT = 'protokol:jump-to-time';
/** Event: a time mark was clicked in the transcript; offer to play from it */
export const TIME_CLICKED_EVENT = 'protokol:time-clicked';
/** Event: play the recording from this moment now */
export const PLAY_FROM_EVENT = 'protokol:play-from';
/** Event: play one stretch of the recording ({ start, end, loop }) — a line */
export const PLAY_RANGE_EVENT = 'protokol:play-range';
/** «Позначити все перевіреним», asked from outside the transcript */
export const MARK_ALL_REVIEWED_EVENT = 'protokol:mark-all-reviewed';
export function playRange(start: number, end: number, loop = false) {
  window.dispatchEvent(new CustomEvent(PLAY_RANGE_EVENT, { detail: { start, end, loop } }));
}
/** Plays from a word: right at its start (no lead-in) */
export function playFromWord(seconds: number) {
  window.dispatchEvent(new CustomEvent(PLAY_FROM_EVENT, { detail: { seconds, exact: true } }));
}
/** Event: open the «Спікери» window */
export const OPEN_SPEAKERS_EVENT = 'protokol:open-speakers';

export function mmss(seconds: number): string {
  const s = Math.max(0, Math.floor(seconds));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = s % 60;
  const pad = (n: number) => String(n).padStart(2, '0');
  return h > 0 ? `${h}:${pad(m)}:${pad(sec)}` : `${pad(m)}:${pad(sec)}`;
}

/** A line just changed (speaker, text): the transcript shows it briefly highlighted */
export const LINE_CHANGED_EVENT = 'protokol:line-changed';
export function markLineChanged(id: string) {
  window.dispatchEvent(new CustomEvent(LINE_CHANGED_EVENT, { detail: { id } }));
}
