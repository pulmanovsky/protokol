/**
 * The windows' side of the journal (src-tauri/src/logfile.rs, report.rs).
 * Script errors and failed promises of a window become one line each in the
 * journal; the line is cleaned again in Rust (no Ukrainian text, quoted values
 * or paths), so names and titles that were in an error's text stay out.
 */
import { invoke } from '@tauri-apps/api/core';

export interface JournalMark {
  n: number;
  /** UTC, «2026-11-21T10:42:07Z» */
  t: string;
  note?: string | null;
  source: string;
}

const SKIP = [
  // Noise of the browser engine, not of Protokol
  'ResizeObserver loop',
  'Script error.',
];

function short(value: unknown): string {
  if (value instanceof Error) return `${value.name}: ${value.message}`;
  if (typeof value === 'string') return value;
  try {
    return JSON.stringify(value) ?? String(value);
  } catch {
    return String(value);
  }
}

/** The first frames of a stack: file names and lines only */
function frames(stack: string | undefined): string {
  if (!stack) return '';
  return stack
    .split('\n')
    .slice(1, 4)
    .map((l) => l.trim().replace(/^at\s+/, '').replace(/https?:\/\/[^/]+/g, ''))
    .join(' | ');
}

function send(kind: string, message: string, source: string) {
  if (!message || SKIP.some((s) => message.includes(s))) return;
  void invoke('api_journal_ui', { kind, message: message.slice(0, 400), source: source.slice(0, 200) }).catch(() => {});
}

/** Starts sending this window's errors to the journal; returns the undo */
export function watchWindowErrors(windowName: string): () => void {
  const onError = (e: ErrorEvent) => {
    const where = e.filename ? `${e.filename.replace(/https?:\/\/[^/]+/, '')}:${e.lineno}` : '';
    send('error', short(e.error ?? e.message), [windowName, where, frames((e.error as Error | undefined)?.stack)].filter(Boolean).join(' '));
  };
  const onRejection = (e: PromiseRejectionEvent) => {
    send('promise', short(e.reason), [windowName, frames((e.reason as Error | undefined)?.stack)].filter(Boolean).join(' '));
  };
  window.addEventListener('error', onError);
  window.addEventListener('unhandledrejection', onRejection);
  return () => {
    window.removeEventListener('error', onError);
    window.removeEventListener('unhandledrejection', onRejection);
  };
}

/** A React error caught by an error boundary */
export function journalRenderError(error: unknown, windowName: string) {
  send('render', short(error), `${windowName} ${frames((error as Error | undefined)?.stack)}`.trim());
}

/** Local time of a mark, «10:42» today or «21.11 10:42» */
export function markTime(t: string): string {
  const d = new Date(t);
  if (Number.isNaN(d.getTime())) return t;
  const time = d.toLocaleTimeString('uk-UA', { hour: '2-digit', minute: '2-digit' });
  const today = new Date();
  const sameDay = d.toDateString() === today.toDateString();
  return sameDay ? time : `${d.toLocaleDateString('uk-UA', { day: '2-digit', month: '2-digit' })} ${time}`;
}
