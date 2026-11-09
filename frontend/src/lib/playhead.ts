/** The player's clock, read by what follows the sound (the lit words, the
 * timeline's playhead) without re-rendering the page */
let source: (() => number | null) | null = null;

export function setPlayheadSource(fn: (() => number | null) | null) {
  source = fn;
}

export function readPlayhead(): number | null {
  return source ? source() : null;
}

/*
 * One light clock for everything that moves with the sound: about 15 times a
 * second (enough for words and a playhead; 60 would cost the Mac four times
 * as much), only while playing and while the window is shown. Each follower
 * does its own work only when what it shows has changed.
 */
type Follower = (t: number) => void;
const followers = new Set<Follower>();
const STEP_MS = 66;
let playing = false;
let timer: number | undefined;
let frame: number | undefined;

function stop() {
  if (timer !== undefined) window.clearTimeout(timer);
  if (frame !== undefined) window.cancelAnimationFrame(frame);
  timer = frame = undefined;
}

function step() {
  timer = undefined;
  frame = window.requestAnimationFrame(() => {
    frame = undefined;
    const t = readPlayhead();
    if (t != null) followers.forEach((f) => f(t));
    schedule();
  });
}

function schedule() {
  stop();
  if (!playing || followers.size === 0 || typeof document === 'undefined' || document.hidden) return;
  timer = window.setTimeout(step, STEP_MS);
}

if (typeof document !== 'undefined') {
  document.addEventListener('visibilitychange', schedule);
}

/** The player started or stopped */
export function setPlayheadRunning(on: boolean) {
  playing = on;
  schedule();
}

/** Called with the player's time while it plays; returns the way to stop */
export function followPlayhead(fn: Follower): () => void {
  followers.add(fn);
  schedule();
  return () => {
    followers.delete(fn);
    if (followers.size === 0) stop();
  };
}
