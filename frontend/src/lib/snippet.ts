/** What is played for one word: a little before and after, at word edges when they are near */
export const SNIPPET_BEFORE_S = 0.5;
export const SNIPPET_AFTER_S = 0.5;
/** An edge further than this from the wanted place is not used */
const SNAP_S = 0.2;

export interface WordSpan {
  start: number;
  end: number;
}

export function snippetRange(words: WordSpan[] | undefined, w: number, span: WordSpan): WordSpan {
  const wantStart = Math.max(0, span.start - SNIPPET_BEFORE_S);
  const wantEnd = span.end + SNIPPET_AFTER_S;
  let start = wantStart;
  let end = wantEnd;
  if (words && w >= 0 && w < words.length) {
    let bestStart = SNAP_S;
    for (let k = 0; k < w; k++) {
      const d = Math.abs(words[k].start - wantStart);
      if (d <= bestStart) {
        bestStart = d;
        start = words[k].start;
      }
    }
    let bestEnd = SNAP_S;
    for (let k = w + 1; k < words.length; k++) {
      const d = Math.abs(words[k].end - wantEnd);
      if (d <= bestEnd) {
        bestEnd = d;
        end = words[k].end;
      }
    }
  }
  return { start: Math.max(0, start), end };
}
