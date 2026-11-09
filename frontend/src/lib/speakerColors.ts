/** Colours of speakers: the same person keeps a colour in a meeting (by the
 * order they first speak); readable in the light and the dark theme */
export const SPEAKER_COLORS = ['#4f8ff7', '#2fa66a', '#e0912b', '#a066e8', '#e2683a', '#1fa8bd', '#d9506a', '#7a9a2e', '#8a7f72'];

/** Unnamed voices («Спікер 3», a suggestion with «(?)») */
export function isUnnamed(speaker: string | undefined): boolean {
  return !speaker || /^Спікер \d+$/.test(speaker.trim()) || speaker.trim().endsWith('(?)') || speaker.trim() === '…';
}

/** A person's colour by their name, so it stays when lines change hands;
 * two people sharing a colour get the next free one (in speaking order) */
export function speakerColorMap(speakers: (string | undefined)[]): Map<string, string> {
  const out = new Map<string, string>();
  const used = new Set<number>();
  for (const s of speakers) {
    if (!s || out.has(s)) continue;
    let h = 0;
    for (const c of s) h = (h * 31 + c.charCodeAt(0)) >>> 0;
    let k = h % SPEAKER_COLORS.length;
    for (let n = 0; n < SPEAKER_COLORS.length && used.has(k); n++) k = (k + 1) % SPEAKER_COLORS.length;
    used.add(k);
    if (used.size >= SPEAKER_COLORS.length) used.clear();
    out.set(s, SPEAKER_COLORS[k]);
  }
  return out;
}
