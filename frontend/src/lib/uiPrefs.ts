/**
 * Settings kept on this Mac by the window itself (Загальні: start screen,
 * returning to the place; Вигляд: text size). Each read and write is guarded:
 * without storage the defaults apply.
 */

export type TextSize = 'small' | 'normal' | 'large';
export type StartScreen = 'today' | 'records' | 'last';

export const TEXT_SIZE_EVENT = 'protokol:text-size';
const SIZES: TextSize[] = ['small', 'normal', 'large'];

function read(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function write(key: string, value: string) {
  try {
    localStorage.setItem(key, value);
  } catch {
    /* defaults apply */
  }
}

// ===== Вигляд → «Розмір тексту» =====

export function textSize(): TextSize {
  const v = read('protokol-text-size');
  return v === 'small' || v === 'large' ? v : 'normal';
}

/** Applies the size to the transcript and the document (CSS: html[data-text-size]) */
export function applyTextSize(size: TextSize = textSize()) {
  if (typeof document !== 'undefined') document.documentElement.dataset.textSize = size;
}

export function setTextSize(size: TextSize) {
  write('protokol-text-size', size);
  applyTextSize(size);
  window.dispatchEvent(new CustomEvent(TEXT_SIZE_EVENT, { detail: size }));
}

/** ⌘+ / ⌘− in the menu: one step; ⌘0: the usual size */
export function stepTextSize(step: 1 | -1 | 0) {
  if (step === 0) return setTextSize('normal');
  const i = SIZES.indexOf(textSize()) + step;
  setTextSize(SIZES[Math.max(0, Math.min(SIZES.length - 1, i))]);
}

// ===== Загальні → «Після запуску відкривати» =====

export function startScreen(): StartScreen {
  const v = read('protokol-start-screen');
  return v === 'records' || v === 'last' ? v : 'today';
}

export function setStartScreen(v: StartScreen) {
  write('protokol-start-screen', v);
}

export function rememberLastMeeting(id: string) {
  write('protokol-last-meeting', id);
}

/** Where the window goes once, right after the app starts (null: stay on «Сьогодні») */
export function startRoute(): string | null {
  const choice = startScreen();
  if (choice === 'records') return '/protocols';
  if (choice === 'last') {
    const id = read('protokol-last-meeting');
    return id ? `/meeting-details?id=${encodeURIComponent(id)}` : null;
  }
  return null;
}

// ===== Загальні → «Повертатися до місця в стенограмі» =====

export function restorePlace(): boolean {
  return read('protokol-restore-place') !== 'off';
}

export function setRestorePlace(on: boolean) {
  write('protokol-restore-place', on ? 'on' : 'off');
}

export function savedScroll(meetingId: string): number {
  return Number(read(`protokol-scroll:${meetingId}`)) || 0;
}

export function saveScroll(meetingId: string, top: number) {
  write(`protokol-scroll:${meetingId}`, String(Math.round(top)));
}
