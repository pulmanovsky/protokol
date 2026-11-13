'use client';

import React, { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useRouter } from 'next/navigation';
import { Switch } from '@/components/ui/switch';
import { SettingsGroup, SettingsRow } from '@/components/settings/SettingsGroup';
import { accountsSummary, useMeetAccounts } from '@/components/MeetAccounts';
import { confirmNative } from '@/lib/confirm';
import { loadMeetSettings, type MeetSettingKey, type MeetSettings as Switches } from '@/lib/meet';
import { plural } from '@/lib/plural';
import { toast } from '@/lib/notice';

interface MeetData {
  meetings: number;
  bytes: number;
  signedIn: boolean | null;
  windowOpen: boolean;
}

function size(bytes: number): string {
  if (bytes < 1024 * 1024) return `${Math.max(1, Math.round(bytes / 1024))} КБ`;
  return `${(bytes / 1024 / 1024).toLocaleString('uk-UA', { maximumFractionDigits: 1 })} МБ`;
}

function Label({ title, sub }: { title: string; sub?: React.ReactNode }) {
  return (
    <span className="flex flex-col min-w-0">
      <span>{title}</span>
      {sub && <span className="text-xs text-[color:var(--muted-text)]">{sub}</span>}
    </span>
  );
}

/** Settings → Google Meet: the meeting window, recording offers, speakers, sign-in and data */
export function MeetSettings() {
  const router = useRouter();
  const [switches, setSwitches] = useState<Switches | null>(null);
  const [data, setData] = useState<MeetData | null>(null);
  const { accounts } = useMeetAccounts();

  useEffect(() => {
    loadMeetSettings().then(setSwitches);
    invoke<MeetData>('get_meet_data').then(setData).catch(() => setData(null));
  }, []);

  const change = async (key: MeetSettingKey, on: boolean) => {
    setSwitches((s) => (s ? { ...s, [key]: on } : s));
    try {
      await invoke('set_meet_setting', { key, on });
    } catch (e) {
      setSwitches((s) => (s ? { ...s, [key]: !on } : s));
      toast.error(`${e}`);
    }
  };

  const toggle = (key: MeetSettingKey, label: string) => (
    <Switch checked={switches?.[key] ?? true} disabled={!switches} onCheckedChange={(on) => void change(key, on)} aria-label={label} />
  );

  const signOut = async () => {
    const ok = await confirmNative(
      'Вийти з Google у вікні Meet?',
      'Вікно Meet закриється, а вхід і дані сайтів Google у ньому буде видалено. Chrome і Safari це не зачіпає.',
      'Вийти',
    );
    if (!ok) return;
    try {
      setData(await invoke<MeetData>('meet_sign_out'));
      toast.success('Вихід з Google у вікні Meet виконано');
    } catch (e) {
      toast.error(`${e}`);
    }
  };

  const deleteData = async () => {
    if (!data) return;
    const ok = await confirmNative(
      'Видалити дані нарад Google Meet?',
      `Буде видалено дані ${data.meetings} ${plural(data.meetings, 'наради', 'нарад', 'нарад')}, що зберігаються окремо від записів. Записи і розпізнані в них спікери не зміняться.`,
      'Видалити',
    );
    if (!ok) return;
    try {
      setData(await invoke<MeetData>('delete_meet_data'));
      toast.success('Дані нарад Google Meet видалено');
    } catch (e) {
      toast.error(`${e}`);
    }
  };

  const signInText =
    data?.signedIn === true
      ? 'Ви увійшли. Вхід окремий від Chrome і Safari'
      : data?.signedIn === false
        ? 'Ви не ввійшли. Вхід окремий від Chrome і Safari'
        : 'Вхід виконується у вікні Meet, окремо від Chrome і Safari';

  const linked = accounts?.filter((a) => a.person).length ?? 0;
  const open = accounts?.filter((a) => !a.person && !a.shared).length ?? 0;

  return (
    <div className="flex flex-col gap-6">
      <SettingsGroup title="Вікно наради">
        <SettingsRow
          label={<Label title="Відкривати наради Google Meet у вікні Протоколу" sub="Кнопки в картці наради і в нагадуваннях календаря. Вимкнено: у браузері, як раніше" />}
        >
          {toggle('openInApp', 'Відкривати наради Google Meet у вікні Протоколу')}
        </SettingsRow>
      </SettingsGroup>

      <SettingsGroup title="Запис">
        <SettingsRow label={<Label title="Нагадувати почати запис, коли нарада почалась" sub="Якщо у вікні Meet почалась нарада, а запис не йде" />}>
          {toggle('remindStart', 'Нагадувати почати запис')}
        </SettingsRow>
        <SettingsRow label={<Label title="Пропонувати завершити запис, коли нарада завершилась" />}>{toggle('offerStop', 'Пропонувати завершити запис')}</SettingsRow>
        <SettingsRow
          label={
            <Label
              title="Записувати мікрофон, лише коли він увімкнений у Google Meet"
              sub="Мікрофон вимкнено в Meet: голос із цього Mac не записується. Увімкнули: записується з тієї ж секунди. Звук дзвінка записується завжди"
            />
          }
        >
          {toggle('micSync', 'Записувати мікрофон, лише коли він увімкнений у Google Meet')}
        </SettingsRow>
      </SettingsGroup>

      <SettingsGroup title="Спікери">
        <SettingsRow label={<Label title="Ділити репліки за учасниками Google Meet" sub="Після запису. Вимкнено: спікери лише за голосом, як для інших нарад" />}>
          {toggle('split', 'Ділити репліки за учасниками Google Meet')}
        </SettingsRow>
        <SettingsRow
          label={
            <Label
              title="Облікові записи Google Meet і люди"
              sub={accounts ? `${linked} прив'язано · ${open} без людини. Редагуються в одному місці: «Пам'ять»` : accountsSummary(null)}
            />
          }
        >
          <button type="button" className="whitespace-nowrap text-[13px] text-[var(--blue)] hover:underline" onClick={() => router.push('/settings?section=memory&accounts=1')}>
            Керувати в «Пам&apos;яті» ›
          </button>
        </SettingsRow>
      </SettingsGroup>

      <SettingsGroup title="Вхід і дані">
        <SettingsRow label={<Label title="Вхід у Google у вікні Meet" sub={signInText} />}>
          {data?.signedIn === true && (
            <button type="button" className="settings-button" onClick={() => void signOut()}>
              Вийти з Google…
            </button>
          )}
        </SettingsRow>
        <SettingsRow
          label={
            <Label
              title="Дані нарад Google Meet"
              sub={`${data ? `${data.meetings} ${plural(data.meetings, 'нарада', 'наради', 'нарад')} · ${size(data.bytes)}. ` : ''}Окремо зберігаються 60 днів, у записах разом із записом`}
            />
          }
        >
          <button type="button" className="settings-button" onClick={() => invoke('reveal_meet_data').catch((e) => toast.error(`${e}`))}>
            Показати у Finder
          </button>
          <button
            type="button"
            className="settings-button text-[#c4001a] dark:text-[#ff6961]"
            disabled={!data || data.meetings === 0}
            onClick={() => void deleteData()}
          >
            Видалити…
          </button>
        </SettingsRow>
        <SettingsRow
          label={
            <Label
              title="Що бачить Протокол у вікні Meet"
              sub="Хто з учасників говорить, гучність кожного, імена на плитках і чи увімкнений мікрофон у Meet. Відео, чат і показ екрана не зберігаються. Нічого з цього не надсилається"
            />
          }
        />
      </SettingsGroup>

      <p className="settings-help" style={{ marginTop: -8 }}>
        Записувати мікрофон і звук дзвінка:{' '}
        <button type="button" className="text-[var(--blue)] hover:underline" onClick={() => router.push('/settings?section=recording')}>
          Запис
        </button>
        , як для будь-якої наради. Облікові записи Meet видно також у{' '}
        <button type="button" className="text-[var(--blue)] hover:underline" onClick={() => router.push('/settings?section=memory')}>
          Пам&apos;ять
        </button>
        , у картці кожної людини.
      </p>
    </div>
  );
}
