'use client';

import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { FileText, FolderOpen, Share } from 'lucide-react';
import { toast } from '@/lib/notice';
import { readModels } from '@/lib/feedback';
import { markTime, type JournalMark } from '@/lib/journal';

/**
 * «Повідомити про проблему»: the report is put together on this Mac
 * (src-tauri/src/report.rs), every file can be opened before saving, and the
 * zip goes to the Desktop. Nothing is sent: the person passes the file on
 * themselves.
 */

interface ReportFile {
  name: string;
  bytes: number;
  about: string;
}

interface Prepared {
  code: string;
  files: ReportFile[];
  marks: JournalMark[];
}

interface Saved {
  path: string;
  code: string;
  bytes: number;
}

interface MeetingItem {
  id: string;
  title: string;
  created_at?: string;
}

/** Files written only when the report is saved */
const WRITTEN_ON_SAVE = new Set(['опис.txt', 'зміст.txt']);

const SOURCES: Record<string, string> = {
  menu: 'меню',
  'menu-bar-icon': 'значок у рядку меню',
  'recording-panel': 'панель запису',
  'meet-window': 'вікно Meet',
  window: 'вікно програми',
};

export function sizeText(bytes: number): string {
  if (bytes < 1024) return `${bytes} Б`;
  if (bytes < 1024 * 1024) return `${Math.max(1, Math.round(bytes / 1024))} КБ`;
  return `${(bytes / 1024 / 1024).toFixed(1).replace('.', ',')} МБ`;
}

/** The meeting open in the main window, if any */
function openMeetingId(): string {
  if (typeof window === 'undefined' || !window.location.pathname.startsWith('/meeting-details')) return '';
  return new URLSearchParams(window.location.search).get('id') ?? '';
}

function meetingLabel(m: MeetingItem): string {
  const when = m.created_at ? new Date(m.created_at) : null;
  const date = when && !Number.isNaN(when.getTime()) ? when.toLocaleString('uk-UA', { day: '2-digit', month: '2-digit', hour: '2-digit', minute: '2-digit' }) : '';
  return [m.title || 'Без назви', date].filter(Boolean).join(' · ');
}

interface Props {
  what: string;
  setWhat: (v: string) => void;
  expected: string;
  setExpected: (v: string) => void;
  onClose: () => void;
  /** «Створити публічне звернення на GitHub»: the same words, no journal */
  onPublic: () => void;
}

export function ProblemReport({ what, setWhat, expected, setExpected, onClose, onPublic }: Props) {
  const [meetings, setMeetings] = useState<MeetingItem[]>([]);
  const [meetingId, setMeetingId] = useState<string>(openMeetingId);
  const [prepared, setPrepared] = useState<Prepared | null>(null);
  const [preparing, setPreparing] = useState(true);
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState<Saved | null>(null);
  const codeRef = useRef<string | null>(null);
  const savedRef = useRef(false);
  const shareRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    void invoke<MeetingItem[]>('api_get_meetings')
      .then((list) => {
        const sorted = [...(list ?? [])].sort((a, b) => (b.created_at ?? '').localeCompare(a.created_at ?? ''));
        setMeetings(sorted.slice(0, 30));
      })
      .catch(() => setMeetings([]));
  }, []);

  // The files are put together again whenever the meeting changes
  useEffect(() => {
    let alive = true;
    setPreparing(true);
    (async () => {
      const models = await readModels();
      const before = codeRef.current;
      if (before) void invoke('api_report_discard', { code: before }).catch(() => {});
      try {
        const p = await invoke<Prepared>('api_report_prepare', { meetingId: meetingId || null, models: models || null });
        if (!alive) {
          void invoke('api_report_discard', { code: p.code }).catch(() => {});
          return;
        }
        codeRef.current = p.code;
        setPrepared(p);
      } catch (e) {
        if (alive) toast.error('Не вдалося зібрати файли звіту', { description: `${e}` });
      } finally {
        if (alive) setPreparing(false);
      }
    })();
    return () => {
      alive = false;
    };
  }, [meetingId]);

  // Closed without saving: the gathered files are deleted
  useEffect(
    () => () => {
      if (!savedRef.current && codeRef.current) void invoke('api_report_discard', { code: codeRef.current }).catch(() => {});
    },
    [],
  );

  const open = useCallback(
    (name: string) => {
      if (!prepared) return;
      void invoke('api_report_open', { code: prepared.code, name }).catch((e) => toast.error(`${e}`));
    },
    [prepared],
  );

  const save = async () => {
    if (!what.trim()) {
      toast.error('Заповніть поле «Що сталося?»');
      return;
    }
    if (!prepared) return;
    setSaving(true);
    try {
      const s = await invoke<Saved>('api_report_save', { code: prepared.code, what, expected, meetingId: meetingId || null });
      savedRef.current = true;
      setSaved(s);
    } catch (e) {
      toast.error('Звіт не збережено', { description: `${e}` });
    } finally {
      setSaving(false);
    }
  };

  const share = () => {
    if (!saved) return;
    const r = shareRef.current?.getBoundingClientRect();
    void invoke('api_report_share', { path: saved.path, x: r?.left ?? 0, y: r?.top ?? 0, width: r?.width ?? 1, height: r?.height ?? 1 }).catch((e) => toast.error(`${e}`));
  };

  if (saved) {
    const fileName = saved.path.split('/').pop() ?? '';
    return (
      <>
        <div className="min-h-0 flex-1 overflow-y-auto custom-scrollbar -mx-1 px-1">
          <div className="flex flex-col gap-3 text-[13px] leading-relaxed">
            <p className="font-semibold">Звіт збережено на робочому столі</p>
            <div className="settings-list">
              <div className="flex items-center gap-2.5 px-3 py-2.5">
                <FileText className="h-4 w-4 flex-none text-[color:var(--muted-text)]" aria-hidden="true" />
                <span className="min-w-0 flex-1 break-words">{fileName}</span>
                <span className="text-xs text-[color:var(--muted-text)]">{sizeText(saved.bytes)}</span>
              </div>
            </div>
            <p>
              Надішліть цей файл тому, хто супроводжує Протокол, зручним способом: Signal, пошта або флешка. Програма нічого не надсилає
              сама.
            </p>
            <p className="text-[color:var(--muted-text)]">Код звіту ЗВ-{saved.code} записано в журнал: за ним звіт легко знайти.</p>
          </div>
        </div>
        <div className="flex flex-wrap items-center gap-2 pt-1">
          <button type="button" className="settings-button inline-flex items-center gap-1.5" onClick={() => void invoke('api_report_reveal', { path: saved.path }).catch((e) => toast.error(`${e}`))}>
            <FolderOpen className="h-3.5 w-3.5" aria-hidden="true" />
            Показати у Finder
          </button>
          <button ref={shareRef} type="button" className="settings-button inline-flex items-center gap-1.5" onClick={share}>
            <Share className="h-3.5 w-3.5" aria-hidden="true" />
            Поділитися…
          </button>
          <span className="flex-1" />
          <button type="button" className="accent-button h-8 rounded-lg px-3 text-sm font-medium" onClick={onClose}>
            Готово
          </button>
        </div>
      </>
    );
  }

  const files = prepared?.files ?? [];
  const marks = prepared?.marks ?? [];

  return (
    <>
      <div className="min-h-0 flex-1 overflow-y-auto custom-scrollbar -mx-1 px-1">
        <div className="flex flex-col gap-3.5">
          <label className="flex flex-col gap-1.5 text-[13px] font-semibold">
            Що сталося?
            <textarea
              className="min-h-[72px] px-2.5 py-2 text-[13px] font-normal leading-relaxed"
              value={what}
              placeholder="Наприклад: після наради в Meet знайдено лише двох спікерів, хоча виступало десять."
              onChange={(e) => setWhat(e.target.value)}
              autoFocus
            />
          </label>
          <label className="flex flex-col gap-1.5 text-[13px] font-semibold">
            Що мало статися?
            <textarea
              className="min-h-[52px] px-2.5 py-2 text-[13px] font-normal leading-relaxed"
              value={expected}
              placeholder="Наприклад: кожен доповідач окремим спікером з іменем з Meet."
              onChange={(e) => setExpected(e.target.value)}
            />
          </label>
          <label className="flex flex-col gap-1.5 text-[13px] font-semibold">
            Нарада, з якою сталася проблема
            <select className="h-8 px-2 text-[13px] font-normal" value={meetingId} onChange={(e) => setMeetingId(e.target.value)} disabled={preparing}>
              <option value="">Без наради</option>
              {meetings.map((m) => (
                <option key={m.id} value={m.id}>
                  {meetingLabel(m)}
                </option>
              ))}
            </select>
            <span className="text-xs font-normal text-[color:var(--muted-text)]">
              До звіту додаються журнал цієї наради і технічні дані про неї. Назва наради, стенограма, документ і звук не додаються.
            </span>
          </label>

          {marks.length > 0 && (
            <div className="flex flex-col gap-1.5">
              <span className="text-[13px] font-semibold">Позначки проблем за 7 днів</span>
              <div className="settings-list">
                {marks.slice(-8).map((m) => (
                  <div key={m.n} className="flex items-center gap-2.5 px-3 py-1.5 text-[13px] border-t border-[var(--hairline)] first:border-t-0">
                    <span className="tabular-nums">№{m.n}</span>
                    <span className="tabular-nums text-[color:var(--muted-text)]">{markTime(m.t)}</span>
                    <span className="min-w-0 flex-1 truncate text-[color:var(--muted-text)]">{SOURCES[m.source] ?? m.source}</span>
                  </div>
                ))}
              </div>
            </div>
          )}

          <div className="flex flex-col gap-1.5">
            <div className="flex items-baseline gap-2">
              <span className="text-[13px] font-semibold">Що буде у звіті</span>
              <span className="flex-1" />
              {prepared && (
                <button type="button" className="text-xs text-[color:var(--blue)] hover:underline" onClick={() => void invoke('api_report_show_all', { code: prepared.code }).catch((e) => toast.error(`${e}`))}>
                  Показати всі файли
                </button>
              )}
            </div>
            <div className="settings-list">
              {preparing && !prepared ? (
                <div className="px-3 py-2.5 text-[13px] text-[color:var(--muted-text)]">Збирання файлів…</div>
              ) : (
                files.map((f) => {
                  const later = WRITTEN_ON_SAVE.has(f.name);
                  return (
                    <div key={f.name} className="flex items-center gap-2.5 px-3 py-2 text-[13px] border-t border-[var(--hairline)] first:border-t-0">
                      <span className="min-w-0 flex-1">
                        <span className="block break-words">{f.name}</span>
                        <span className="block text-xs text-[color:var(--muted-text)]">{f.about}</span>
                      </span>
                      <span className="flex-none text-xs tabular-nums text-[color:var(--muted-text)]">{later ? 'під час збереження' : sizeText(f.bytes)}</span>
                      {!later && (
                        <button type="button" className="settings-button flex-none" onClick={() => open(f.name)} disabled={preparing}>
                          Переглянути
                        </button>
                      )}
                    </div>
                  );
                })
              )}
            </div>
            <p className="text-xs text-[color:var(--muted-text)]">
              Звіт зберігається на цьому Mac і нікуди не надсилається. Імена учасників замінено на «Учасник 1, 2…», назви пристроїв приховано. Звук,
              тексти стенограм і документів, назви нарад та ім'я користувача Mac не додаються ніколи.
            </p>
          </div>

          <button type="button" className="self-start text-xs text-[color:var(--muted-text)] hover:underline" onClick={onPublic}>
            Створити публічне звернення на GitHub (без журналу)
          </button>
        </div>
      </div>

      <div className="flex flex-wrap items-center gap-2 pt-1">
        <span className="flex-1" />
        <button type="button" className="settings-button" onClick={onClose}>
          Скасувати
        </button>
        <button type="button" className="accent-button h-8 rounded-lg px-3 text-sm font-medium disabled:opacity-60" onClick={() => void save()} disabled={!prepared || preparing || saving}>
          {saving ? 'Збереження…' : 'Зберегти звіт'}
        </button>
      </div>
    </>
  );
}
