/** The recording's shape, for the simple timeline (see waveform.rs) */
export interface Waveform {
  duration: number;
  /** Slices per second */
  rate: number;
  /** Heights: 0 silence, 1–255 sound */
  data: number[];
}

export interface View {
  start: number;
  length: number;
}

const BAR = 2;
const GAP = 1;
/** A column this low (of 255) is drawn as silence */
const FLAT = 8;

/** Silent stretches of at least `minS` seconds, for «тиша · M:SS» */
export function silences(w: Waveform, minS = 10): { from: number; to: number }[] {
  const out: { from: number; to: number }[] = [];
  let start = -1;
  for (let i = 0; i <= w.data.length; i++) {
    const quiet = i < w.data.length && w.data[i] === 0;
    if (quiet && start < 0) start = i;
    if (!quiet && start >= 0) {
      if ((i - start) / w.rate >= minS) out.push({ from: start / w.rate, to: i / w.rate });
      start = -1;
    }
  }
  return out;
}

/** The view kept inside the recording, and no shorter than `min` seconds */
export function clampView(v: View, duration: number, min = 10): View {
  const length = Math.max(Math.min(min, duration), Math.min(v.length, duration));
  return { start: Math.max(0, Math.min(v.start, duration - length)), length };
}

/**
 * Paints the visible part once: bars around the middle line, silence as a thin
 * flat line in a lighter tone. Only on a resize, a theme change, a zoom or a
 * pan; the playhead and the played part move over it without painting.
 */
export function drawWave(canvas: HTMLCanvasElement, w: Waveform, view: View, width: number, height: number, bar: string, silent: string) {
  const dpr = window.devicePixelRatio || 1;
  canvas.width = Math.round(width * dpr);
  canvas.height = Math.round(height * dpr);
  canvas.style.width = `${width}px`;
  canvas.style.height = `${height}px`;
  const c = canvas.getContext('2d');
  if (!c || !w.data.length) return;
  c.setTransform(dpr, 0, 0, dpr, 0, 0);
  c.clearRect(0, 0, width, height);
  const mid = height / 2;
  const columns = Math.max(1, Math.floor(width / (BAR + GAP)));
  const perColumn = (view.length * w.rate) / columns;
  const first = view.start * w.rate;
  for (let k = 0; k < columns; k++) {
    const from = Math.floor(first + k * perColumn);
    const to = Math.max(from + 1, Math.floor(first + (k + 1) * perColumn));
    let sum = 0;
    let n = 0;
    for (let i = from; i < to && i < w.data.length; i++) {
      sum += w.data[i] * w.data[i];
      n++;
    }
    const level = n ? Math.sqrt(sum / n) : 0;
    const x = k * (BAR + GAP);
    if (level < FLAT) {
      c.fillStyle = silent;
      c.fillRect(x, mid - 0.75, BAR + GAP, 1.5);
    } else {
      const h = Math.max(3, (level / 255) * height);
      c.fillStyle = bar;
      c.beginPath();
      c.roundRect(x, mid - h / 2, BAR, h, 1);
      c.fill();
    }
  }
}
