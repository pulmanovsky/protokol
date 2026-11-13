'use client';

/**
 * Google Meet accounts and the saved people behind them, edited in one
 * place: Settings → Пам'ять («Облікові записи Google Meet») and each
 * person's card in «Збережені люди». «Спікери» after a meeting and Settings →
 * Google Meet lead here.
 */

import React, { useCallback, useEffect, useMemo, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { X } from 'lucide-react';
import { toast } from '@/lib/notice';
import { Dialog, DialogContent, DialogDescription, DialogTitle } from '@/components/ui/dialog';
import { PersonPicker, type PersonLite } from '@/components/MeetingDetails/MeetSpeakers';
import type { MeetAccount, VoiceMatch } from '@/lib/meet';

const BTN = 'whitespace-nowrap rounded-full bg-[var(--selected)] px-3 py-1 text-xs font-medium disabled:opacity-40';

const day = (iso: string | null) => (iso ? new Date(iso).toLocaleDateString('uk-UA', { day: 'numeric', month: 'long' }) : '');

export function useMeetAccounts() {
  const [accounts, setAccounts] = useState<MeetAccount[] | null>(null);
  const reload = useCallback(async () => {
    setAccounts(await invoke<MeetAccount[]>('get_meet_accounts').catch(() => []));
  }, []);
  useEffect(() => {
    void reload();
  }, [reload]);
  return { accounts, reload };
}

/** «прив'язано: 9 · без людини: 2» */
export function accountsSummary(accounts: MeetAccount[] | null): string {
  if (!accounts) return '…';
  const linked = accounts.filter((a) => a.person).length;
  const open = accounts.filter((a) => !a.person && !a.shared).length;
  return `прив'язано: ${linked} · без людини: ${open}`;
}

async function setLink(meetName: string, person: string | null) {
  await invoke('set_meet_account', { meetName, person });
}

type Filter = 'all' | 'linked' | 'open';

/** Settings → Пам'ять → «Облікові записи Google Meet» */
export function MeetAccountsDialog({ open, onOpenChange }: { open: boolean; onOpenChange: (open: boolean) => void }) {
  const { accounts, reload } = useMeetAccounts();
  const [people, setPeople] = useState<PersonLite[]>([]);
  const [filter, setFilter] = useState<Filter>('all');
  const [picking, setPicking] = useState<string | null>(null);

  useEffect(() => {
    if (!open) return;
    void reload();
    invoke<{ people: PersonLite[] }>('get_saved_speakers')
      .then((s) => setPeople(s.people))
      .catch(() => setPeople([]));
    setPicking(null);
  }, [open, reload]);

  const list = accounts ?? [];
  const linked = list.filter((a) => a.person);
  const shared = list.filter((a) => !a.person && a.shared);
  const seen = list.filter((a) => !a.person && !a.shared);
  const filters: { value: Filter; label: string }[] = [
    { value: 'all', label: `Усі · ${list.length}` },
    { value: 'linked', label: `Прив'язані · ${linked.length}` },
    { value: 'open', label: `Без людини · ${seen.length}` },
  ];

  const act = async (what: () => Promise<void>, done: string) => {
    try {
      await what();
      await reload();
      setPicking(null);
      toast.success(done);
    } catch (e) {
      toast.error(`Не вдалося зберегти: ${e}`);
    }
  };

  const picker = (a: MeetAccount) =>
    picking === a.meetName && (
      <div className="px-3 pb-2.5">
        <PersonPicker
          people={people}
          matches={a.voiceLike ? [{ name: a.voiceLike, similarity: 1, level: 'medium' } as VoiceMatch] : []}
          taken={[]}
          current={a.person}
          firstName={a.meetName}
          pickLabel="Прив'язати до"
          onPick={(name) => void act(() => setLink(a.meetName, name), `«${a.meetName}» прив'язано до людини ${name}`)}
        />
      </div>
    );

  const row = (a: MeetAccount, text: React.ReactNode, button: React.ReactNode) => (
    <div key={a.meetName} className="border-t border-[var(--hairline)] first:border-t-0">
      <div className="flex items-center gap-3 px-3.5 py-2.5">
        <span className="flex min-w-0 flex-1 flex-col">{text}</span>
        {button}
      </div>
      {picker(a)}
    </div>
  );

  const pickButton = (a: MeetAccount) => (
    <button type="button" className={BTN} onClick={() => setPicking(picking === a.meetName ? null : a.meetName)} aria-expanded={picking === a.meetName}>
      Прив&apos;язати…
    </button>
  );

  const linkedRows = linked.map((a) =>
    row(
      a,
      <>
        <span className="truncate text-sm">
          {a.meetName} → <b>{a.person}</b>
        </span>
        {a.linkedAt && <span className="text-xs text-[color:var(--muted-text)]">з {day(a.linkedAt)}</span>}
      </>,
      <button type="button" className={BTN} onClick={() => void act(() => setLink(a.meetName, null), `«${a.meetName}» відв'язано`)}>
        Відв&apos;язати
      </button>,
    ),
  );
  const sharedRows = shared.map((a) =>
    row(
      a,
      <>
        <span className="truncate text-sm">{a.meetName}</span>
        <span className="text-xs text-[color:var(--muted-text)]">Говорять різні люди: не прив&apos;язується</span>
      </>,
      pickButton(a),
    ),
  );
  const seenRows = seen.map((a) =>
    row(
      a,
      <>
        <span className="truncate text-sm">{a.meetName}</span>
        <span className="text-xs text-[color:var(--muted-text)]">
          {[day(a.lastSeen), a.voiceLike ? `голос схожий на ${a.voiceLike}` : "голос не знайдено в пам'яті"].filter(Boolean).join(' · ')}
        </span>
      </>,
      pickButton(a),
    ),
  );

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="flex w-[min(620px,calc(100vw-48px))] max-w-none flex-col gap-3 p-6 sm:max-w-none">
        <DialogTitle className="text-lg font-bold">Облікові записи Google Meet</DialogTitle>
        <DialogDescription className="text-[13px] text-[color:var(--muted-text)]">Хто з людей сидить під яким обліковим записом Meet.</DialogDescription>
        <div className="flex flex-wrap gap-1.5" role="radiogroup" aria-label="Які облікові записи показати">
          {filters.map((f) => (
            <button
              key={f.value}
              type="button"
              role="radio"
              aria-checked={filter === f.value}
              onClick={() => setFilter(f.value)}
              className={`rounded-full px-2.5 py-1 text-xs ${filter === f.value ? 'bg-[var(--blue)] font-medium text-white' : 'bg-[var(--selected)]'}`}
            >
              {f.label}
            </button>
          ))}
        </div>
        <div className="flex max-h-[min(440px,55vh)] flex-col gap-3 overflow-y-auto">
          {accounts && list.length === 0 && (
            <p className="py-6 text-center text-sm text-[color:var(--muted-text)]">
              Облікових записів ще немає. Вони з&apos;являються після наради у вікні Google Meet або коли обліковий запис додано в картці людини.
            </p>
          )}
          {filter !== 'open' && linked.length + shared.length > 0 && (
            <div className="overflow-hidden rounded-xl bg-[var(--card-bg)] shadow-[inset_0_0_0_0.5px_var(--hairline)]">
              {linkedRows}
              {filter === 'all' && sharedRows}
            </div>
          )}
          {filter !== 'linked' && seen.length > 0 && (
            <section className="flex flex-col gap-1.5">
              <h3 className="text-[11px] font-semibold uppercase tracking-wide text-[color:var(--muted-text)]">Бачили на нарадах, ще не прив&apos;язані</h3>
              <div className="overflow-hidden rounded-xl bg-[var(--card-bg)] shadow-[inset_0_0_0_0.5px_var(--hairline)]">{seenRows}</div>
            </section>
          )}
        </div>
        <p className="text-xs text-[color:var(--muted-text)]">
          «Відв&apos;язати»: на наступних нарадах цей обліковий запис знову отримає підказку з іменем із Meet, а людину буде впізнано за голосом. Збережений голос не змінюється.
        </p>
        <div className="flex justify-end">
          <button type="button" onClick={() => onOpenChange(false)} className="rounded-full bg-[#1d1d1f] px-5 py-2 text-sm font-semibold text-white dark:bg-[#f2f2f7] dark:text-[#1d1d1f]">
            Готово
          </button>
        </div>
      </DialogContent>
    </Dialog>
  );
}

/** A person's card in «Збережені люди»: the Meet accounts linked to them */
export function PersonMeetAccounts({ person }: { person: string }) {
  const { accounts, reload } = useMeetAccounts();
  const [adding, setAdding] = useState(false);
  const [typed, setTyped] = useState('');

  useEffect(() => {
    setAdding(false);
    setTyped('');
  }, [person]);

  const mine = useMemo(() => (accounts ?? []).filter((a) => a.person === person), [accounts, person]);
  const seen = useMemo(() => (accounts ?? []).filter((a) => !a.person && !a.shared), [accounts]);

  const link = async (meetName: string, to: string | null) => {
    const name = meetName.trim().replace(/\s+/g, ' ');
    if (!name) return;
    try {
      await setLink(name, to);
      await reload();
      setAdding(false);
      setTyped('');
      toast.success(to ? `Обліковий запис «${name}» прив'язано до людини ${to}` : `Обліковий запис «${name}» відв'язано`);
    } catch (e) {
      toast.error(`Не вдалося зберегти: ${e}`);
    }
  };

  return (
    <section className="flex flex-col gap-1.5">
      <h3 className="text-[11px] font-semibold uppercase tracking-wide text-[color:var(--muted-text)]">Облікові записи Google Meet</h3>
      <div className="flex flex-wrap items-center gap-1.5">
        {mine.map((a) => (
          <span key={a.meetName} className="inline-flex items-center gap-1.5 rounded-full bg-[var(--selected)] py-1 pl-3 pr-1 text-xs">
            {a.meetName}
            <button
              type="button"
              aria-label={`Відв'язати «${a.meetName}»`}
              title="Відв'язати"
              onClick={() => void link(a.meetName, null)}
              className="flex h-[18px] w-[18px] items-center justify-center rounded-full bg-[var(--card-bg)]"
            >
              <X className="h-2.5 w-2.5" />
            </button>
          </span>
        ))}
        {!adding && (
          <button type="button" className={BTN} onClick={() => setAdding(true)}>
            Додати обліковий запис Meet…
          </button>
        )}
      </div>
      {adding && (
        <div className="flex flex-col gap-2 rounded-xl bg-[var(--selected)] p-2.5">
          <input
            autoFocus
            value={typed}
            onChange={(e) => setTyped(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter') void link(typed, person);
              if (e.key === 'Escape') setAdding(false);
            }}
            placeholder="Ім'я, як у Google Meet"
            aria-label="Ім'я, як у Google Meet"
            className="h-8 rounded-lg bg-[var(--card-bg)] px-2.5 text-sm outline-none placeholder:text-[color:var(--muted-text)]"
          />
          {seen.length > 0 && (
            <>
              <span className="text-xs text-[color:var(--muted-text)]">Або з тих, кого бачили на нарадах:</span>
              <div className="flex flex-wrap gap-1.5">
                {seen.slice(0, 12).map((a) => (
                  <button key={a.meetName} type="button" className={`${BTN} !bg-[var(--card-bg)]`} onClick={() => setTyped(a.meetName)}>
                    {a.meetName}
                  </button>
                ))}
              </div>
            </>
          )}
          <div className="flex justify-end gap-2">
            <button type="button" className={BTN} onClick={() => setAdding(false)}>
              Скасувати
            </button>
            <button type="button" className={`${BTN} !bg-[var(--blue)] text-white`} disabled={!typed.trim()} onClick={() => void link(typed, person)}>
              Додати
            </button>
          </div>
          <span className="text-xs text-[color:var(--muted-text)]">
            Так обліковий запис можна прив&apos;язати ще до першої наради. Ім&apos;я порівнюється з підписом на плитці Meet.
          </span>
        </div>
      )}
      {mine.length === 0 && !adding && <span className="text-xs text-[color:var(--muted-text)]">Жодного облікового запису не прив&apos;язано.</span>}
    </section>
  );
}
