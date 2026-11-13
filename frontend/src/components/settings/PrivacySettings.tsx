'use client';

import React, { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Shield } from 'lucide-react';
import { Switch } from '@/components/ui/switch';
import { useRouter } from 'next/navigation';
import { SettingsGroup, SettingsLinkRow, SettingsRow } from '@/components/settings/SettingsGroup';
import { confirmNative } from '@/lib/confirm';
import { plural } from '@/lib/plural';
import { toast } from '@/lib/notice';
import { formatBytes } from '@/components/settings/StorageSettings';

function Label({ title, sub }: { title: string; sub: string }) {
  return (
    <span className="flex flex-col min-w-0">
      <span>{title}</span>
      <span className="text-xs text-[color:var(--muted-text)]">{sub}</span>
    </span>
  );
}

/** Settings → Конфіденційність: what stays on this Mac and how to remove it */
export function PrivacySettings() {
  const router = useRouter();
  const [hide, setHide] = useState<boolean | null>(null);
  const [logSize, setLogSize] = useState<number | null>(null);
  const [voices, setVoices] = useState<number | null>(null);
  const [rules, setRules] = useState<number | null>(null);

  const load = () => {
    invoke<number>('api_log_size').then(setLogSize).catch(() => setLogSize(null));
    invoke<number>('api_voice_profile_count').then(setVoices).catch(() => setVoices(0));
    invoke<unknown[]>('api_corrections').then((c) => setRules(c.length)).catch(() => setRules(0));
  };
  useEffect(() => {
    invoke<boolean>('api_get_hide_from_screen_share').then(setHide).catch(() => setHide(false));
    load();
  }, []);

  const changeHide = async (on: boolean) => {
    setHide(on);
    try {
      await invoke('api_set_hide_from_screen_share', { enabled: on });
    } catch {
      setHide(!on);
      toast.error('Не вдалося зберегти налаштування');
    }
  };

  const clearLog = async () => {
    if (!(await confirmNative('Очистити журнал помилок?', 'Технічні записи буде видалено. Записи, стенограми й документи не зміняться.', 'Очистити'))) return;
    try {
      await invoke('api_log_clear');
      load();
    } catch (e) {
      toast.error(`${e}`);
    }
  };

  const deleteVoices = async () => {
    const n = voices ?? 0;
    if (!(await confirmNative(
      `Видалити голосові відбитки ${n} ${plural(n, 'людини', 'людей', 'людей')}?`,
      'Імена залишаються, але голоси в нових записах доведеться підписати знову. Скасувати не можна.',
      'Видалити',
    ))) return;
    try {
      await invoke('api_voice_profiles_delete_all');
      load();
    } catch (e) {
      toast.error(`${e}`);
    }
  };

  const clearRules = async () => {
    const n = rules ?? 0;
    if (!(await confirmNative(
      `Очистити пам'ять виправлень: ${n} ${plural(n, 'правило', 'правила', 'правил')}?`,
      'Нові записи більше не виправлятимуться за цими правилами. Словник не зміниться. Скасувати не можна.',
      'Очистити',
    ))) return;
    try {
      await invoke('api_corrections_clear');
      load();
    } catch (e) {
      toast.error(`${e}`);
    }
  };

  return (
    <div className="flex flex-col gap-6">
      <div className="settings-list">
        <div className="settings-row" style={{ justifyContent: 'flex-start', gap: 12 }}>
          <span className="flex-none w-7 h-7 rounded-[7px] bg-[#0a84ff] flex items-center justify-center" aria-hidden="true">
            <Shield className="w-4 h-4 text-white" />
          </span>
          <Label
            title="Усе обробляється на цьому Mac"
            sub="Звук, стенограми, документи і голоси нікуди не надсилаються. В інтернет програма звертається лише по оновлення, моделі та календар за вашим посиланням, а звернення відкриваєте ви самі. Нарада Google Meet працює через Google, як у браузері."
          />
        </div>
      </div>

      <SettingsGroup title="Google Meet">
        <SettingsLinkRow
          label="Вхід у Google, дані нарад і що бачить Протокол у вікні Meet"
          sub="Усе це в окремому розділі «Google Meet»"
          onClick={() => router.push('/settings?section=meet')}
        />
      </SettingsGroup>

      <SettingsGroup title="Показ екрана">
        <SettingsRow label={<Label title="Приховувати головне вікно під час показу екрана" sub="Учасники дзвінка не бачать стенограму і список записів. Вікно Google Meet лишається видимим: його показуєте ви самі" />}>
          <Switch checked={!!hide} disabled={hide === null} onCheckedChange={(v) => void changeHide(v)} aria-label="Приховувати головне вікно під час показу екрана" />
        </SettingsRow>
        <SettingsRow label={<Label title="Панель запису" sub="Під час показу екрана її не видно іншим" />}>
          <span className="text-[13px] text-[color:var(--muted-text)] whitespace-nowrap">Прихована завжди</span>
        </SettingsRow>
      </SettingsGroup>

      <SettingsGroup
        title="Журнал помилок"
        help="Журнал не надсилається сам. Його рядки можна додати до звернення у «Повідомити про проблему», перед цим видно весь текст."
      >
        <SettingsRow
          label={<Label title="Журнал помилок" sub={`Технічні записи без текстів, назв записів і імен учасників Meet${logSize != null ? ` · ${formatBytes(logSize)}` : ''}. Потрібен, щоб описати проблему`} />}
        >
          <span className="flex gap-2">
            <button type="button" className="settings-button" onClick={() => void invoke('api_log_reveal').catch((e) => toast.error(`${e}`))}>
              Показати у Finder
            </button>
            <button type="button" className="settings-button" onClick={() => void clearLog()}>
              Очистити…
            </button>
          </span>
        </SettingsRow>
      </SettingsGroup>

      <SettingsGroup title="Видалення">
        <SettingsRow
          label={
            <Label
              title="Голосові відбитки"
              sub={`${voices ?? '…'} ${voices != null ? plural(voices, 'людина', 'людини', 'людей') : ''} · імена залишаються, голоси доведеться підписати знову`}
            />
          }
        >
          <button type="button" className="settings-button" style={{ color: "#e5484d" }} disabled={!voices} onClick={() => void deleteVoices()}>
            Видалити всі…
          </button>
        </SettingsRow>
        <SettingsRow
          label={
            <Label
              title="Пам'ять виправлень"
              sub={`${rules ?? '…'} ${rules != null ? plural(rules, 'правило', 'правила', 'правил') : ''} · словник не змінюється`}
            />
          }
        >
          <button type="button" className="settings-button" style={{ color: "#e5484d" }} disabled={!rules} onClick={() => void clearRules()}>
            Очистити…
          </button>
        </SettingsRow>
      </SettingsGroup>
    </div>
  );
}
