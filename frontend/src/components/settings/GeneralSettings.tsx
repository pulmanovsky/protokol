'use client';

import React, { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Switch } from '@/components/ui/switch';
import { SettingsGroup, SettingsRow } from '@/components/settings/SettingsGroup';
import { toast } from '@/lib/notice';
import { restorePlace, setRestorePlace, setStartScreen, startScreen, StartScreen } from '@/lib/uiPrefs';

function Label({ title, sub }: { title: string; sub: string }) {
  return (
    <span className="flex flex-col min-w-0">
      <span>{title}</span>
      <span className="text-xs text-[color:var(--muted-text)]">{sub}</span>
    </span>
  );
}

const START: { value: StartScreen; label: string }[] = [
  { value: 'today', label: 'Сьогодні' },
  { value: 'records', label: 'Записи' },
  { value: 'last', label: 'Останній запис' },
];

/** Settings → Загальні: what concerns the whole app */
export function GeneralSettings() {
  const [login, setLogin] = useState<{ on: boolean; needs_approval: boolean } | null>(null);
  const [tray, setTray] = useState<boolean | null>(null);
  const [start, setStart] = useState<StartScreen>('today');
  const [place, setPlace] = useState(true);

  useEffect(() => {
    invoke<{ on: boolean; needs_approval: boolean }>('api_login_item_get').then(setLogin).catch(() => setLogin({ on: false, needs_approval: false }));
    invoke<boolean>('api_tray_visible_get').then(setTray).catch(() => setTray(true));
    setStart(startScreen());
    setPlace(restorePlace());
  }, []);

  const changeLogin = async (on: boolean) => {
    try {
      setLogin(await invoke<{ on: boolean; needs_approval: boolean }>('api_login_item_set', { on }));
    } catch (e) {
      toast.error(`${e}`);
    }
  };

  const changeTray = async (on: boolean) => {
    setTray(on);
    try {
      await invoke('api_tray_visible_set', { on });
    } catch (e) {
      setTray(!on);
      toast.error(`${e}`);
    }
  };

  return (
    <div className="flex flex-col gap-6">
      <SettingsGroup
        title="Запуск"
        help="Якщо значок сховано, а вікно закрито, Протокол відкривається з Launchpad або Dock."
      >
        <SettingsRow
          label={
            <Label
              title="Відкривати Протокол під час входу в систему"
              sub={login?.needs_approval ? 'macOS чекає дозволу: Системні параметри → Загальні → Об’єкти входу' : 'Щоб не пропустити нагадування календаря'}
            />
          }
        >
          <Switch checked={!!login?.on} disabled={!login} onCheckedChange={(v) => void changeLogin(v)} aria-label="Відкривати Протокол під час входу в систему" />
        </SettingsRow>
        <SettingsRow label={<Label title="Показувати значок у рядку меню" sub="Звідти можна почати або завершити запис, не відкриваючи вікно" />}>
          <Switch checked={tray ?? true} disabled={tray === null} onCheckedChange={(v) => void changeTray(v)} aria-label="Показувати значок у рядку меню" />
        </SettingsRow>
      </SettingsGroup>

      <SettingsGroup title="Після запуску">
        <SettingsRow label="Відкривати" htmlFor="start-screen">
          <select
            id="start-screen"
            className="h-7 px-2 text-[13px]"
            value={start}
            onChange={(e) => {
              const v = e.target.value as StartScreen;
              setStart(v);
              setStartScreen(v);
            }}
          >
            {START.map((o) => (
              <option key={o.value} value={o.value}>
                {o.label}
              </option>
            ))}
          </select>
        </SettingsRow>
      </SettingsGroup>

      <SettingsGroup title="Вікно">
        <SettingsRow label={<Label title="Повертатися до місця в стенограмі" sub="Відкритий запис показується там, де його залишили" />}>
          <Switch
            checked={place}
            onCheckedChange={(v) => {
              setPlace(v);
              setRestorePlace(v);
            }}
            aria-label="Повертатися до місця в стенограмі"
          />
        </SettingsRow>
      </SettingsGroup>
    </div>
  );
}
