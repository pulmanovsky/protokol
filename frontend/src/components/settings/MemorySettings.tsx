'use client';

import React, { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useRouter, useSearchParams } from 'next/navigation';
import { MeetAccountsDialog, accountsSummary, useMeetAccounts } from '@/components/MeetAccounts';
import { ChevronRight } from 'lucide-react';
import { Switch } from '@/components/ui/switch';
import { SettingsGroup, SettingsRow } from '@/components/settings/SettingsGroup';
import { plural } from '@/lib/plural';
import { toast } from '@/lib/notice';

function LinkRow({ title, sub, count, onClick }: { title: string; sub: string; count: string; onClick: () => void }) {
  return (
    <button type="button" className="settings-row w-full text-left hover:bg-[var(--hover)]" onClick={onClick}>
      <span className="flex flex-col min-w-0">
        <span>{title}</span>
        <span className="text-xs text-[color:var(--muted-text)]">{sub}</span>
      </span>
      <span className="flex items-center gap-1.5 text-[13px] text-[color:var(--muted-text)] whitespace-nowrap">
        {count}
        <ChevronRight className="w-3.5 h-3.5" aria-hidden="true" />
      </span>
    </button>
  );
}

/** Settings → Пам'ять: what the app learned from corrections and approvals */
export function MemorySettings() {
  const router = useRouter();
  const searchParams = useSearchParams();
  // «Керувати в «Пам'яті»» (Settings → Google Meet) opens the accounts at once
  const [accountsOpen, setAccountsOpen] = useState(false);
  const { accounts, reload: reloadAccounts } = useMeetAccounts();
  useEffect(() => {
    if (searchParams.get('accounts') === '1') setAccountsOpen(true);
  }, [searchParams]);
  const [voiceOn, setVoiceOn] = useState<boolean | null>(null);
  const [people, setPeople] = useState<number | null>(null);
  const [words, setWords] = useState<number | null>(null);
  const [rules, setRules] = useState<number | null>(null);
  const [examples, setExamples] = useState<number | null>(null);

  useEffect(() => {
    invoke<boolean>('api_voice_recognition_get').then(setVoiceOn).catch(() => setVoiceOn(true));
    invoke<number>('api_voice_profile_count').then(setPeople).catch(() => setPeople(0));
    Promise.all([
      invoke<string[]>('api_get_glossary').catch(() => [] as string[]),
      invoke<unknown[]>('api_learned_terms').catch(() => [] as unknown[]),
    ]).then(([g, l]) => setWords(g.filter((t) => t.trim()).length + l.length));
    invoke<unknown[]>('api_corrections').then((c) => setRules(c.length)).catch(() => setRules(0));
    invoke<{ examples: number }[]>('api_learned_meetings')
      .then((m) => setExamples(m.reduce((n, x) => n + x.examples, 0)))
      .catch(() => setExamples(0));
  }, []);

  const changeVoice = async (on: boolean) => {
    setVoiceOn(on);
    try {
      await invoke('api_voice_recognition_set', { on });
    } catch (e) {
      setVoiceOn(!on);
      toast.error(`${e}`);
    }
  };

  const n = (v: number | null, one: string, few: string, many: string) => (v === null ? '…' : `${v} ${plural(v, one, few, many)}`);

  return (
    <div className="flex flex-col gap-6">
      <p className="settings-help" style={{ marginTop: -8 }}>Що програма бере з ваших виправлень. Усе зберігається лише на цьому Mac.</p>

      <SettingsGroup title="Люди" help="Голос зберігається, коли ви підписуєте спікера в стенограмі.">
        <SettingsRow
          label={
            <span className="flex flex-col min-w-0">
              <span>Упізнавати збережені голоси</span>
              <span className="text-xs text-[color:var(--muted-text)]">Упевнені збіги підписуються одразу, сумнівні з «?» для підтвердження. Вимкнено: голоси не впізнаються і не зберігаються</span>
            </span>
          }
        >
          <Switch checked={voiceOn ?? true} disabled={voiceOn === null} onCheckedChange={(v) => void changeVoice(v)} aria-label="Упізнавати збережені голоси" />
        </SettingsRow>
        <LinkRow
          title="Збережені люди"
          sub="Ім'я, посада, зразки голосу, об'єднати двох, «Не пропонувати», «Забути»"
          count={n(people, 'людина', 'людини', 'людей')}
          onClick={() => router.push('/templates?people=1')}
        />
        <LinkRow
          title="Облікові записи Google Meet"
          sub="Хто з людей сидить під яким обліковим записом Meet"
          count={accountsSummary(accounts)}
          onClick={() => setAccountsOpen(true)}
        />
      </SettingsGroup>
      <MeetAccountsDialog
        open={accountsOpen}
        onOpenChange={(open) => {
          setAccountsOpen(open);
          if (!open) void reloadAccounts();
        }}
      />

      <SettingsGroup title="Написання" help="Правило створюється, коли ви виправляєте слово в стенограмі. Нові записи виправляються за ним автоматично, такі місця позначено.">
        <LinkRow
          title="Слова"
          sub="Як правильно писати імена, посади, абревіатури. Вручну, вивчені автоматично та підказки з останніх записів"
          count={n(words, 'слово', 'слова', 'слів')}
          onClick={() => router.push('/templates?tab=words')}
        />
        <LinkRow
          title="Правила виправлень"
          sub="Почуто → записується. Звідки взялося, скільки разів застосовано; кожне можна вимкнути або забути"
          count={n(rules, 'правило', 'правила', 'правил')}
          onClick={() => router.push('/templates')}
        />
      </SettingsGroup>

      <SettingsGroup title="Документ" help="Приклад додається, коли ви затверджуєте документ. За ним нові документи пишуться ближче до вашого стилю.">
        <LinkRow
          title="Приклади документів"
          sub="З яких записів, «Забути» для окремого запису"
          count={n(examples, 'приклад', 'приклади', 'прикладів')}
          onClick={() => router.push('/templates')}
        />
      </SettingsGroup>
    </div>
  );
}
