'use client';

import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { convertFileSrc, invoke } from '@tauri-apps/api/core';
import { Check, Pause, Play, Search } from 'lucide-react';
import { toast } from '@/lib/notice';
import { confirmNative } from '@/lib/confirm';
import { plural } from '@/lib/plural';
import { speakerColorMap } from '@/lib/speakerColors';
import { Switch } from '@/components/ui/switch';

interface SavedSpeaker {
  name: string;
  role: string | null;
  hasVoice: boolean;
  recognize: boolean;
  suggest: boolean;
  voiceMeetings: number;
  meetings: number;
  lastSeen: string | null;
}
interface SamePerson {
  keep: string;
  merge: string;
}
interface VoiceSample {
  meetingId: string;
  title: string;
  date: string;
  audioPath: string | null;
  clipStart: number | null;
  clipEnd: number | null;
  excluded: boolean;
}

type Filter = 'all' | 'voice' | 'name' | 'extra';
const FILTERS: { value: Filter; label: string; hint: string }[] = [
  { value: 'all', label: 'Усі', hint: 'Усі збережені люди' },
  { value: 'voice', label: 'З голосом', hint: 'Люди, яких програма впізнає за голосом' },
  { value: 'name', label: "Лише ім'я", hint: "Імена без збереженого голосу: лише пропонуються в підказках" },
  { value: 'extra', label: 'Можливо, зайві', hint: 'Імена, що трапились лише в одному записі: часто одруківки або випадкові слова' },
];

/** Only in one recording: often a typo or a stray word given as a name */
const maybeExtra = (p: SavedSpeaker) => p.meetings <= 1;
const day = (iso?: string | null) => (iso ? new Date(iso).toLocaleDateString('uk-UA', { day: '2-digit', month: '2-digit' }) : '');
const initials = (name: string) =>
  name
    .split(/\s+/)
    .filter(Boolean)
    .slice(0, 2)
    .map((w) => w[0]?.toUpperCase() ?? '')
    .join('');

const DISMISSED = 'saved-speakers-not-same';
const readDismissed = (): string[] => {
  try {
    return JSON.parse(localStorage.getItem(DISMISSED) ?? '[]');
  } catch {
    return [];
  }
};
const pairKey = (p: SamePerson) => [p.keep, p.merge].sort().join('|');

function meta(p: SavedSpeaker): string {
  const head = p.hasVoice ? 'Голос' : "Лише ім'я";
  const n = Math.max(p.meetings, p.voiceMeetings);
  const where = n > 0 ? `${n} ${plural(n, 'запис', 'записи', 'записів')}` : '';
  return [head, where, day(p.lastSeen)].filter(Boolean).join(' · ');
}

/**
 * «Спікери → Усі збережені»: every person the app knows by voice or by name.
 * A voice can stop naming anyone, a name can stop being offered, one
 * recording can be left out of a voice, two entries of one person joined,
 * people forgotten (with «Скасувати»). Recordings already made keep their names.
 */
/** The bottom bar of «Спікери»: the same height and place in both tabs */
export function SpeakersFooter({ children, onDone }: { children?: React.ReactNode; onDone?: () => void }) {
  return (
    <div className="flex h-14 flex-none items-center gap-2">
      {children}
      <span className="flex-1" />
      {onDone && (
        <button type="button" onClick={onDone} className="rounded-full bg-[#1d1d1f] px-5 py-2 text-sm font-semibold text-white dark:bg-[#f2f2f7] dark:text-[#1d1d1f]">
          Готово
        </button>
      )}
    </div>
  );
}

/** The area under the header, the same size in both tabs */
export const SPEAKERS_BODY = 'flex h-[min(520px,58vh)] min-h-0 overflow-hidden rounded-2xl border border-[var(--hairline)] bg-[var(--selected-soft,rgba(127,127,127,0.04))]';

export function SavedSpeakers({ onChanged, onDone, focus }: { onChanged?: () => void; onDone?: () => void; focus?: string | null }) {
  const [people, setPeople] = useState<SavedSpeaker[]>([]);
  const [same, setSame] = useState<SamePerson[]>([]);
  const [dismissed, setDismissed] = useState<string[]>(readDismissed);
  const [filter, setFilter] = useState<Filter>('all');
  const [query, setQuery] = useState('');
  const [chosen, setChosen] = useState<string[]>([]);
  const [current, setCurrent] = useState<string | null>(focus ?? null);
  const [samples, setSamples] = useState<VoiceSample[]>([]);
  const [nameDraft, setNameDraft] = useState('');
  const [roleDraft, setRoleDraft] = useState('');
  const [mergeOpen, setMergeOpen] = useState(false);
  const [playing, setPlaying] = useState<string | null>(null);
  const audio = useRef<HTMLAudioElement | null>(null);
  const listRef = useRef<HTMLDivElement | null>(null);
  // A person opened from another place is scrolled into view once listed
  const focused = useRef(false);
  useEffect(() => {
    if (!focus || focused.current || !people.some((p) => p.name === focus)) return;
    focused.current = true;
    const row = Array.from(listRef.current?.querySelectorAll<HTMLElement>('[data-person]') ?? []).find((el) => el.dataset.person === focus);
    row?.scrollIntoView({ block: 'center' });
  }, [focus, people]);

  const load = useCallback(async () => {
    try {
      const s = await invoke<{ people: SavedSpeaker[]; same: SamePerson[] }>('get_saved_speakers');
      setPeople(s.people);
      setSame(s.same);
      setChosen((c) => c.filter((n) => s.people.some((p) => p.name === n)));
      setCurrent((c) => (c && s.people.some((p) => p.name === c) ? c : s.people[0]?.name ?? null));
    } catch (e) {
      toast.error(`Не вдалося прочитати список: ${e}`);
    }
  }, []);
  useEffect(() => {
    load();
    return () => audio.current?.pause();
  }, [load]);

  const person = people.find((p) => p.name === current) ?? null;
  useEffect(() => {
    setNameDraft(person?.name ?? '');
    setRoleDraft(person?.role ?? '');
    setMergeOpen(false);
    setSamples([]);
    if (person?.hasVoice || (person && person.meetings > 0)) {
      invoke<VoiceSample[]>('get_saved_speaker_samples', { name: person.name }).then(setSamples).catch(() => setSamples([]));
    }
  }, [person?.name, person?.hasVoice]);

  const colors = useMemo(() => speakerColorMap(people.map((p) => p.name)), [people]);
  const q = query.trim().toLowerCase();
  const shown = people.filter(
    (p) =>
      (!q || p.name.toLowerCase().includes(q) || (p.role ?? '').toLowerCase().includes(q)) &&
      (filter === 'all' || (filter === 'voice' && p.hasVoice) || (filter === 'name' && !p.hasVoice) || (filter === 'extra' && maybeExtra(p))),
  );
  const pairs = same.filter((p) => !dismissed.includes(pairKey(p)));

  const changed = async () => {
    await load();
    onChanged?.();
  };

  // A few seconds of the voice from one of its recordings
  const play = async (key: string, s: VoiceSample | undefined) => {
    audio.current?.pause();
    if (playing === key) {
      setPlaying(null);
      return;
    }
    if (!s?.audioPath || s.clipStart == null) {
      toast.info('Звук цього запису не знайдено');
      return;
    }
    const a = new Audio(convertFileSrc(s.audioPath));
    audio.current = a;
    const end = s.clipEnd ?? s.clipStart + 8;
    a.addEventListener('loadedmetadata', () => {
      a.currentTime = s.clipStart ?? 0;
      a.play().catch(() => setPlaying(null));
    });
    a.addEventListener('timeupdate', () => {
      if (a.currentTime >= end) {
        a.pause();
        setPlaying((p) => (p === key ? null : p));
      }
    });
    a.addEventListener('error', () => {
      setPlaying(null);
      toast.info('Звук цього запису не знайдено');
    });
    setPlaying(key);
  };
  const playPerson = async (p: SavedSpeaker) => {
    if (playing === `row:${p.name}`) return play(`row:${p.name}`, undefined);
    const list = p.name === current && samples.length ? samples : await invoke<VoiceSample[]>('get_saved_speaker_samples', { name: p.name }).catch(() => []);
    await play(`row:${p.name}`, list.find((s) => !s.excluded && s.audioPath && s.clipStart != null));
  };

  const set = async (args: { recognize?: boolean; suggest?: boolean; role?: string }) => {
    if (!person) return;
    try {
      await invoke('set_saved_speaker', { name: person.name, recognize: args.recognize ?? null, suggest: args.suggest ?? null, role: args.role ?? null });
      await changed();
    } catch (e) {
      toast.error(`Не вдалося зберегти: ${e}`);
    }
  };

  const mergeInto = async (keep: string, merge: string) => {
    try {
      await invoke('merge_saved_speakers', { keep, merge });
      setCurrent(keep);
      await changed();
      toast.success(`Об'єднано в «${keep}»`);
    } catch (e) {
      toast.error(`Не вдалося об'єднати: ${e}`);
    }
  };

  const rename = async () => {
    const wanted = nameDraft.trim().replace(/\s+/g, ' ');
    if (!person || !wanted || wanted === person.name) {
      setNameDraft(person?.name ?? '');
      return;
    }
    await mergeInto(wanted, person.name);
  };

  const exclude = async (s: VoiceSample) => {
    if (!person) return;
    try {
      await invoke('set_voice_sample_excluded', { name: person.name, meetingId: s.meetingId, excluded: !s.excluded });
      setSamples(await invoke<VoiceSample[]>('get_saved_speaker_samples', { name: person.name }));
      await changed();
    } catch (e) {
      toast.error(`Не вдалося зберегти: ${e}`);
    }
  };

  const forget = async (names: string[]) => {
    if (names.length === 0) return;
    const title = names.length === 1 ? `Забути «${names[0]}»?` : `Забути ${names.length} ${plural(names.length, 'людину', 'людей', 'людей')}?`;
    const list = names.length > 1 ? `${names.map((n) => `«${n}»`).join(', ')}\n\n` : '';
    const ok = await confirmNative(
      title,
      `${list}Голоси більше не впізнаються в нових записах, імена не пропонуються в підказках. Наявні стенограми та документи залишаються без змін.`,
      'Забути',
    );
    if (!ok) return;
    try {
      audio.current?.pause();
      const batch = await invoke<string>('forget_saved_speakers', { names });
      setChosen([]);
      await changed();
      toast.success(`Прибрано з пам'яті: ${names.length === 1 ? `«${names[0]}»` : `${names.length} ${plural(names.length, 'людина', 'людини', 'людей')}`}`, {
        duration: 10000,
        action: {
          label: 'Скасувати',
          onClick: async () => {
            try {
              await invoke('restore_forgotten_speakers', { batch });
              await changed();
              toast.success('Повернуто');
            } catch (e) {
              toast.error(`Не вдалося повернути: ${e}`);
            }
          },
        },
      });
    } catch (e) {
      toast.error(`Не вдалося забути: ${e}`);
    }
  };

  const toggleChosen = (name: string) => setChosen((c) => (c.includes(name) ? c.filter((n) => n !== name) : [...c, name]));

  if (people.length === 0) {
    return (
      <>
        <div className={`${SPEAKERS_BODY} items-center justify-center`}>
          <p className="max-w-sm text-center text-sm text-[color:var(--muted-text)]">
            Збережених людей поки немає. Імена з&apos;являться тут, щойно у стенограмі підписати голос.
          </p>
        </div>
        <SpeakersFooter onDone={onDone} />
      </>
    );
  }

  return (
    <>
    <div className={SPEAKERS_BODY}>
      {/* The list */}
      <div className="flex w-[340px] flex-none flex-col border-r border-[var(--hairline)]">
        <div className="flex flex-col gap-2 p-3 pb-2">
          <label className="flex h-8 items-center gap-1.5 rounded-lg bg-[var(--selected)] px-2">
            <Search className="h-3.5 w-3.5 flex-none text-[color:var(--muted-text)]" />
            <input
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder="Пошук за іменем або роллю"
              aria-label="Пошук людини"
              className="min-w-0 flex-1 bg-transparent text-sm outline-none placeholder:text-[color:var(--muted-text)]"
            />
          </label>
          <div className="flex h-[24px] flex-nowrap items-center gap-1" role="radiogroup" aria-label="Кого показати">
            {FILTERS.map((f) => (
              <button
                key={f.value}
                type="button"
                role="radio"
                aria-checked={filter === f.value}
                title={f.hint}
                onClick={() => setFilter(f.value)}
                className={`flex-none whitespace-nowrap rounded-full px-2 py-1 text-xs ${filter === f.value ? 'bg-[var(--blue)] font-medium text-white' : 'bg-[var(--selected)]'}`}
              >
                {f.label}
              </button>
            ))}
          </div>
        </div>

        {pairs.slice(0, 1).map((p) => (
          <div key={pairKey(p)} className="mx-3 mb-2 flex items-center gap-2 rounded-lg bg-[var(--blue-tint)] px-2.5 py-2 text-xs">
            <span className="min-w-0 flex-1">
              Схоже, одна людина: <b>«{p.keep}»</b> і <b>«{p.merge}»</b>
            </span>
            <button type="button" className="rounded-full bg-[var(--card-bg)] px-2.5 py-1 font-medium" onClick={() => mergeInto(p.keep, p.merge)}>
              Об&apos;єднати
            </button>
            <button
              type="button"
              className="px-1 text-[color:var(--muted-text)]"
              onClick={() => {
                const next = [...dismissed, pairKey(p)];
                setDismissed(next);
                try {
                  localStorage.setItem(DISMISSED, JSON.stringify(next));
                } catch {}
              }}
            >
              Ні
            </button>
          </div>
        ))}

        <div ref={listRef} className="min-h-0 flex-1 overflow-y-auto px-1.5 pb-1.5">
          {shown.length === 0 && <p className="px-2 py-3 text-sm text-[color:var(--muted-text)]">Нікого не знайдено</p>}
          {shown.map((p) => (
            <div
              key={p.name}
              data-person={p.name}
              className={`flex items-center gap-2 rounded-lg px-2 py-1.5 ${p.name === current ? 'bg-[var(--blue-tint)]' : 'hover:bg-[var(--selected)]'}`}
            >
              <input
                type="checkbox"
                aria-label={`Вибрати «${p.name}»`}
                checked={chosen.includes(p.name)}
                onChange={() => toggleChosen(p.name)}
                className="h-3.5 w-3.5 flex-none accent-[var(--blue)]"
              />
              <button type="button" onClick={() => setCurrent(p.name)} className="flex min-w-0 flex-1 items-center gap-2 text-left">
                <span
                  className="flex h-7 w-7 flex-none items-center justify-center rounded-full text-[11px] font-semibold text-white"
                  style={{ background: p.hasVoice ? colors.get(p.name) : '#a1a1a8' }}
                >
                  {initials(p.name)}
                </span>
                <span className="flex min-w-0 flex-col">
                  <span className="flex min-w-0 items-center gap-1.5">
                    <span className={`truncate text-sm font-medium ${p.suggest ? '' : 'text-[color:var(--muted-text)]'}`}>{p.name}</span>
                    {maybeExtra(p) && <span className="flex-none rounded bg-[rgba(255,149,0,0.16)] px-1.5 text-[10px] font-medium text-[#8a4b00] dark:text-[#ffb340]">зайвий?</span>}
                  </span>
                  <span className="truncate text-xs text-[color:var(--muted-text)]">{meta(p)}</span>
                </span>
              </button>
              {p.hasVoice && (
                <button
                  type="button"
                  aria-label={`Послухати голос: ${p.name}`}
                  title="Послухати голос"
                  onClick={() => playPerson(p)}
                  className="flex h-7 w-7 flex-none items-center justify-center rounded-full bg-[var(--selected)]"
                >
                  {playing === `row:${p.name}` ? <Pause className="h-3 w-3" /> : <Play className="h-3 w-3" />}
                </button>
              )}
            </div>
          ))}
        </div>

      </div>

      {/* The person */}
      {person && (
        <div className="flex min-w-0 flex-1 flex-col gap-4 overflow-y-auto p-5 [&>*]:shrink-0">
          <div className="flex items-center gap-3">
            <span
              className="flex h-12 w-12 flex-none items-center justify-center rounded-full text-base font-semibold text-white"
              style={{ background: person.hasVoice ? colors.get(person.name) : '#a1a1a8' }}
            >
              {initials(person.name)}
            </span>
            <div className="flex min-w-0 flex-1 flex-col gap-0.5">
              <input
                aria-label="Ім'я"
                title="Нове ім'я: для наступних записів"
                value={nameDraft}
                onChange={(e) => setNameDraft(e.target.value)}
                onBlur={rename}
                onKeyDown={(e) => {
                  if (e.key === 'Enter') (e.target as HTMLInputElement).blur();
                  if (e.key === 'Escape') setNameDraft(person.name);
                }}
                className="min-w-0 bg-transparent text-lg font-semibold outline-none"
              />
              <input
                aria-label="Роль"
                placeholder="Роль (необов'язково)"
                value={roleDraft}
                onChange={(e) => setRoleDraft(e.target.value)}
                onBlur={() => roleDraft.trim() !== (person.role ?? '') && set({ role: roleDraft.trim() })}
                onKeyDown={(e) => e.key === 'Enter' && (e.target as HTMLInputElement).blur()}
                className="min-w-0 bg-transparent text-sm text-[color:var(--muted-text)] outline-none placeholder:text-[color:var(--muted-text)]"
              />
            </div>
          </div>

          {samples.length > 0 && (
            <section className="flex flex-col gap-1.5">
              <h3 className="text-[11px] font-semibold uppercase tracking-wide text-[color:var(--muted-text)]">
                {person.hasVoice ? `Голос · зібрано з ${person.voiceMeetings} ${plural(person.voiceMeetings, 'запису', 'записів', 'записів')}` : 'Записи'}
              </h3>
              <div className="overflow-hidden rounded-xl bg-[var(--card-bg)] shadow-[inset_0_0_0_0.5px_var(--hairline)]">
                {samples.map((s) => (
                  <div key={s.meetingId} className="flex items-center gap-2.5 border-b border-[var(--hairline)] px-3 py-2 last:border-b-0">
                    <button
                      type="button"
                      aria-label={`Послухати: ${s.title}`}
                      disabled={!s.audioPath || s.clipStart == null}
                      onClick={() => play(s.meetingId, s)}
                      className="flex h-7 w-7 flex-none items-center justify-center rounded-full bg-[var(--selected)] disabled:opacity-40"
                    >
                      {playing === s.meetingId ? <Pause className="h-3 w-3" /> : <Play className="h-3 w-3" />}
                    </button>
                    <span className={`flex min-w-0 flex-1 flex-col ${s.excluded ? 'opacity-50' : ''}`}>
                      <span className="truncate text-sm">{s.title}</span>
                      <span className="text-xs text-[color:var(--muted-text)]">
                        {day(s.date)}
                        {s.excluded ? ' · не входить до голосу' : ''}
                      </span>
                    </span>
                    {person.hasVoice || s.excluded ? (
                      <button type="button" onClick={() => exclude(s)} className="flex-none rounded-full bg-[var(--selected)] px-2.5 py-1 text-xs">
                        {s.excluded ? 'Повернути' : 'Не цей голос'}
                      </button>
                    ) : null}
                  </div>
                ))}
              </div>
              {person.hasVoice && <span className="px-1 text-xs text-[color:var(--muted-text)]">«Не цей голос» прибирає запис із голосу людини, решта записів залишається.</span>}
            </section>
          )}

          <div className="overflow-hidden rounded-xl bg-[var(--card-bg)] shadow-[inset_0_0_0_0.5px_var(--hairline)]">
            <label className="flex items-center gap-3 border-b border-[var(--hairline)] px-3.5 py-2.5">
              <span className="flex flex-1 flex-col">
                <span className="text-sm">Впізнавати за голосом</span>
                <span className="text-xs text-[color:var(--muted-text)]">
                  {person.hasVoice ? "У нових записах ім'я ставиться автоматично" : "Голос з'явиться, коли людину підписати в записі"}
                </span>
              </span>
              <Switch checked={person.hasVoice && person.recognize} disabled={!person.hasVoice} onCheckedChange={(on) => set({ recognize: on })} aria-label="Впізнавати за голосом" />
            </label>
            <label className="flex items-center gap-3 px-3.5 py-2.5">
              <span className="flex flex-1 flex-col">
                <span className="text-sm">Пропонувати ім&apos;я</span>
                <span className="text-xs text-[color:var(--muted-text)]">У списку «Хто говорив?» і підказках</span>
              </span>
              <Switch checked={person.suggest} onCheckedChange={(on) => set({ suggest: on })} aria-label="Пропонувати ім'я" />
            </label>
          </div>

          <div className="flex flex-wrap items-center gap-2">
            <button type="button" className="rounded-full bg-[var(--selected)] px-3.5 py-1.5 text-sm font-medium" onClick={() => setMergeOpen((o) => !o)} aria-expanded={mergeOpen}>
              Об&apos;єднати з…
            </button>
            <span className="flex-1" />
            <button
              type="button"
              className="rounded-full bg-[rgba(255,59,48,0.12)] px-3.5 py-1.5 text-sm font-medium text-[#c4001a] dark:text-[#ff6961]"
              onClick={() => forget([person.name])}
            >
              Забути людину
            </button>
          </div>
          {mergeOpen && (
            <div className="flex flex-col gap-1 rounded-xl bg-[var(--selected)] p-2">
              <span className="px-1.5 pb-1 text-xs text-[color:var(--muted-text)]">«{person.name}» стане частиною людини:</span>
              <div className="max-h-[160px] overflow-y-auto">
                {people
                  .filter((p) => p.name !== person.name)
                  .map((p) => (
                    <button key={p.name} type="button" className="nav-row flex h-8 w-full items-center gap-2 px-2 text-left text-sm" onClick={() => mergeInto(p.name, person.name)}>
                      <Check className="h-3.5 w-3.5 flex-none opacity-0" />
                      <span className="truncate">{p.name}</span>
                    </button>
                  ))}
              </div>
            </div>
          )}
          <p className="text-xs text-[color:var(--muted-text)]">Наявні стенограми та документи залишаються без змін.</p>
        </div>
      )}
    </div>
    <SpeakersFooter onDone={onDone}>
      {chosen.length > 0 ? (
        <>
          <span className="whitespace-nowrap text-sm font-semibold">Вибрано {chosen.length}</span>
          <button type="button" className="whitespace-nowrap rounded-full bg-[var(--selected)] px-3.5 py-2 text-sm font-medium" onClick={() => setChosen([])}>
            Зняти
          </button>
          <button type="button" className="whitespace-nowrap rounded-full bg-[rgba(255,59,48,0.12)] px-3.5 py-2 text-sm font-medium text-[#c4001a] dark:text-[#ff6961]" onClick={() => forget(chosen)}>
            Забути вибраних
          </button>
        </>
      ) : (
        <span className="text-xs text-[color:var(--muted-text)]">Зміни зберігаються одразу. Позначте кількох людей, щоб забути їх разом.</span>
      )}
    </SpeakersFooter>
    </>
  );
}
