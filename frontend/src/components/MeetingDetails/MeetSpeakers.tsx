"use client";

/**
 * Google Meet in «Спікери»: what the Meet window gave this recording, where
 * each name came from, and the Meet account behind each voice (link, change,
 * unlink). Links between accounts and people are kept in memory: later
 * meetings name a linked account at once.
 */

import React, { useEffect, useMemo, useRef, useState } from 'react';
import { convertFileSrc, invoke } from '@tauri-apps/api/core';
import { Pause, Play, Search } from 'lucide-react';
import { toast } from '@/lib/notice';
import { plural } from '@/lib/plural';
import { Dialog, DialogContent, DialogDescription, DialogTitle } from '../ui/dialog';
import type { LinkCandidate, MeetAccount, MeetStatus, NameSource, VoiceMatch } from '@/lib/meet';

export interface MeetSpeakerInfo {
  label: string;
  name: string | null;
  suggested: string | null;
  speech_seconds: number;
  first_seconds: number | null;
  clip_start: number | null;
  clip_end: number | null;
  meet_name: string | null;
  name_source: NameSource | null;
}

export interface PersonLite {
  name: string;
  hasVoice: boolean;
  voiceMeetings: number;
}

interface VoiceSample {
  audioPath: string | null;
  clipStart: number | null;
  clipEnd: number | null;
  excluded: boolean;
}

export const AMBER_NOTE = 'bg-[rgba(255,159,10,0.14)] text-[#9a5a00] dark:text-[#ffcf8a]';
const BTN = 'whitespace-nowrap rounded-full bg-[var(--selected)] px-3.5 py-2 text-sm font-medium disabled:opacity-40';
const SMALL_BTN = 'whitespace-nowrap rounded-full bg-[var(--card-bg)] px-2.5 py-1 text-xs font-medium';
const SECTION = 'text-[11px] font-semibold uppercase tracking-wide text-[color:var(--muted-text)]';
const CARD = 'rounded-xl bg-[var(--card-bg)] shadow-[inset_0_0_0_0.5px_var(--hairline)]';

const LEVEL_TEXT: Record<VoiceMatch['level'], string> = { high: 'висока схожість', medium: 'середня схожість', low: 'низька схожість' };

function hms(seconds: number): string {
  const s = Math.max(0, Math.floor(seconds));
  return [Math.floor(s / 3600), Math.floor((s % 3600) / 60), s % 60].map((v) => String(v).padStart(2, '0')).join(':');
}

function minutes(seconds: number): string {
  return seconds < 60 ? `${Math.round(seconds)} с` : `${Math.round(seconds / 60)} хв`;
}

function day(iso: string | null): string {
  return iso ? new Date(iso).toLocaleDateString('uk-UA', { day: 'numeric', month: 'long' }) : '';
}

/** The short tag under a voice in the list: where its name came from */
export function sourceTag(s: MeetSpeakerInfo): string | null {
  switch (s.name_source) {
    case 'voice':
      return 'голос збігся';
    case 'meet_link':
      return "прив'язано до Meet";
    case 'meet_link_doubt':
      return 'голос не схожий';
    case 'meet':
    case 'meet_same_name':
      return "ім'я з Google Meet";
    default:
      return null;
  }
}

/** «· Meet: Андрій Бондар» unless the account's name is the shown name */
export function meetTag(s: MeetSpeakerInfo): string | null {
  if (!s.meet_name) return null;
  const shown = s.name ?? s.suggested;
  return shown && shown.toLowerCase() === s.meet_name.toLowerCase() ? null : `Meet: ${s.meet_name}`;
}

// ---------------------------------------------------------------------------
// Notices at the top of the voice list
// ---------------------------------------------------------------------------

const laterKey = (meetingId: string) => `meet-link-later:${meetingId}`;

export function MeetNotices({
  meetingId,
  status,
  candidates,
  onLink,
}: {
  meetingId: string;
  status: MeetStatus | null;
  candidates: LinkCandidate[];
  onLink: () => void;
}) {
  const [later, setLater] = useState(false);
  useEffect(() => {
    try {
      setLater(localStorage.getItem(laterKey(meetingId)) === '1');
    } catch {
      setLater(false);
    }
  }, [meetingId]);

  const open = candidates.filter((c) => c.reason !== 'shared');
  const found = open.filter((c) => c.person && c.checked).length;
  if (!status && open.length === 0) return null;

  return (
    <div className="mx-3 mb-2 flex flex-col gap-1.5 text-xs leading-snug">
      {status?.used && (
        <div className="rounded-lg bg-[var(--blue-tint)] px-2.5 py-2">
          Репліки розділено за учасниками Google Meet: {status.accounts} {plural(status.accounts, 'обліковий запис', 'облікові записи', 'облікових записів')}.
        </div>
      )}
      {status?.used && status.opened_at_s != null && (
        <div className={`rounded-lg px-2.5 py-2 ${AMBER_NOTE}`}>
          Вікно Google Meet відкрили о {hms(status.opened_at_s)}. До цього спікерів розпізнано за голосом.
        </div>
      )}
      {status?.mismatch && (
        <div className="rounded-lg bg-[var(--selected)] px-2.5 py-2 text-[color:var(--muted-text)]">
          Дані Google Meet не збіглися зі звуком запису, тому спікерів розпізнано за голосом.
        </div>
      )}
      {open.length > 0 && !later && (
        <div className="flex items-center gap-2 rounded-lg bg-[var(--blue-tint)] px-2.5 py-2">
          <span className="min-w-0 flex-1">
            Облікові записи Google Meet, ще не прив&apos;язані до людей у пам&apos;яті: {open.length}.
            {found > 0 ? ` Для ${found} знайдено збіг.` : ''}
          </span>
          <button type="button" className={SMALL_BTN} onClick={onLink}>
            Прив&apos;язати…
          </button>
          <button
            type="button"
            className="px-1 text-[color:var(--muted-text)]"
            onClick={() => {
              try {
                localStorage.setItem(laterKey(meetingId), '1');
              } catch {}
              setLater(true);
            }}
          >
            Пізніше
          </button>
        </div>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// A saved person's voice, to compare with the meeting's
// ---------------------------------------------------------------------------

export function useSavedVoicePlayer() {
  const audio = useRef<HTMLAudioElement | null>(null);
  const [playing, setPlaying] = useState<string | null>(null);
  useEffect(() => () => audio.current?.pause(), []);

  const play = async (name: string) => {
    audio.current?.pause();
    if (playing === name) {
      setPlaying(null);
      return;
    }
    const list = await invoke<VoiceSample[]>('get_saved_speaker_samples', { name }).catch(() => [] as VoiceSample[]);
    const s = list.find((x) => !x.excluded && x.audioPath && x.clipStart != null);
    if (!s?.audioPath || s.clipStart == null) {
      toast.info('Звук збереженого голосу не знайдено');
      return;
    }
    const a = new Audio(convertFileSrc(s.audioPath));
    audio.current = a;
    const start = s.clipStart;
    const end = s.clipEnd ?? start + 8;
    a.addEventListener('loadedmetadata', () => {
      a.currentTime = start;
      a.play().catch(() => setPlaying(null));
    });
    a.addEventListener('timeupdate', () => {
      if (a.currentTime >= end) {
        a.pause();
        setPlaying((p) => (p === name ? null : p));
      }
    });
    a.addEventListener('error', () => {
      setPlaying(null);
      toast.info('Звук збереженого голосу не знайдено');
    });
    setPlaying(name);
  };
  return { playing, play };
}

// ---------------------------------------------------------------------------
// Choosing a person: saved people, most similar voice first
// ---------------------------------------------------------------------------

export function PersonPicker({
  people,
  matches,
  taken,
  current,
  firstName,
  onPick,
  onNew,
  pickLabel = 'Це',
}: {
  people: PersonLite[];
  matches: VoiceMatch[];
  /** Names already given to other voices of this meeting */
  taken: string[];
  /** The person linked now */
  current?: string | null;
  /** A person to put first (the same name as the Meet account) */
  firstName?: string | null;
  onPick: (name: string) => void;
  onNew?: () => void;
  pickLabel?: string;
}) {
  const [query, setQuery] = useState('');
  const [chosen, setChosen] = useState<string | null>(null);
  const sim = useMemo(() => new Map(matches.map((m) => [m.name, m])), [matches]);
  const sorted = useMemo(() => {
    const first = firstName?.toLowerCase();
    return [...people].sort((a, b) => {
      const fa = a.name.toLowerCase() === first ? 1 : 0;
      const fb = b.name.toLowerCase() === first ? 1 : 0;
      if (fa !== fb) return fb - fa;
      const sa = sim.get(a.name)?.similarity ?? -2;
      const sb = sim.get(b.name)?.similarity ?? -2;
      if (sa !== sb) return sb - sa;
      return a.name.localeCompare(b.name, 'uk');
    });
  }, [people, sim, firstName]);
  const q = query.trim().toLowerCase();
  const list = sorted.filter((p) => !q || p.name.toLowerCase().includes(q));

  return (
    <div className="flex flex-col gap-1.5 rounded-xl bg-[var(--selected)] p-2">
      <label className="flex h-8 items-center gap-1.5 rounded-lg bg-[var(--card-bg)] px-2">
        <Search className="h-3.5 w-3.5 flex-none text-[color:var(--muted-text)]" />
        <input
          autoFocus
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder={`Пошук серед ${people.length} ${plural(people.length, 'людини', 'людей', 'людей')}`}
          aria-label="Пошук людини"
          className="min-w-0 flex-1 bg-transparent text-sm outline-none placeholder:text-[color:var(--muted-text)]"
        />
      </label>
      <div className="max-h-[180px] overflow-y-auto">
        {list.map((p) => {
          const m = sim.get(p.name);
          const tag =
            p.name === current
              ? "прив'язано зараз"
              : taken.includes(p.name)
                ? 'уже на цій нараді'
                : m && m.level !== 'low'
                  ? 'голос схожий'
                  : null;
          return (
            <button
              key={p.name}
              type="button"
              onClick={() => setChosen(p.name)}
              onDoubleClick={() => onPick(p.name)}
              className={`flex h-8 w-full items-center gap-2 rounded-md px-2 text-left text-sm ${chosen === p.name ? 'bg-[var(--blue)] text-white' : 'hover:bg-[var(--card-bg)]'}`}
            >
              <span className="min-w-0 flex-1 truncate">{p.name}</span>
              {tag && <span className={`flex-none text-xs ${chosen === p.name ? 'text-white/80' : 'text-[color:var(--muted-text)]'}`}>{tag}</span>}
            </button>
          );
        })}
        {list.length === 0 && <p className="px-2 py-1.5 text-xs text-[color:var(--muted-text)]">Нікого не знайдено</p>}
      </div>
      <div className="flex items-center gap-2 pt-0.5">
        <button type="button" className={`${SMALL_BTN} !bg-[var(--blue)] text-white disabled:opacity-40`} disabled={!chosen} onClick={() => chosen && onPick(chosen)}>
          {chosen ? `${pickLabel} ${chosen}` : 'Оберіть людину'}
        </button>
        {onNew && (
          <button type="button" className={SMALL_BTN} onClick={onNew}>
            Нова людина…
          </button>
        )}
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// The voice and the Meet account in a speaker's card
// ---------------------------------------------------------------------------

export function MeetSpeakerCard({
  meetingId,
  speaker,
  people,
  taken,
  account,
  playing,
  onPlayClip,
  onRename,
  onNewPerson,
  onChanged,
}: {
  meetingId: string;
  speaker: MeetSpeakerInfo;
  people: PersonLite[];
  taken: string[];
  account: MeetAccount | null;
  playing: boolean;
  onPlayClip: () => void;
  /** Gives this voice a saved person's name (links its account too) */
  onRename: (name: string) => Promise<void>;
  /** Clears the name field for a new person's name */
  onNewPerson: () => void;
  onChanged: () => Promise<void>;
}) {
  const [matches, setMatches] = useState<VoiceMatch[]>([]);
  const [picker, setPicker] = useState<null | 'name' | 'link'>(null);
  const saved = useSavedVoicePlayer();

  useEffect(() => {
    setPicker(null);
    invoke<VoiceMatch[]>('get_speaker_voice_matches', { meetingId, label: speaker.label })
      .then(setMatches)
      .catch(() => setMatches([]));
  }, [meetingId, speaker.label, speaker.name]);

  const shownName = speaker.name ?? speaker.suggested;
  // The voice next to the person it is shown as, else the closest one
  const compared = (shownName && matches.find((m) => m.name === shownName)) || matches[0] || null;
  const person = people.find((p) => p.name === compared?.name);
  const linkedTo = account?.person ?? null;
  const meet = speaker.meet_name;

  const link = async (to: string) => {
    if (!meet) return;
    try {
      await invoke('link_meet_accounts', { meetingId, links: [{ meetName: meet, person: to }] });
      setPicker(null);
      await onChanged();
      toast.success(`Обліковий запис «${meet}» прив'язано до людини ${to}`);
    } catch (e) {
      toast.error(`Не вдалося прив'язати: ${e}`);
    }
  };

  const unlink = async () => {
    if (!meet) return;
    try {
      await invoke('set_meet_account', { meetName: meet, person: null });
      await onChanged();
      toast.success(`Обліковий запис «${meet}» відв'язано`);
    } catch (e) {
      toast.error(`Не вдалося відв'язати: ${e}`);
    }
  };

  const voiceSection = compared && (
    <section className="flex flex-col gap-1.5">
      <h3 className={SECTION}>Голос</h3>
      <div className={`${CARD} flex flex-col gap-2 px-3.5 py-2.5 text-sm`}>
        <span>
          Схожий на збережений голос людини <b>{compared.name}</b> · {LEVEL_TEXT[compared.level]}
        </span>
        {person?.hasVoice && (
          <span className="text-xs text-[color:var(--muted-text)]">
            Збережений голос зібрано з {person.voiceMeetings} {plural(person.voiceMeetings, 'запису', 'записів', 'записів')}. Щоб перевірити, послухайте обидва.
          </span>
        )}
        <div className="flex flex-wrap gap-2">
          <button type="button" className={`${BTN} flex items-center gap-1.5 !py-1.5`} onClick={onPlayClip} disabled={speaker.clip_start == null}>
            {playing ? <Pause className="h-3 w-3" /> : <Play className="h-3 w-3" />} Цей спікер на нараді
          </button>
          <button type="button" className={`${BTN} flex items-center gap-1.5 !py-1.5`} onClick={() => saved.play(compared.name)}>
            {saved.playing === compared.name ? <Pause className="h-3 w-3" /> : <Play className="h-3 w-3" />} Збережений голос: {compared.name}
          </button>
        </div>
      </div>
    </section>
  );

  const accountSection = meet && (
    <section className="flex flex-col gap-1.5">
      <h3 className={SECTION}>Обліковий запис Google Meet</h3>
      <div className={`${CARD} flex flex-col gap-2 px-3.5 py-2.5 text-sm`}>
        <span className="flex items-center gap-2">
          <b className="min-w-0 truncate">{meet}</b>
          <span className="flex-none text-xs text-[color:var(--muted-text)]">
            {account?.shared ? 'говорять різні люди' : linkedTo ? "прив'язаний" : "ще не прив'язаний"}
          </span>
        </span>
        {account?.shared ? (
          <span className="text-xs text-[color:var(--muted-text)]">
            З цього облікового запису говорили різні люди, тому він не прив&apos;язується до однієї людини. Репліки розділено за голосом.
          </span>
        ) : linkedTo ? (
          <>
            <span className="text-xs text-[color:var(--muted-text)]">
              Прив&apos;язано до людини {linkedTo}
              {account?.linkedAt ? ` з ${day(account.linkedAt)}` : ''}.
            </span>
            <div className="flex flex-wrap gap-2">
              <button type="button" className={`${BTN} !py-1.5`} onClick={unlink}>
                Відв&apos;язати
              </button>
              <button type="button" className={`${BTN} !py-1.5`} onClick={() => setPicker(picker === 'link' ? null : 'link')} aria-expanded={picker === 'link'}>
                Прив&apos;язати до іншої людини…
              </button>
            </div>
          </>
        ) : (
          <>
            <span className="text-xs text-[color:var(--muted-text)]">
              {speaker.name
                ? `Після прив'язки «${meet}» підписуватиметься як «${speaker.name}» одразу.`
                : "Після прив'язки цей обліковий запис підписуватиметься іменем людини з пам'яті одразу."}
            </span>
            <div className="flex flex-wrap gap-2">
              {speaker.name && (
                <button type="button" className={`${BTN} !py-1.5`} onClick={() => link(speaker.name!)}>
                  Прив&apos;язати до {speaker.name}
                </button>
              )}
              <button type="button" className={`${BTN} !py-1.5`} onClick={() => setPicker(picker === 'link' ? null : 'link')} aria-expanded={picker === 'link'}>
                {speaker.name ? 'До іншої людини…' : "Прив'язати до людини…"}
              </button>
            </div>
          </>
        )}
        {picker === 'link' && (
          <PersonPicker
            people={people}
            matches={matches}
            taken={taken}
            current={linkedTo}
            firstName={meet}
            onPick={(name) => void link(name)}
            pickLabel="Прив'язати до"
          />
        )}
        {picker === 'link' && (
          <span className="text-xs text-[color:var(--muted-text)]">
            Нова прив&apos;язка діє на цій нараді і на наступних. Документи попередніх нарад не змінюються.
          </span>
        )}
      </div>
    </section>
  );

  // A Meet name waiting for confirmation, or a linked account with an unlike voice
  const suggestion = !speaker.name && speaker.suggested && speaker.name_source && speaker.name_source !== 'voice' && (
    <div className="flex flex-col gap-2 rounded-xl bg-[var(--blue-tint)] px-3.5 py-2.5 text-sm">
      <span>
        {speaker.name_source === 'meet_link_doubt' ? (
          <>
            Обліковий запис «{meet}» прив&apos;язано до людини <b>{speaker.suggested}</b>, але голос на цій нараді не схожий на збережений.
          </>
        ) : (
          <>
            Можливо, це <b>{speaker.suggested}</b>: так підписано в Google Meet.
          </>
        )}
      </span>
      <div className="flex flex-wrap gap-2">
        <button type="button" className={SMALL_BTN} onClick={() => void onRename(speaker.suggested!)}>
          Так, це {speaker.suggested}
        </button>
        <button type="button" className={SMALL_BTN} onClick={() => setPicker(picker === 'name' ? null : 'name')} aria-expanded={picker === 'name'}>
          {speaker.name_source === 'meet_link_doubt' ? 'Це інша людина…' : "Або оберіть людину з пам'яті"}
        </button>
      </div>
    </div>
  );

  const notThis = speaker.name && speaker.name !== 'Я' && (
    <section className="flex flex-col gap-1.5">
      <h3 className={SECTION}>Якщо це не {speaker.name}</h3>
      <div className="flex flex-wrap gap-2">
        <button type="button" className={`${BTN} !py-1.5`} onClick={() => setPicker(picker === 'name' ? null : 'name')} aria-expanded={picker === 'name'}>
          Обрати іншу людину…
        </button>
        <button type="button" className={`${BTN} !py-1.5`} onClick={onNewPerson}>
          Нова людина…
        </button>
      </div>
      <span className="text-xs text-[color:var(--muted-text)]">
        Репліки отримають інше ім&apos;я. Голос із цієї наради не додасться до збереженого голосу людини {speaker.name}.
      </span>
    </section>
  );

  return (
    <>
      {suggestion}
      {notThis}
      {picker === 'name' && (
        <PersonPicker
          people={people}
          matches={matches}
          taken={taken}
          current={speaker.name}
          firstName={meet ?? speaker.suggested}
          onPick={(name) => {
            setPicker(null);
            void onRename(name);
          }}
          onNew={() => {
            setPicker(null);
            onNewPerson();
          }}
        />
      )}
      {picker === 'name' && meet && (
        <span className="text-xs text-[color:var(--muted-text)]">
          Після вибору обліковий запис Google Meet «{meet}» підписуватиметься цим іменем на наступних нарадах, а голос збережеться в пам&apos;яті. Змінити зв&apos;язку можна в Налаштування → Пам&apos;ять.
        </span>
      )}
      {voiceSection}
      {accountSection}
    </>
  );
}

// ---------------------------------------------------------------------------
// «Прив'язати облікові записи Google Meet до людей у пам'яті»
// ---------------------------------------------------------------------------

const REASON_TEXT: Record<LinkCandidate['reason'], string> = {
  voice: 'голос збігся',
  name: "ім'я і голос збіглися",
  maybe: 'голос схожий, перевірте',
  none: 'збігу немає',
  shared: 'говорять різні люди',
};

export function MeetLinkSheet({
  open,
  onOpenChange,
  meetingId,
  candidates,
  people,
  speakers,
  playingLabel,
  onPlay,
  onLinked,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  meetingId: string;
  candidates: LinkCandidate[];
  people: PersonLite[];
  speakers: MeetSpeakerInfo[];
  playingLabel: string | null;
  onPlay: (label: string) => void;
  onLinked: () => Promise<void>;
}) {
  const [rows, setRows] = useState<{ c: LinkCandidate; person: string; checked: boolean; order: string[] }[]>([]);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (!open) return;
    let alive = true;
    (async () => {
      const out = [];
      for (const c of candidates) {
        // People for this row: the same name first, then by voice
        const matches = c.reason === 'shared' ? [] : await invoke<VoiceMatch[]>('get_speaker_voice_matches', { meetingId, label: c.label }).catch(() => [] as VoiceMatch[]);
        const byVoice = matches.map((m) => m.name);
        const same = people.find((p) => p.name.toLowerCase() === c.meetName.toLowerCase())?.name;
        const rest = people.map((p) => p.name).filter((n) => !byVoice.includes(n)).sort((a, b) => a.localeCompare(b, 'uk'));
        const order = [...(same ? [same] : []), ...byVoice.filter((n) => n !== same), ...rest.filter((n) => n !== same)];
        out.push({ c, person: c.person ?? '', checked: c.checked && !!c.person, order });
      }
      if (alive) setRows(out);
    })();
    return () => {
      alive = false;
    };
  }, [open, candidates, people, meetingId]);

  const chosen = rows.filter((r) => r.checked && r.person && r.c.reason !== 'shared');
  const shared = rows.filter((r) => r.c.reason === 'shared');
  const sharedVoices = (meetName: string) =>
    speakers
      .filter((s) => s.meet_name === meetName && s.name)
      .map((s) => s.name!)
      .filter((n, i, a) => a.indexOf(n) === i);

  const linkAll = async () => {
    setBusy(true);
    try {
      const n = await invoke<number>('link_meet_accounts', { meetingId, links: chosen.map((r) => ({ meetName: r.c.meetName, person: r.person })) });
      await onLinked();
      onOpenChange(false);
      toast.success(`Прив'язано: ${n} ${plural(n, 'обліковий запис', 'облікові записи', 'облікових записів')}`);
    } catch (e) {
      toast.error(`Не вдалося прив'язати: ${e}`);
    } finally {
      setBusy(false);
    }
  };

  const update = (i: number, patch: Partial<{ person: string; checked: boolean }>) =>
    setRows((all) => all.map((r, j) => (j === i ? { ...r, ...patch, checked: patch.person !== undefined ? !!patch.person : patch.checked ?? r.checked } : r)));

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="flex w-[min(720px,calc(100vw-48px))] max-w-none flex-col gap-3 p-6 sm:max-w-none">
        <DialogTitle className="text-lg font-bold">Прив&apos;язати облікові записи Google Meet до людей у пам&apos;яті</DialogTitle>
        <DialogDescription className="text-[13px] text-[color:var(--muted-text)]">
          Прив&apos;язка робиться один раз. На наступних нарадах ці облікові записи підписуватимуться збереженими іменами одразу.
        </DialogDescription>
        <div className="overflow-hidden rounded-xl border border-[var(--hairline)]">
          <div className="grid grid-cols-[24px_minmax(0,1fr)_16px_minmax(0,1fr)_150px_28px] items-center gap-2 bg-[var(--selected)] px-3 py-1.5 text-[11px] font-semibold uppercase tracking-wide text-[color:var(--muted-text)]">
            <span />
            <span>Обліковий запис у Meet</span>
            <span />
            <span>Людина в пам&apos;яті</span>
            <span>Чому</span>
            <span />
          </div>
          <div className="max-h-[min(360px,45vh)] overflow-y-auto">
            {rows
              .map((r, i) => ({ r, i }))
              .filter(({ r }) => r.c.reason !== 'shared')
              .map(({ r, i }) => (
                <div key={r.c.meetName} className="grid grid-cols-[24px_minmax(0,1fr)_16px_minmax(0,1fr)_150px_28px] items-center gap-2 border-t border-[var(--hairline)] px-3 py-2 text-sm">
                  <input
                    type="checkbox"
                    aria-label={`Прив'язати «${r.c.meetName}»`}
                    checked={r.checked}
                    disabled={!r.person}
                    onChange={(e) => update(i, { checked: e.target.checked })}
                  />
                  <span className="flex min-w-0 flex-col">
                    <span className="truncate font-medium">{r.c.meetName}</span>
                    <span className="text-xs text-[color:var(--muted-text)]">{minutes(r.c.speechSeconds)} на нараді</span>
                  </span>
                  <span className="text-[color:var(--muted-text)]" aria-hidden="true">
                    →
                  </span>
                  <select
                    aria-label={`Людина для «${r.c.meetName}»`}
                    value={r.person}
                    onChange={(e) => update(i, { person: e.target.value })}
                    className="h-8 min-w-0 rounded-lg bg-[var(--selected)] px-2 text-sm outline-none"
                  >
                    <option value="">Оберіть людину</option>
                    {r.order.map((n) => (
                      <option key={n} value={n}>
                        {n}
                      </option>
                    ))}
                  </select>
                  <span className={`text-xs ${r.c.reason === 'maybe' ? 'text-[#9a5a00] dark:text-[#ffcf8a]' : 'text-[color:var(--muted-text)]'}`}>
                    {r.person === (r.c.person ?? '') ? REASON_TEXT[r.c.reason] : 'обрано вручну'}
                  </span>
                  <button
                    type="button"
                    aria-label={`Послухати «${r.c.meetName}»`}
                    onClick={() => onPlay(r.c.label)}
                    className="flex h-7 w-7 items-center justify-center rounded-full bg-[var(--selected)]"
                  >
                    {playingLabel === r.c.label ? <Pause className="h-3 w-3" /> : <Play className="h-3 w-3" />}
                  </button>
                </div>
              ))}
            {shared.map(({ c }) => {
              const who = sharedVoices(c.meetName);
              return (
                <div key={c.meetName} className="flex flex-col border-t border-[var(--hairline)] px-3 py-2 text-sm opacity-60">
                  <span className="font-medium">
                    {c.meetName} <span className="text-xs font-normal text-[color:var(--muted-text)]">{minutes(c.speechSeconds)} на нараді</span>
                  </span>
                  <span className="text-xs text-[color:var(--muted-text)]">
                    З нього говорили різні люди{who.length > 1 ? ` (${who.join(' і ')})` : ''}, тому він не прив&apos;язується до однієї людини.
                  </span>
                </div>
              );
            })}
          </div>
        </div>
        <p className="text-xs text-[color:var(--muted-text)]">
          Голос із цієї наради додається до збережених голосів людини, тож упізнавання стає точнішим. Ім&apos;я людини в пам&apos;яті не змінюється: ім&apos;я з Meet лишається лише підписом облікового запису.
        </p>
        <div className="flex items-center justify-end gap-2">
          <button type="button" className={BTN} onClick={() => onOpenChange(false)}>
            Пізніше
          </button>
          <button
            type="button"
            className="whitespace-nowrap rounded-full bg-[var(--blue)] px-4 py-2 text-sm font-semibold text-white disabled:opacity-40"
            disabled={busy || chosen.length === 0}
            onClick={linkAll}
          >
            Прив&apos;язати позначені ({chosen.length})
          </button>
        </div>
      </DialogContent>
    </Dialog>
  );
}
