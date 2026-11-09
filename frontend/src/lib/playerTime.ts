/**
 * A recording joined from chunks has two clocks: the sound's (what word times
 * and the waveform follow) and the file's (what the audio element counts).
 * Points are [sound, file], in order; none means they are the same.
 */
export type TimePoints = [number, number][];

function along(points: TimePoints, t: number, from: 0 | 1, to: 0 | 1): number {
  if (points.length === 0) return t;
  let lo = 0;
  let hi = points.length;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (points[mid][from] <= t) lo = mid + 1;
    else hi = mid;
  }
  if (lo === 0) return t + points[0][to] - points[0][from];
  if (lo === points.length) return t + points[lo - 1][to] - points[lo - 1][from];
  const a = points[lo - 1];
  const b = points[lo];
  const span = b[from] - a[from];
  return span > 0 ? a[to] + ((t - a[from]) / span) * (b[to] - a[to]) : a[to];
}

/** The place in the file for a moment of the sound */
export const toPlayer = (points: TimePoints, sound: number): number => along(points, sound, 0, 1);
/** The moment of the sound for a place in the file */
export const toSound = (points: TimePoints, player: number): number => along(points, player, 1, 0);
