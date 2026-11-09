"use client";

import React, { useCallback, useEffect, useRef, useState } from 'react';
import { Copy, Loader2, Pause, Play, Users } from 'lucide-react';
import { copySpeakerText } from '@/lib/copySpeaker';
import type { SpokenLine } from '@/lib/speakerText';
import { convertFileSrc, invoke } from '@tauri-apps/api/core';
import { playRange } from '@/lib/timeline';
import { listen, UnlistenFn } from '@tauri-apps/api/event';
import { toast } from '@/lib/notice';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '../ui/dialog';
import { Button } from '../ui/button';

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

/**
 * «Спікери»: recognise who spoke by voice and give the voices names.
 * Each voice can be heard (a few seconds of it), with its first lines, so it
 * can be told apart. Names are optional: a heading can also be renamed in the
 * document later. A name given here is remembered by voice (on this Mac).
 */
export function SpeakersDialog({ open, onOpenChange, meetingId, onChanged, onJumpTo }: SpeakersDialogProps) {
  const [lines, setLines] = useState<SpokenLine[]>([]);
  const [speakers, setSpeakers] = useState<MeetingSpeaker[]>([]);
  const [names, setNames] = useState<Record<string, string>>({});
  const [roles, setRoles] = useState<Record<string, string>>({});
  const [selected, setSelected] = useState<string[]>([]);
  const [merging, setMerging] = useState(false);
  const [running, setRunning] = useState(false);
  const [stage, setStage] = useState<string | null>(null);
  const [progress, setProgress] = useState(0);
  const [available, setAvailable] = useState(true);
  const [downloading, setDownloading] = useState<number | null>(null);
  const [playing, setPlaying] = useState<string | null>(null);
  const [pairs, setPairs] = useState<LikelySame[]>([]);
  const [others, setOthers] = useState<OtherVoice[]>([]);
  const audioRef = useRef<HTMLAudioElement | null>(null);
  const stopAtRef = useRef<number>(0);

  const shown = (s: MeetingSpeaker) => s.name ?? (s.suggested ? `${s.suggested} (?)` : s.label);

  const confirmSuggestion = async (s: MeetingSpeaker) => {
    try {
      await invoke('rename_speaker', { meetingId, current: shown(s), name: s.suggested });
      await load();
      await onChanged?.();
    } catch (e) {
      toast.error(`Не вдалося зберегти: ${e}`);
    }
  };

  const rejectSuggestion = async (s: MeetingSpeaker) => {
    try {
      await invoke('reject_speaker_suggestion', { meetingId, current: shown(s) });
      await load();
      await onChanged?.();
    } catch (e) {
      toast.error(`Не вдалося зберегти: ${e}`);
    }
  };

  const load = useCallback(async () => {
    const list = await invoke<MeetingSpeaker[]>('get_meeting_speakers', { meetingId });
    // Name suggestions first: they need a «Так» or «Ні»
    setSpeakers([...list].sort((a, b) => Number(!!b.suggested && !b.name) - Number(!!a.suggested && !a.name)));
    setNames(Object.fromEntries(list.map((s) => [s.label, s.name ?? ''])));
    setRoles(Object.fromEntries(list.map((s) => [s.label, s.role ?? ''])));
    setSelected([]);
    // The lines are at hand before a click: the clipboard takes text only right after one
    const all = await invoke<{ transcripts: { text?: string; audio_start_time?: number; speaker?: string }[] }>('api_get_meeting_transcripts', { meetingId, limit: 100000, offset: 0 }).catch(() => ({ transcripts: [] }));
    setLines(all.transcripts.map((t) => ({ text: t.text, speaker: t.speaker, start: t.audio_start_time ?? 0 })));
    const dismissed = readDismissed(meetingId);
    const likely = await invoke<LikelySame[]>('get_likely_same_speakers', { meetingId }).catch(() => []);
    setPairs(likely.filter((p) => !dismissed.includes(pairKey(p))));
    setOthers(await invoke<OtherVoice[]>('get_other_voices', { meetingId }).catch(() => []));
  }, [meetingId]);

  // One voice's lines that sound like a second report: give them their own voice
  const splitOff = async (o: OtherVoice) => {
    try {
      const label = await invoke<string>('split_speaker_voice', { meetingId, speaker: o.speaker, fromSeconds: o.fromSeconds });
      await load();
      await onChanged?.();
      toast.success(`Відокремлено як «${label}» — підпишіть його нижче`);
    } catch (e) {
      toast.error(`Не вдалося відокремити: ${e}`);
    }
  };

  const mergePair = async (p: LikelySame) => {
    try {
      await invoke('merge_speakers', { meetingId, keep: p.keep, merge: p.merge });
      await load();
      await onChanged?.();
      toast.success(`Об'єднано в «${p.keep}»`);
    } catch (e) {
      toast.error(`Не вдалося об'єднати: ${e}`);
    }
  };

  const dismissPair = (p: LikelySame) => {
    writeDismissed(meetingId, [...readDismissed(meetingId), pairKey(p)]);
    setPairs((all) => all.filter((x) => pairKey(x) !== pairKey(p)));
  };

  useEffect(() => {
    if (!open) {
      audioRef.current?.pause();
      return;
    }
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

  // A few seconds of one voice, from the meeting's own recording
  const playClip = async (s: MeetingSpeaker) => {
    if (s.clip_start == null || s.clip_end == null) return;
    // Through the meeting's one player (no second sound on top of it)
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

  // Recognition runs in the background (the same job as the automatic one):
  // the window can be closed at any time, the banner over the transcript
  // keeps showing progress
  const identify = async () => {
    audioRef.current?.pause();
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

  // The selected voices become one: the one who talked most keeps its label
  const merge = async () => {
    const chosen = speakers.filter((s) => selected.includes(s.label));
    if (chosen.length < 2) return;
    const [keep, ...rest] = chosen; // already sorted by talk time
    try {
      for (const m of rest) {
        await invoke('merge_speakers', { meetingId, keep: shown(keep), merge: shown(m) });
      }
      await load();
      await onChanged?.();
      toast.success(`Об'єднано в «${shown(keep)}»`);
    } catch (e) {
      toast.error(`Не вдалося об'єднати: ${e}`);
    }
  };

  const save = async () => {
    try {
      let changed = 0;
      for (const s of speakers) {
        const wanted = (names[s.label] ?? '').trim();
        let current = shown(s);
        if (wanted && wanted !== current) {
          await invoke('rename_speaker', { meetingId, current, name: wanted });
          current = wanted;
          changed += 1;
        }
        const role = (roles[s.label] ?? '').trim();
        if (role !== (s.role ?? '')) {
          await invoke('set_speaker_role', { meetingId, current, role });
          changed += 1;
        }
      }
      await load();
      await onChanged?.();
      if (changed > 0) toast.success('Збережено');
      onOpenChange(false);
    } catch (e) {
      toast.error(`Не вдалося зберегти: ${e}`);
    }
  };

  const toggle = (label: string) =>
    setSelected((sel) => (sel.includes(label) ? sel.filter((l) => l !== label) : [...sel, label]));

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-2xl">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            <Users size={18} /> Спікери
          </DialogTitle>
          <DialogDescription>
            Послухайте ▶, щоб упізнати голос, і впишіть ім'я — у наступних записах воно підпишеться само. Усе залишається на цьому Mac.
          </DialogDescription>
        </DialogHeader>

        {!available && (
          <div className="text-sm text-gray-700 space-y-2">
            <p>Для розрізнення голосів потрібні дві невеликі моделі (33 МБ), один раз. Далі все працює без інтернету.</p>
            <Button variant="outline" size="sm" onClick={downloadModels} disabled={downloading !== null}>
              {downloading !== null ? `Завантаження… ${downloading}%` : 'Завантажити моделі'}
            </Button>
          </div>
        )}

        {running ? (
          <div className="text-sm text-gray-700 py-4 space-y-1">
            <div className="flex items-center gap-2">
              <Loader2 className="animate-spin" size={16} />
              <span>{(stage && STAGES[stage]) || 'Обробка…'} {progress > 0 ? `${progress}%` : ''}</span>
            </div>
            <p className="text-xs text-gray-500">Можна закрити це вікно — розпізнавання йде далі, хід видно над стенограмою.</p>
            <Button variant="outline" size="sm" onClick={() => invoke('stop_speaker_recognition', { meetingId }).catch(() => {})}>
              Зупинити
            </Button>
          </div>
        ) : speakers.length === 0 ? (
          <p className="text-sm text-gray-600 py-2">Спікерів ще не розпізнано. Це займає кілька хвилин на годину запису.</p>
        ) : (
          <div className="space-y-3 max-h-[55vh] overflow-y-auto py-1 pr-1">
            {others.length > 0 && (
              <div className="rounded-md bg-[var(--selected)] p-2 space-y-1.5">
                <p className="text-xs font-medium text-gray-700">Можливо, інший голос: тут одна доповідь закінчилась і почалась нова</p>
                {others.map((o) => (
                  <div key={`${o.speaker}:${o.fromSeconds}`} className="flex items-start gap-2 text-sm">
                    <button type="button" className="text-xs tabular-nums text-[var(--blue)] hover:underline pt-0.5" onClick={() => onJumpTo?.(o.fromSeconds)}>
                      {mmss(o.fromSeconds)}
                    </button>
                    <span className="flex-1 min-w-0">
                      <span className="font-semibold">{o.speaker}</span>
                      <span className="text-gray-600"> — «{o.preview}…»</span>
                    </span>
                    <Button size="sm" variant="outline" disabled={running} onClick={() => splitOff(o)}>
                      Відокремити
                    </Button>
                  </div>
                ))}
              </div>
            )}
            {pairs.length > 0 && (
              <div className="rounded-md bg-[var(--blue-tint)] p-2 space-y-1.5">
                <p className="text-xs font-medium text-gray-700">Схоже, це одна людина — послухайте й об'єднайте:</p>
                {pairs.map((p) => (
                  <div key={pairKey(p)} className="flex items-center gap-2 text-sm">
                    <span className="flex-1 min-w-0 truncate">«{p.keep}» і «{p.merge}»</span>
                    <Button size="sm" variant="outline" disabled={running} onClick={() => mergePair(p)}>Об'єднати</Button>
                    <Button size="sm" variant="ghost" onClick={() => dismissPair(p)}>Ні</Button>
                  </div>
                ))}
              </div>
            )}
            {speakers.map((s) => (
              <div key={s.label} className="rounded-md border border-gray-200 p-2">
                <div className="flex items-center gap-2">
                  {merging && (
                    <input
                      type="checkbox"
                      aria-label={`Вибрати ${shown(s)} для об'єднання`}
                      checked={selected.includes(s.label)}
                      onChange={() => toggle(s.label)}
                    />
                  )}
                  <button
                    type="button"
                    className="shrink-0 rounded-full border border-gray-300 p-1 hover:bg-gray-100 disabled:opacity-40"
                    onClick={() => playClip(s)}
                    disabled={s.clip_start == null}
                    title="Послухати цей голос"
                  >
                    {playing === s.label ? <Pause size={14} /> : <Play size={14} />}
                  </button>
                  <button
                    type="button"
                    className="shrink-0 rounded-full border border-gray-300 p-1 hover:bg-gray-100"
                    aria-label={`Копіювати весь текст: ${s.name ?? s.label}`}
                    title={`Копіювати весь текст: ${s.name ?? s.label} · з ⌥ — із часом`}
                    onClick={(e) => copySpeakerText(lines, s.name ?? s.label, e.altKey)}
                  >
                    <Copy size={14} />
                  </button>
                  <span className="w-20 shrink-0 text-sm text-gray-600">{s.label}</span>
                  <input
                    className="flex-1 min-w-0 px-2 py-1 text-sm border border-gray-300 rounded-md"
                    placeholder="Ім'я (можна пізніше)"
                    value={names[s.label] ?? ''}
                    onChange={(e) => setNames((n) => ({ ...n, [s.label]: e.target.value }))}
                  />
                  <input
                    className="w-36 shrink-0 px-2 py-1 text-sm border border-gray-300 rounded-md"
                    placeholder="Роль: керівник…"
                    title="Необов'язково. Типи записів можуть ділити розмову на частини за ролями (напр. «керівник», «доповідач»)"
                    value={roles[s.label] ?? ''}
                    onChange={(e) => setRoles((r) => ({ ...r, [s.label]: e.target.value }))}
                  />
                </div>
                {!s.name && s.suggested && (
                  <div className="mt-1 pl-[4.5rem] flex items-center gap-2 text-xs">
                    <span className="text-gray-700">
                      Можливо, це <strong>{s.suggested}</strong> — так звучало в записі.
                    </span>
                    <button type="button" className="rounded border border-gray-300 px-2 py-0.5 hover:bg-gray-100" onClick={() => confirmSuggestion(s)}>
                      Так
                    </button>
                    <button type="button" className="rounded border border-gray-300 px-2 py-0.5 hover:bg-gray-100" onClick={() => rejectSuggestion(s)}>
                      Ні
                    </button>
                  </div>
                )}
                <div className="mt-1 pl-[4.5rem] text-xs text-gray-500 flex flex-wrap gap-x-3">
                  <span>мовлення {duration(s.speech_seconds)}</span>
                  {s.first_seconds != null && (
                    <button
                      type="button"
                      className="text-blue-600 hover:underline"
                      onClick={() => onJumpTo?.(s.first_seconds!)}
                      title="Показати в транскрипті й увімкнути запис з цього місця"
                    >
                      вперше {mmss(s.first_seconds)} →
                    </button>
                  )}
                </div>
                {s.samples.length > 0 && (
                  <ul className="mt-1 pl-[4.5rem] text-xs text-gray-700 space-y-0.5">
                    {s.samples.map((line, i) => (
                      <li key={i} className="italic">«{line}»</li>
                    ))}
                  </ul>
                )}
              </div>
            ))}
          </div>
        )}

        <DialogFooter className="gap-2 sm:justify-between">
          <div className="flex gap-2">
            <Button variant="outline" onClick={identify} disabled={running || !available}>
              {speakers.length === 0 ? 'Розпізнати спікерів' : 'Розпізнати заново'}
            </Button>
            {speakers.length > 1 && (
              merging ? (
                <>
                  <Button variant="outline" onClick={async () => { await merge(); setMerging(false); }} disabled={running || selected.length < 2}>
                    Об'єднати вибрані ({selected.length})
                  </Button>
                  <Button variant="ghost" onClick={() => { setMerging(false); setSelected([]); }}>
                    Скасувати
                  </Button>
                </>
              ) : (
                <Button
                  variant="outline"
                  onClick={() => setMerging(true)}
                  disabled={running}
                  title="Якщо одну людину розбило на кілька голосів: позначте їх і об'єднайте"
                >
                  Об'єднати голоси…
                </Button>
              )
            )}
          </div>
          <Button onClick={speakers.length > 0 && !running ? save : () => onOpenChange(false)}>
            {speakers.length > 0 && !running ? 'Зберегти' : 'Закрити'}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
