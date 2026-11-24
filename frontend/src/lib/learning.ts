/**
 * Recognition that learns on this Mac (learning/ in the Rust app): the
 * types Settings gets from `api_learning_status` and the wording shared by
 * the offer, Settings → Пам'ять, the history and the result notice.
 */
import { invoke } from '@tauri-apps/api/core';

export type Mode = 'nightly' | 'manual' | 'off';
export type Outcome = 'accepted' | 'rejected' | 'stopped' | 'failed';
export type Wait =
  | 'not_this_mac'
  | 'off'
  | 'no_engine'
  | 'other_model'
  | 'low_disk'
  | 'by_hand_only'
  | 'not_enough_speech'
  | 'recording'
  | 'not_night'
  | 'on_battery'
  | 'in_use'
  | 'tried_recently';

export type Stage =
  | { stage: 'preparing' }
  | { stage: 'training'; share: number; epoch: number; epochs: number }
  | { stage: 'checking' }
  | { stage: 'finished'; outcome: Outcome };

export interface Run {
  id: string;
  outcome: Outcome;
  reason?: string;
  train_hours: number;
  held_hours: number;
  train_meetings: number;
  held_meetings: number;
  checked_hours: number;
  wer_before?: number;
  wer_after?: number;
  minutes: number;
  peak_memory_gb?: number;
}

export interface Status {
  mode: Mode | null;
  can_train_here: boolean;
  engine_ready: boolean;
  engine_download_mb: number;
  running: boolean;
  stage: Stage | null;
  wait: Wait | null;
  wait_by_hand: Wait | null;
  checked_hours: number;
  new_hours: number;
  /** `base`, `unset` or the id of the version in use */
  active: string;
  tuned_in_use: boolean;
  versions: Run[];
  history: Run[];
  offer: boolean;
  unseen: Run | null;
  run_minutes: number | null;
  training_minutes: number | null;
  engine_download: number | null;
  hand_copied: boolean;
}

/** Corrections of checked lines by kind (corrected.rs) */
export interface Edits {
  numbers: number;
  names: number;
  endings: number;
  replaced: number;
  added: number;
  fillers: number;
  removed: number;
  punctuation_lines: number;
  speaker_lines: number;
  /** Of names: dictionary words written small (words of longer terms) */
  names_lower: number;
  /** Of names: the same change made earlier in the same meeting */
  names_repeats: number;
  /** Any change made earlier in the same meeting */
  repeats: number;
}

export interface QualityMeeting {
  date: string;
  words: number;
  corrected: number;
  tuned: boolean;
  edits: Edits;
}

export interface Quality {
  meetings: QualityMeeting[];
  trainings: string[];
}

/** New checked speech needed for a run (schedule.rs) */
export const MIN_HOURS = 2;

/** Fewer mistakes per 100 words a new model needs to replace the one in use (gate.rs) */
export const MIN_GAIN = 0.5;

/** Rejected as «not better» although it made fewer mistakes, only too few fewer */
export function slightlyBetter(run: Run): boolean {
  return run.reason === 'NotBetter' && run.wer_before != null && run.wer_after != null && run.wer_after < run.wer_before;
}

export const STATUS_CHANGED = 'protokol:learning-changed';

export function loadStatus(): Promise<Status> {
  return invoke<Status>('api_learning_status');
}

/** Other places showing learning read the status again */
export function statusChanged() {
  window.dispatchEvent(new Event(STATUS_CHANGED));
}

/** 15,8 */
export function num(v: number, digits = 1): string {
  return v.toFixed(digits).replace('.', ',');
}

/** 15,8% */
export function pct(v: number | undefined | null): string {
  return v == null ? '—' : `${num(v)}%`;
}

/** 1 год 10 хв, 45 хв, 2 год */
export function hoursText(hours: number): string {
  const total = Math.max(0, Math.round(hours * 60));
  const h = Math.floor(total / 60);
  const m = total % 60;
  if (h === 0) return `${m} хв`;
  return m === 0 ? `${h} год` : `${h} год ${m} хв`;
}

/** `2026-11-15T01-30` → 15.11 */
export function runDate(id: string): string {
  const m = /^\d{4}-(\d{2})-(\d{2})/.exec(id);
  return m ? `${m[2]}.${m[1]}` : id;
}

/** `2026-11-15` → 15.11 */
export function dayText(date: string): string {
  const m = /^\d{4}-(\d{2})-(\d{2})$/.exec(date);
  return m ? `${m[2]}.${m[1]}` : date;
}

export const MODE_LABEL: Record<Mode, string> = {
  nightly: 'Уночі автоматично',
  manual: 'Лише вручну',
  off: 'Вимкнено',
};

/** Why «Почати» does not start a run now */
export function waitText(wait: Wait): string {
  switch (wait) {
    case 'not_this_mac':
      return 'Потрібен Mac з Apple Silicon і щонайменше 16 ГБ пам\'яті';
    case 'off':
      return 'Донавчання вимкнено';
    case 'no_engine':
      return 'Спершу потрібно встановити модуль навчання';
    case 'other_model':
      return 'Донавчається лише модель Whisper Large V3';
    case 'low_disk':
      return 'Потрібно щонайменше 10 ГБ вільного місця на диску';
    case 'not_enough_speech':
      return `Потрібно щонайменше ${MIN_HOURS} год перевіреного мовлення`;
    case 'recording':
      return 'Почати можна після завершення запису';
    default:
      return 'Зараз почати не можна';
  }
}

/** The state of a run in the history */
export function outcomeText(run: Run): string {
  const r = run.reason ?? '';
  switch (run.outcome) {
    case 'accepted':
      return 'Прийнято';
    case 'rejected':
      if (r === 'OneMeetingWorse') return 'Не прийнято: гірше на одній з нарад';
      if (r === 'TooLittleToTell') return 'Не прийнято: замало мовлення для перевірки';
      if (slightlyBetter(run)) return 'Не прийнято: недостатньо точна';
      return 'Не прийнято: не точніша';
    case 'stopped':
      if (r === 'TooMuchMemory' || r === 'Swapping') return "Зупинено: забракло пам'яті";
      if (r === 'TooSlow') return 'Зупинено: Mac працював надто повільно';
      return 'Зупинено вручну';
    default:
      return 'Не вдалося завершити';
  }
}

/**
 * The model to go back to from the one in use: the version before it if it
 * is still on disk, else the file copied by hand, else the original
 */
export function previousChoice(status: Status): string {
  const older = status.versions.find((v) => v.id !== status.active);
  if (older) return older.id;
  if (status.hand_copied && status.active !== 'unset') return 'unset';
  return 'base';
}

/**
 * Downloads and installs the learning engine, with its progress in the
 * notice; `true` once it is installed. `fromFile`: the archive copied by
 * hand is picked instead (a Mac without the internet).
 */
export async function installEngine(fromFile = false): Promise<boolean> {
  const { listen } = await import('@tauri-apps/api/event');
  const { toast } = await import('@/lib/notice');
  const id = 'learning-engine';
  if (fromFile) {
    try {
      const picked = await invoke<boolean>('api_learning_engine_choose_file');
      if (picked) toast.success('Модуль навчання встановлено', { id });
      statusChanged();
      return picked;
    } catch (e) {
      toast.error('Не вдалося встановити модуль навчання', {
        id,
        description: 'Поруч з архівом має лежати файл підпису .sig, отриманий разом з ним.',
      });
      console.warn('learning engine from file:', e);
      return false;
    }
  }
  toast.loading('Завантаження модуля навчання…', { id });
  const un = await listen<{ percent: number }>('learning-engine-progress', (e) => {
    if (e.payload.percent < 100) toast.loading(`Завантаження модуля навчання… ${e.payload.percent}%`, { id });
  });
  statusChanged();
  try {
    await invoke('api_learning_engine_download');
    toast.success('Модуль навчання встановлено', { id });
    return true;
  } catch (e) {
    console.warn('learning engine download:', e);
    toast.error('Не вдалося завантажити модуль навчання', {
      id,
      description: "Перевірте з'єднання з інтернетом. Без інтернету модуль встановлюється з файлу в Налаштуваннях → Пам'ять.",
    });
    return false;
  } finally {
    un();
    statusChanged();
  }
}
