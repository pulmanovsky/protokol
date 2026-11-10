/**
 * Document templates: what the cards and the template menu show, and the
 * secretary's choices kept on this Mac (the main template, the template
 * last used at each hour of the day).
 */

/** «Підсумки доповідачів»: the main template unless another is chosen */
export const MAIN_TEMPLATE_DEFAULT = 'meeting_materials_uk';
export const OVERALL_BLOCK = 'Загальний підсумок';

export interface DocumentTemplate {
  id: string;
  name: string;
  description: string;
  /** What the document has, in order */
  blocks: string[];
  /** Each speaker by the template's fields */
  byFields: boolean;
  builtIn: boolean;
}

export interface DocumentVariant {
  templateId: string;
  createdAt: string;
  updatedAt: string;
  current: boolean;
}

const MAIN_KEY = 'protokol.mainTemplate';
const BY_HOUR_ON_KEY = 'protokol.templateByHour.on';
const BY_HOUR_KEY = 'protokol.templateByHour';
export const TEMPLATES_CHANGED_EVENT = 'protokol:templates-changed';

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
    // a convenience on this Mac only
  }
}

export function mainTemplate(): string {
  return read(MAIN_KEY) || MAIN_TEMPLATE_DEFAULT;
}

export function setMainTemplate(id: string) {
  write(MAIN_KEY, id);
  window.dispatchEvent(new Event(TEMPLATES_CHANGED_EVENT));
}

/** «Пам'ятати вибір за часом дня» (on unless switched off) */
export function remembersByHour(): boolean {
  return read(BY_HOUR_ON_KEY) !== '0';
}

export function setRemembersByHour(on: boolean) {
  write(BY_HOUR_ON_KEY, on ? '1' : '0');
}

function byHour(): Record<string, string> {
  try {
    return JSON.parse(read(BY_HOUR_KEY) || '{}') ?? {};
  } catch {
    return {};
  }
}

/** The template last used for a meeting that started at this hour (or the hour next to it) */
export function templateForHour(hour: number | null): string | null {
  if (hour === null || !remembersByHour()) return null;
  const map = byHour();
  return map[String(hour)] ?? map[String(hour - 1)] ?? map[String(hour + 1)] ?? null;
}

export function rememberTemplateForHour(hour: number | null, id: string) {
  if (hour === null || !remembersByHour()) return;
  write(BY_HOUR_KEY, JSON.stringify({ ...byHour(), [String(hour)]: id }));
}

/** The hour a meeting started, from its title («Запис 06.11.2026 08:30»); an import has none */
export function meetingHour(title?: string): number | null {
  const match = (title ?? '').match(/\b(\d{1,2}):\d{2}\b/);
  if (!match) return null;
  const hour = Number(match[1]);
  return hour < 24 ? hour : null;
}

/** Only the transcript: no model needed */
export function isTranscriptOnly(t: Pick<DocumentTemplate, 'blocks'>): boolean {
  return t.blocks.every((b) => b === 'Стенограма');
}

/**
 * About how long the document takes, in minutes, for a meeting of
 * `minutes` (measured on an M-series Mac with 24 GB: speakers' points
 * ≈ 0.12 min per minute of meeting, decisions and tasks ≈ 0.05)
 */
export function estimateMinutes(t: Pick<DocumentTemplate, 'blocks' | 'byFields'>, minutes: number | null): number | null {
  if (isTranscriptOnly(t)) return 0;
  const m = minutes && minutes > 0 ? minutes : 30;
  let total = 0;
  if (t.blocks.includes('Хід наради')) total += m * (t.byFields ? 0.16 : 0.12);
  if (t.blocks.some((b) => b === 'Рішення' || b === 'Завдання' || b === 'Відкриті питання')) total += m * 0.05;
  if (t.blocks.includes(OVERALL_BLOCK)) total += 0.4;
  if (t.blocks.includes('Контроль')) total += 0.5;
  return Math.max(1, Math.round(total));
}

export function estimateLabel(t: Pick<DocumentTemplate, 'blocks' | 'byFields'>, minutes: number | null): string {
  const n = estimateMinutes(t, minutes);
  return n === 0 ? 'одразу' : `≈ ${n} хв`;
}

/** «10:42» for today, «09.11 10:42» for an earlier day */
export function variantTime(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return '';
  const time = d.toLocaleTimeString('uk-UA', { hour: '2-digit', minute: '2-digit' });
  const today = new Date();
  if (d.toDateString() === today.toDateString()) return time;
  return `${String(d.getDate()).padStart(2, '0')}.${String(d.getMonth() + 1).padStart(2, '0')} ${time}`;
}

/** The blocks as the secretary reads them */
export function blockLabel(block: string, byFields = false): string {
  switch (block) {
    case 'Хід наради':
      return byFields ? 'Кожен доповідач за полями' : 'Підсумок кожного доповідача';
    case 'Стенограма':
      return 'Повна стенограма';
    case 'Контроль':
      return 'Контроль попередніх завдань';
    case 'Не доповідали':
      return 'Хто не доповідав';
    default:
      return block;
  }
}
