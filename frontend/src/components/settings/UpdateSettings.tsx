'use client';

import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { relaunch } from '@tauri-apps/plugin-process';
import { getVersion } from '@tauri-apps/api/app';
import { toast } from '@/lib/notice';
import { Switch } from '@/components/ui/switch';
import { SettingsGroup, SettingsRow } from './SettingsGroup';
import { confirmNative } from '@/lib/confirm';

interface Available {
  version: string;
  current: string;
  notes?: string | null;
}

/**
 * «Оновлення»: where the app looks for a newer version (one address, asked
 * once a day when allowed, nothing about the user sent), «Перевірити зараз»,
 * and an update from a folder for closed networks. Installed only on the
 * user's word; every update is checked against the app's signing key.
 */
export function UpdateSettings() {
  const [version, setVersion] = useState('');
  const [url, setUrl] = useState('');
  const [auto, setAuto] = useState(false);
  const [last, setLast] = useState<string | null>(null);
  const [busy, setBusy] = useState<'' | 'check' | 'install'>('');
  const [found, setFound] = useState<Available | null>(null);
  const [progress, setProgress] = useState<number | null>(null);

  useEffect(() => {
    void getVersion().then(setVersion).catch(() => {});
    invoke<{ url: string; auto: boolean; lastCheck?: string | null }>('api_update_settings')
      .then((s) => {
        setUrl(s.url);
        setAuto(s.auto);
        setLast(s.lastCheck ?? null);
      })
      .catch(() => {});
    // Found by the daily check: shown at once, ready to install
    invoke<Available | null>('api_update_found')
      .then((f) => f && setFound(f))
      .catch(() => {});
    let unlisten: (() => void) | undefined;
    void listen<{ downloaded: number; total?: number | null }>('update-progress', (e) => {
      if (e.payload.total) setProgress(Math.round((100 * e.payload.downloaded) / e.payload.total));
    }).then((u) => (unlisten = u));
    return () => unlisten?.();
  }, []);

  const save = async (nextUrl = url, nextAuto = auto) => {
    try {
      await invoke('api_set_update_settings', { url: nextUrl, auto: nextAuto });
      return true;
    } catch (e) {
      toast.error(`${e}`);
      return false;
    }
  };

  const check = async (fromFolder?: string) => {
    setBusy('check');
    setFound(null);
    try {
      const r = fromFolder
        ? await invoke<Available | null>('api_update_from_folder', { folder: fromFolder })
        : (await save()) && (await invoke<Available | null>('api_update_check', { url: null }));
      if (r === false) return;
      setLast(new Date().toISOString());
      if (r) setFound(r);
      else toast.success('Встановлено найновішу версію');
    } catch (e) {
      toast.error(`${e}`);
    } finally {
      setBusy('');
    }
  };

  const install = async () => {
    if (!found) return;
    setBusy('install');
    setProgress(0);
    try {
      await invoke('api_update_install');
      setProgress(null);
      if (await confirmNative('Оновлення встановлено', `Версія ${found.version} готова. Перезапустити «Протокол» зараз?`, 'Перезапустити', 'Пізніше')) await relaunch();
    } catch (e) {
      // Kept on screen: a refused update (for example a wrong signature) must be seen
      toast.error(`${e}`, { duration: 30000 });
      setProgress(null);
      setFound(null);
    } finally {
      setBusy('');
    }
  };

  return (
    <SettingsGroup
      title="Оновлення"
      help="Програма звертається лише за вказаною адресою і нічого про вас не надсилає. Кожне оновлення перевіряється підписом, тож ставиться лише справжнє."
    >
      <SettingsRow label={<span>Версія</span>}>
        <span className="text-sm tabular-nums text-[color:var(--muted-text)]">{version}</span>
      </SettingsRow>
      <SettingsRow
        label={
          <span className="flex flex-col min-w-0">
            <span>Адреса оновлень</span>
            <span className="text-xs text-[color:var(--muted-text)]">Посилання на latest.json від того, хто видає програму</span>
          </span>
        }
      >
        <input
          type="url"
          className="h-8 px-2.5 w-[260px] max-w-full"
          placeholder="https://…/latest.json"
          value={url}
          spellCheck={false}
          onChange={(e) => setUrl(e.target.value)}
          onBlur={() => void save()}
          aria-label="Адреса оновлень"
        />
      </SettingsRow>
      <SettingsRow label={<span>Перевіряти щодня</span>}>
        <Switch
          checked={auto}
          disabled={!url.trim()}
          aria-label="Перевіряти оновлення щодня"
          onCheckedChange={(v) => {
            setAuto(v);
            void save(url, v);
          }}
        />
      </SettingsRow>
      <SettingsRow
        label={
          <span className="flex flex-col min-w-0">
            <span>{found ? `Є версія ${found.version}` : 'Перевірити оновлення'}</span>
            <span className="text-xs text-[color:var(--muted-text)]">
              {progress !== null ? `Завантаження… ${progress}%` : found?.notes ? found.notes.slice(0, 140) : last ? `Востаннє: ${new Date(last).toLocaleString('uk-UA')}` : 'Ще не перевіряли'}
            </span>
          </span>
        }
      >
        <div className="flex gap-2">
          {found ? (
            <button type="button" className="accent-button h-8 rounded-lg px-3 text-sm font-medium" disabled={busy !== ''} onClick={() => void install()}>
              {busy === 'install' ? 'Встановлення…' : 'Встановити'}
            </button>
          ) : (
            <button type="button" className="settings-button" disabled={busy !== '' || !url.trim()} onClick={() => void check()}>
              {busy === 'check' ? 'Перевірка…' : 'Перевірити зараз'}
            </button>
          )}
          <button
            type="button"
            className="settings-button"
            disabled={busy !== ''}
            title="Для мереж без інтернету: папка з latest.json і архівом оновлення"
            onClick={async () => {
              const folder = await invoke<string | null>('api_pick_folder', { title: 'Папка з оновленням' }).catch(() => null);
              if (typeof folder === 'string') await check(folder);
            }}
          >
            З папки…
          </button>
        </div>
      </SettingsRow>
    </SettingsGroup>
  );
}
