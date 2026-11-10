"use client";

import React, { useCallback, useEffect, useMemo, useState } from 'react';
import { Copy, Loader2, Pause, Play, Search, Users, X } from 'lucide-react';
import { copySpeakerText } from '@/lib/copySpeaker';
import type { SpokenLine } from '@/lib/speakerText';
import { invoke } from '@tauri-apps/api/core';
import { playRange } from '@/lib/timeline';
import { listen, UnlistenFn } from '@tauri-apps/api/event';
import { toast } from '@/lib/notice';
import { speakerColorMap } from '@/lib/speakerColors';
import { Dialog, DialogClose, DialogContent, DialogDescription, DialogTitle } from '../ui/dialog';
import { SavedSpeakers, SpeakersFooter, SPEAKERS_BODY } from '../SavedSpeakers';

interface MeetingSpeaker {
  label: string;
  name: string | null;
  role: string | null;
  /** Name suggested from what was said, not confirmed yet */
  suggested: string | null;
  speech_seconds: number;
  first_seconds: number | null;
  samples: string[];
  clip_start: number | null;
  clip_end: number | null;
}

interface SpeakersDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  meetingId: string;
  onChanged?: () => Promise<void>;
  /** Shows that moment in the transcript and plays the recording from it */
  onJumpTo?: (seconds: number) => void;
  /** «Усі збережені» first */
  initialTab?: 'here' | 'all';
}

const STAGES: Record<string, string> = {
  decoding: 'Читання запису…',
  diarizing: 'Розрізнення голосів…',
  transcribing: 'Розпізнавання мовлення кожного спікера…',
  saving: 'Збереження…',
};

interface OtherVoice {
  speaker: string;
  fromSeconds: number;
  toSeconds: number;
  preview: string;
}

interface LikelySame {
  keep: string;
  merge: string;
  similarity: number;
}

interface SavedPerson {
  name: string;
  hasVoice: boolean;
  recognize: boolean;
  voiceMeetings: number;
}

const pairKey = (p: LikelySame) => [p.keep, p.merge].sort().join('|');
const dismissedKey = (meetingId: string) => `speakers-not-same:${meetingId}`;
function readDismissed(meetingId: string): string[] {
  try {
    return JSON.parse(localStorage.getItem(dismissedKey(meetingId)) ?? '[]');
  } catch {
    return [];
  }
}
function writeDismissed(meetingId: string, keys: string[]) {
  try {
    localStorage.setItem(dismissedKey(meetingId), JSON.stringify(keys));
  } catch {}
}

function duration(seconds: number): string {
  if (seconds < 60) return `${Math.round(seconds)} с`;
  return `${Math.round(seconds / 60)} хв`;
}

function mmss(seconds: number): string {
  const s = Math.floor(seconds);
  return `${String(Math.floor(s / 60)).padStart(2, '0')}:${String(s % 60).padStart(2, '0')}`;
}

const initials = (name: string) => {
  const n = name.match(/^Спікер (\d+)$/);
  if (n) return n[1];
  return name
    .replace(/\s*\(\?\)\s*$/, '')
    .split(/\s+/)
    .filter(Boolean)
    .slice(0, 2)
    .map((w) => w[0]?.toUpperCase() ?? '')
    .join('');
};

const btn = 'whitespace-nowrap rounded-full bg-[var(--selected)] px-3.5 py-2 text-sm font-medium disabled:opacity-40';

/**
 * «Спікери»: one window, two tabs of the same shape. «У цьому записі»: the
 * voices of this meeting (hear, name, role, join, split); «Усі збережені»:
 * every person the app knows. Both are a list on the left and a card on the
 * right, with the same bar below; changes are saved at once.
 */
export function SpeakersDialog({ open, onOpenChange, meetingId, onChanged, onJumpTo, initialTab = 'here' }: SpeakersDialogProps) {
  const [tab, setTab] = useState<'here' | 'all'>(initialTab);
  // A person opened from «Відкрити в "Усі збережені"»
  const [focus, setFocus] = useState<string | null>(null);
  useEffect(() => {
    if (open) {
      setTab(initialTab);
      setFocus(null);
    }
  }, [open, initialTab]);
  const [lines, setLines] = useState<SpokenLine[]>([]);
  const [speakers, setSpeakers] = useState<MeetingSpeaker[]>([]);
  const [current, setCurrent] = useState<string | null>(null);
  const [nameDraft, setNameDraft] = useState('');
  const [roleDraft, setRoleDraft] = useState('');
  const [mergeOpen, setMergeOpen] = useState(false);
  const [running, setRunning] = useState(false);
  const [stage, setStage] = useState<string | null>(null);
  const [progress, setProgress] = useState(0);
  const [available, setAvailable] = useState(true);
  const [downloading, setDownloading] = useState<number | null>(null);
  const [playing, setPlaying] = useState<string | null>(null);
  const [pairs, setPairs] = useState<LikelySame[]>([]);
  const [others, setOthers] = useState<OtherVoice[]>([]);
  const [saved, setSaved] = useState<SavedPerson[]>([]);
  const [query, setQuery] = useState('');

  const shown = (s: MeetingSpeaker) => s.name ?? (s.suggested ? `${s.suggested} (?)` : s.label);
  const colors = useMemo(() => speakerColorMap(speakers.map((s) => s.name ?? s.label)), [speakers]);
  const speaker = speakers.find((s) => s.label === current) ?? null;

  const load = useCallback(async () => {
    const list = await invoke<MeetingSpeaker[]>('get_meeting_speakers', { meetingId });
    // Name suggestions first: they need a «Так» or «Ні»
    const sorted = [...list].sort((a, b) => Number(!!b.suggested && !b.name) - Number(!!a.suggested && !a.name));
    setSpeakers(sorted);
    setCurrent((c) => (c && sorted.some((s) => s.label === c) ? c : sorted[0]?.label ?? null));
    // The lines are at hand before a click: the clipboard takes text only right after one
    const all = await invoke<{ transcripts: { text?: string; audio_start_time?: number; speaker?: string }[] }>('api_get_meeting_transcripts', { meetingId, limit: 100000, offset: 0 }).catch(() => ({ transcripts: [] }));
    setLines(all.transcripts.map((t) => ({ text: t.text, speaker: t.speaker, start: t.audio_start_time ?? 0 })));
    const dismissed = readDismissed(meetingId);
    const likely = await invoke<LikelySame[]>('get_likely_same_speakers', { meetingId }).catch(() => []);
    setPairs(likely.filter((p) => !dismissed.includes(pairKey(p))));
    setOthers(await invoke<OtherVoice[]>('get_other_voices', { meetingId }).catch(() => []));
    const people = await invoke<{ people: SavedPerson[] }>('get_saved_speakers').catch(() => ({ people: [] }));
    setSaved(people.people);
  }, [meetingId]);

  const changed = async () => {
    await load();
    await onChanged?.();
  };

  useEffect(() => {
    setNameDraft(speaker?.name ?? '');
    setRoleDraft(speaker?.role ?? '');
    setMergeOpen(false);
  }, [speaker?.label, speaker?.name, speaker?.role]);

  useEffect(() => {
    if (!open) return;
    load().catch(() => setSpeakers([]));
    invoke<boolean>('speaker_models_available').then(setAvailable).catch(() => setAvailable(false));
    invoke<boolean>('speaker_recognition_running', { meetingId }).then(setRunning).catch(() => {});
  }, [open, load]);

  useEffect(() => {
    if (!open) return;
    const unlisteners: UnlistenFn[] = [];
    listen<{ meeting_id: string; stage: string; progress_percentage: number }>('speakers-progress', (e) => {
      if (e.payload.meeting_id !== meetingId) return;
      setRunning(e.payload.stage !== 'complete');
      setStage(e.payload.stage);
      setProgress(e.payload.progress_percentage);
    }).then((u) => unlisteners.push(u));
    listen<{ progress_percentage: number }>('speaker-models-progress', (e) => setDownloading(e.payload.progress_percentage)).then((u) => unlisteners.push(u));
    listen<{ meeting_id: string }>('speakers-done', (e) => {
      if (e.payload.meeting_id !== meetingId) return;
      setRunning(false);
      setStage(null);
      load().catch(() => {});
    }).then((u) => unlisteners.push(u));
    return () => unlisteners.forEach((u) => u());
  }, [open, meetingId]);

  const act = async (what: () => Promise<unknown>, done?: string) => {
    try {
      await what();
      await changed();
      if (done) toast.success(done);
    } catch (e) {
      toast.error(`Не вдалося зберегти: ${e}`);
    }
  };

  // A few seconds of one voice, through the meeting's one player
  const playClip = (s: MeetingSpeaker) => {
    if (s.clip_start == null || s.clip_end == null) return;
    playRange(s.clip_start, s.clip_end);
    setPlaying(s.label);
    window.setTimeout(() => setPlaying((p) => (p === s.label ? null : p)), Math.max(500, (s.clip_end - s.clip_start) * 1000));
  };

  const downloadModels = async () => {
    setDownloading(0);
    try {
      await invoke('download_speaker_models');
      setAvailable(await invoke<boolean>('speaker_models_available'));
    } catch (e) {
      toast.error(`Не вдалося завантажити моделі: ${e}`);
    } finally {
      setDownloading(null);
    }
  };

  // Recognition runs in the background: the window can be closed at any time
  const identify = async () => {
    setRunning(true);
    setStage('decoding');
    setProgress(0);
    try {
      await invoke('start_speaker_recognition', { meetingId });
    } catch (e) {
      setRunning(false);
      toast.error(`Не вдалося розпізнати спікерів: ${e}`);
    }
  };

  const rename = async () => {
    if (!speaker) return;
    const wanted = nameDraft.trim().replace(/\s+/g, ' ');
    if (!wanted || wanted === (speaker.name ?? '')) {
      setNameDraft(speaker.name ?? '');
      return;
    }
    await act(() => invoke('rename_speaker', { meetingId, current: shown(speaker), name: wanted }));
  };

  const saveRole = async () => {
    if (!speaker || roleDraft.trim() === (speaker.role ?? '')) return;
    await act(() => invoke('set_speaker_role', { meetingId, current: shown(speaker), role: roleDraft.trim() }));
  };

  const close = () => onOpenChange(false);
  const person = speaker?.name ? saved.find((p) => p.name === speaker.name) : undefined;

  const hint = others[0]
    ? {
        key: `o:${others[0].speaker}:${others[0].fromSeconds}`,
        text: (
          <>
            Можливо, інший голос: <b>«{others[0].speaker}»</b> з {mmss(others[0].fromSeconds)}
          </>
        ),
        ok: 'Відокремити',
        onOk: () =>
          act(async () => {
            const label = await invoke<string>('split_speaker_voice', { meetingId, speaker: others[0].speaker, fromSeconds: others[0].fromSeconds });
            setCurrent(label);
          }, 'Відокремлено: підпишіть новий голос'),
        onNo: () => setOthers((o) => o.slice(1)),
      }
    : pairs[0]
      ? {
          key: `p:${pairKey(pairs[0])}`,
          text: (
            <>
              Схоже, один голос: <b>«{pairs[0].keep}»</b> і <b>«{pairs[0].merge}»</b>
            </>
          ),
          ok: "Об'єднати",
          onOk: () => act(() => invoke('merge_speakers', { meetingId, keep: pairs[0].keep, merge: pairs[0].merge }), `Об'єднано в «${pairs[0].keep}»`),
          onNo: () => {
            writeDismissed(meetingId, [...readDismissed(meetingId), pairKey(pairs[0])]);
            setPairs((all) => all.slice(1));
          },
        }
      : null;

  const unnamed = speakers.filter((s) => !s.name).length;

  const hereBody = () => {
    if (!available || running || speakers.length === 0) {
      return (
        <div className={`${SPEAKERS_BODY} flex-col items-center justify-center gap-3 p-8 text-center text-sm`}>
          {!available ? (
            <>
              <p className="max-w-md text-[color:var(--muted-text)]">Для розрізнення голосів потрібні дві невеликі моделі (33 МБ), один раз. Далі все працює без інтернету.</p>
              <button type="button" className={btn} onClick={downloadModels} disabled={downloading !== null}>
                {downloading !== null ? `Завантаження… ${downloading}%` : 'Завантажити моделі'}
              </button>
            </>
          ) : running ? (
            <>
              <span className="flex items-center gap-2">
                <Loader2 className="animate-spin" size={16} />
                {(stage && STAGES[stage]) || 'Обробка…'} {progress > 0 ? `${progress}%` : ''}
              </span>
              <p className="text-xs text-[color:var(--muted-text)]">Вікно можна закрити: розпізнавання триває, хід видно над стенограмою.</p>
              <button type="button" className={btn} onClick={() => invoke('stop_speaker_recognition', { meetingId }).catch(() => {})}>
                Зупинити
              </button>
            </>
          ) : (
            <p className="text-[color:var(--muted-text)]">Спікерів ще не розпізнано. Це займає кілька хвилин на годину запису.</p>
          )}
        </div>
      );
    }
    return (
      <div className={SPEAKERS_BODY}>
        {/* The voices: the same column, rows and hint as «Усі збережені» */}
        <div className="flex w-[340px] flex-none flex-col border-r border-[var(--hairline)]">
          <div className="flex flex-col gap-2 p-3 pb-2">
            <label className="flex h-8 items-center gap-1.5 rounded-lg bg-[var(--selected)] px-2">
              <Search className="h-3.5 w-3.5 flex-none text-[color:var(--muted-text)]" />
              <input
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                placeholder="Пошук у цьому записі"
                aria-label="Пошук голосу"
                className="min-w-0 flex-1 bg-transparent text-sm outline-none placeholder:text-[color:var(--muted-text)]"
              />
            </label>
            <div className="flex h-[24px] items-center px-1 text-xs text-[color:var(--muted-text)]">
              {speakers.length} {speakers.length % 10 === 1 && speakers.length % 100 !== 11 ? 'голос' : [2, 3, 4].includes(speakers.length % 10) && ![12, 13, 14].includes(speakers.length % 100) ? 'голоси' : 'голосів'}
              {unnamed > 0 ? ` · ${unnamed} без імені` : ''}
            </div>
          </div>
          {hint && (
            <div key={hint.key} className="mx-3 mb-2 flex items-center gap-2 rounded-lg bg-[var(--blue-tint)] px-2.5 py-2 text-xs">
              <span className="min-w-0 flex-1">{hint.text}</span>
              <button type="button" className="whitespace-nowrap rounded-full bg-[var(--card-bg)] px-2.5 py-1 font-medium" onClick={hint.onOk} disabled={running}>
                {hint.ok}
              </button>
              <button type="button" className="px-1 text-[color:var(--muted-text)]" onClick={hint.onNo}>
                Ні
              </button>
            </div>
          )}
          <div className="min-h-0 flex-1 overflow-y-auto px-1.5 pb-1.5">
            {speakers.filter((s) => !query.trim() || shown(s).toLowerCase().includes(query.trim().toLowerCase()) || (s.role ?? '').toLowerCase().includes(query.trim().toLowerCase())).map((s) => (
              <div key={s.label} className={`flex items-center gap-2 rounded-lg px-2 py-1.5 ${s.label === current ? 'bg-[var(--blue-tint)]' : 'hover:bg-[var(--selected)]'}`}>
                <button type="button" onClick={() => setCurrent(s.label)} className="flex min-w-0 flex-1 items-center gap-2 text-left">
                  <span
                    className="flex h-7 w-7 flex-none items-center justify-center rounded-full text-[11px] font-semibold text-white"
                    style={{ background: s.name ? colors.get(s.name) : '#8e8e93' }}
                  >
                    {initials(s.name ?? s.label)}
                  </span>
                  <span className="flex min-w-0 flex-col">
                    <span className={`truncate text-sm font-medium ${s.name ? '' : 'text-[color:var(--muted-text)]'}`}>{s.name ?? (s.suggested ? `${s.suggested}?` : s.label)}</span>
                    <span className="truncate text-xs text-[color:var(--muted-text)]">
                      {!s.name && s.suggested ? `можливо, ${s.suggested} · ` : !s.name ? 'без імені · ' : ''}
                      мовлення {duration(s.speech_seconds)}
                      {s.first_seconds != null && s.name ? ` · вперше ${mmss(s.first_seconds)}` : ''}
                    </span>
                  </span>
                </button>
                <button
                  type="button"
                  aria-label={`Послухати голос: ${s.name ?? s.label}`}
                  title="Послухати голос"
                  disabled={s.clip_start == null}
                  onClick={() => playClip(s)}
                  className="flex h-7 w-7 flex-none items-center justify-center rounded-full bg-[var(--selected)] disabled:opacity-40"
                >
                  {playing === s.label ? <Pause className="h-3 w-3" /> : <Play className="h-3 w-3" />}
                </button>
              </div>
            ))}
          </div>
        </div>

        {/* The voice: the same header, groups and buttons as a saved person */}
        {speaker && (
          <div className="flex min-w-0 flex-1 flex-col gap-4 overflow-y-auto p-5 [&>*]:shrink-0">
            <div className="flex items-center gap-3">
              <span
                className="flex h-12 w-12 flex-none items-center justify-center rounded-full text-base font-semibold text-white"
                style={{ background: speaker.name ? colors.get(speaker.name) : '#8e8e93' }}
              >
                {initials(speaker.name ?? speaker.label)}
              </span>
              <div className="flex min-w-0 flex-1 flex-col gap-0.5">
                <input
                  aria-label="Ім'я"
                  placeholder={`${speaker.label}: впишіть ім'я`}
                  value={nameDraft}
                  onChange={(e) => setNameDraft(e.target.value)}
                  onBlur={rename}
                  onKeyDown={(e) => {
                    if (e.key === 'Enter') (e.target as HTMLInputElement).blur();
                    if (e.key === 'Escape') setNameDraft(speaker.name ?? '');
                  }}
                  className="min-w-0 bg-transparent text-lg font-semibold outline-none placeholder:font-normal placeholder:text-[color:var(--muted-text)]"
                />
                <input
                  aria-label="Роль"
                  placeholder="Роль (необов'язково)"
                  title="Шаблони документа можуть ділити розмову на частини за ролями (напр. «керівник», «доповідач»)"
                  value={roleDraft}
                  onChange={(e) => setRoleDraft(e.target.value)}
                  onBlur={saveRole}
                  onKeyDown={(e) => e.key === 'Enter' && (e.target as HTMLInputElement).blur()}
                  className="min-w-0 bg-transparent text-sm text-[color:var(--muted-text)] outline-none placeholder:text-[color:var(--muted-text)]"
                />
              </div>
            </div>

            {!speaker.name && speaker.suggested && (
              <div className="flex items-center gap-2 rounded-xl bg-[var(--blue-tint)] px-3.5 py-2.5 text-sm">
                <span className="min-w-0 flex-1">
                  Можливо, це <b>{speaker.suggested}</b>: так звучало в записі.
                </span>
                <button type="button" className="rounded-full bg-[var(--card-bg)] px-3 py-1 text-xs font-medium" onClick={() => act(() => invoke('rename_speaker', { meetingId, current: shown(speaker), name: speaker.suggested }))}>
                  Так
                </button>
                <button type="button" className="px-1 text-xs text-[color:var(--muted-text)]" onClick={() => act(() => invoke('reject_speaker_suggestion', { meetingId, current: shown(speaker) }))}>
                  Ні
                </button>
              </div>
            )}

            <section className="flex flex-col gap-1.5">
              <h3 className="text-[11px] font-semibold uppercase tracking-wide text-[color:var(--muted-text)]">У цьому записі · мовлення {duration(speaker.speech_seconds)}</h3>
              <div className="overflow-hidden rounded-xl bg-[var(--card-bg)] shadow-[inset_0_0_0_0.5px_var(--hairline)]">
                {speaker.samples.map((line, i) => (
                  <p key={i} className="border-b border-[var(--hairline)] px-3.5 py-2 text-[13px] italic leading-snug last:border-b-0">
                    «{line}»
                  </p>
                ))}
                {speaker.first_seconds != null && (
                  <button type="button" onClick={() => onJumpTo?.(speaker.first_seconds!)} className="flex w-full items-center justify-between px-3.5 py-2 text-left text-sm text-[var(--blue)] hover:bg-[var(--selected)]" title="Показати в стенограмі й увімкнути запис з цього місця">
                    <span>Вперше говорить о {mmss(speaker.first_seconds)}</span>
                    <span>→</span>
                  </button>
                )}
              </div>
            </section>

            {person && (
              <div className="flex items-center gap-3 rounded-xl bg-[var(--card-bg)] px-3.5 py-2.5 shadow-[inset_0_0_0_0.5px_var(--hairline)]">
                <span className="flex min-w-0 flex-1 flex-col">
                  <span className="text-sm">Збережено в пам&apos;яті</span>
                  <span className="text-xs text-[color:var(--muted-text)]">
                    {person.hasVoice ? `Голос з ${person.voiceMeetings} ${person.voiceMeetings === 1 ? 'запису' : 'записів'} · ${person.recognize ? 'упізнається автоматично' : 'не впізнається'}` : "Лише ім'я"}
                  </span>
                </span>
                <button
                  type="button"
                  className="whitespace-nowrap text-sm font-medium text-[var(--blue)] hover:underline"
                  onClick={() => {
                    setFocus(person.name);
                    setTab('all');
                  }}
                >
                  Відкрити в «Усі збережені»
                </button>
              </div>
            )}

            <div className="flex flex-wrap items-center gap-2">
              {speakers.length > 1 && (
                <button type="button" className={btn} onClick={() => setMergeOpen((o) => !o)} aria-expanded={mergeOpen} disabled={running}>
                  Об&apos;єднати з…
                </button>
              )}
              <button type="button" className={`${btn} flex items-center gap-1.5`} onClick={(e) => copySpeakerText(lines, speaker.name ?? speaker.label, e.altKey)} title="Копіювати весь текст · з ⌥ — із часом">
                <Copy className="h-3.5 w-3.5" /> Копіювати весь текст
              </button>
            </div>
            {mergeOpen && (
              <div className="flex flex-col gap-1 rounded-xl bg-[var(--selected)] p-2">
                <span className="px-1.5 pb-1 text-xs text-[color:var(--muted-text)]">«{shown(speaker)}» і цей голос є однією людиною:</span>
                <div className="max-h-[160px] overflow-y-auto">
                  {speakers
                    .filter((s) => s.label !== speaker.label)
                    .map((s) => (
                      <button
                        key={s.label}
                        type="button"
                        className="nav-row flex h-8 w-full items-center px-2 text-left text-sm"
                        onClick={() => {
                          // The one with a name, else the one who talked more, keeps its name
                          const keepOther = !!s.name || (!speaker.name && s.speech_seconds > speaker.speech_seconds);
                          const [keep, merge] = keepOther ? [shown(s), shown(speaker)] : [shown(speaker), shown(s)];
                          act(async () => {
                            await invoke('merge_speakers', { meetingId, keep, merge });
                            setCurrent(keepOther ? s.label : speaker.label);
                          }, `Об'єднано в «${keep}»`);
                        }}
                      >
                        <span className="truncate">{shown(s)}</span>
                      </button>
                    ))}
                </div>
              </div>
            )}
          </div>
        )}
      </div>
    );
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      {/* One size for both tabs: switching moves nothing */}
      <DialogContent className="flex w-[min(960px,calc(100vw-48px))] max-w-none flex-col gap-0 p-6 pb-2 sm:max-w-none [&>button.absolute]:hidden">
        <div className="flex h-10 items-center gap-4">
          <DialogTitle className="flex items-center gap-2 text-xl font-bold">
            <Users size={20} /> Спікери
          </DialogTitle>
          <span className="step-switch" role="tablist" aria-label="Які спікери">
            <button type="button" role="tab" aria-selected={tab === 'here'} data-on={tab === 'here'} onClick={() => setTab('here')} className="!h-7 !px-3.5">
              У цьому записі
            </button>
            <button type="button" role="tab" aria-selected={tab === 'all'} data-on={tab === 'all'} onClick={() => setTab('all')} className="!h-7 !px-3.5">
              Усі збережені
            </button>
          </span>
          <span className="flex-1" />
          <DialogClose className="flex h-8 w-8 items-center justify-center rounded-full bg-[var(--selected)]" aria-label="Закрити">
            <X className="h-4 w-4" />
          </DialogClose>
        </div>
        <DialogDescription className="mb-3 mt-1.5 h-5 truncate text-[13px] text-[color:var(--muted-text)]">
          {tab === 'all'
            ? 'Усі люди, яких програма знає. Зайвих можна забути: їх не буде впізнано і запропоновано.'
            : "Голоси цього запису. Ім'я, вписане тут, у наступних записах ставиться автоматично."}
        </DialogDescription>

        {tab === 'all' ? (
          <SavedSpeakers key={focus ?? 'all'} focus={focus} onChanged={() => void changed()} onDone={close} />
        ) : (
          <>
            {hereBody()}
            <SpeakersFooter onDone={close}>
              <button type="button" className={btn} onClick={identify} disabled={running || !available}>
                {speakers.length === 0 ? 'Розпізнати спікерів' : 'Розпізнати заново'}
              </button>
              <span className="ml-1 text-xs text-[color:var(--muted-text)]">Зміни зберігаються одразу</span>
            </SpeakersFooter>
          </>
        )}
      </DialogContent>
    </Dialog>
  );
}
