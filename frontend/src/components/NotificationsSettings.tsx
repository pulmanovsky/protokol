'use client';

import React, { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Switch } from '@/components/ui/switch';
import { SettingsGroup, SettingsRow } from '@/components/settings/SettingsGroup';
import { useConfig, NotificationSettings } from '@/contexts/ConfigContext';
import { toast } from '@/lib/notice';

type Prefs = NotificationSettings['notification_preferences'];
type Permission = 'allowed' | 'denied' | 'unknown';

function Label({ title, sub }: { title: string; sub?: string }) {
  return (
    <span className="flex flex-col min-w-0">
      <span>{title}</span>
      {sub && <span className="text-xs text-[color:var(--muted-text)]">{sub}</span>}
    </span>
  );
}

/**
 * Settings → Сповіщення: what the app says without being asked. macOS
 * notifications (only while «Протокол» is behind other windows), the calendar
 * reminder and the reminder at the start of a recording.
 */
export function NotificationsSettings() {
  const { notificationSettings, loadPreferences, updateNotificationSettings } = useConfig();
  const [permission, setPermission] = useState<Permission>('unknown');
  const [participantsReminder, setParticipantsReminder] = useState<boolean | null>(null);
  const [remindBefore, setRemindBefore] = useState<number | null>(null);

  useEffect(() => {
    loadPreferences();
  }, [loadPreferences]);

  // macOS can switch notifications off at any time: read it again when the window comes back
  useEffect(() => {
    const read = () => invoke<Permission>('api_notification_permission').then(setPermission).catch(() => setPermission('unknown'));
    read();
    window.addEventListener('focus', read);
    return () => window.removeEventListener('focus', read);
  }, []);

  useEffect(() => {
    invoke<{ settings: { remind_minutes_before: number } }>('api_calendar_status')
      .then((s) => setRemindBefore(s.settings.remind_minutes_before))
      .catch(() => {});
    (async () => {
      try {
        const { Store } = await import('@tauri-apps/plugin-store');
        const store = await Store.load('preferences.json');
        setParticipantsReminder((await store.get<boolean>('show_recording_notification')) ?? true);
      } catch {
        setParticipantsReminder(true);
      }
    })();
  }, []);

  const prefs = notificationSettings?.notification_preferences;
  const pref = (key: keyof Prefs) => (prefs ? prefs[key] !== false : true);

  const setPref = async (key: keyof Prefs, on: boolean) => {
    if (!notificationSettings) return;
    try {
      await updateNotificationSettings({
        ...notificationSettings,
        notification_preferences: { ...notificationSettings.notification_preferences, [key]: on },
      });
    } catch (e) {
      console.error('Failed to save notification settings:', e);
      toast.error('Не вдалося зберегти налаштування');
    }
  };

  const setReminder = async (on: boolean) => {
    setParticipantsReminder(on);
    try {
      const { Store } = await import('@tauri-apps/plugin-store');
      const store = await Store.load('preferences.json');
      await store.set('show_recording_notification', on);
      await store.save();
    } catch (e) {
      console.error('Failed to save the recording reminder setting:', e);
      setParticipantsReminder(!on);
      toast.error('Не вдалося зберегти налаштування');
    }
  };

  const openSystemSettings = () =>
    void invoke('api_open_notification_settings').catch(() => toast.error('Не вдалося відкрити Системні параметри'));

  const toggle = (key: keyof Prefs, label: string) => (
    <Switch checked={pref(key)} disabled={!prefs} onCheckedChange={(v) => void setPref(key, v)} aria-label={label} />
  );

  const status =
    permission === 'denied'
      ? { dot: 'var(--warning, #ff9f0a)', title: 'Сповіщення вимкнено в macOS', sub: 'Увімкніть їх для «Протоколу» в Системних параметрах' }
      : permission === 'allowed'
        ? { dot: 'var(--success, #30d158)', title: 'Сповіщення macOS дозволено', sub: 'Стиль, звук і «Не турбувати» задаються в Системних параметрах' }
        : { dot: 'var(--muted-text)', title: 'Сповіщення macOS', sub: 'Стиль, звук і «Не турбувати» задаються в Системних параметрах' };

  return (
    <div className="flex flex-col gap-6">
      <SettingsGroup>
        <SettingsRow
          label={
            <span className="flex items-center gap-2.5 min-w-0">
              <span className="w-2 h-2 rounded-full flex-none" style={{ background: status.dot }} aria-hidden="true" />
              <Label title={status.title} sub={status.sub} />
            </span>
          }
        >
          <button type="button" className="settings-button" onClick={openSystemSettings}>
            Системні параметри…
          </button>
        </SettingsRow>
      </SettingsGroup>

      <SettingsGroup
        title="Коли Протокол у фоні"
        help="Сповіщення macOS з'являються, лише коли вікно Протоколу закрите або під іншими вікнами. Коли програма на передньому плані, про ці події повідомляє сама програма."
      >
        <SettingsRow label={<Label title="Стенограма готова" sub="Назва запису і кількість місць для перевірки" />}>
          {toggle('show_transcription_complete', 'Стенограма готова')}
        </SettingsRow>
        <SettingsRow label="Документ готовий">{toggle('show_document_ready', 'Документ готовий')}</SettingsRow>
        <SettingsRow label="Не вдалося створити стенограму або документ">
          {toggle('show_processing_failed', 'Не вдалося створити стенограму або документ')}
        </SettingsRow>
      </SettingsGroup>

      <SettingsGroup title="Нагадування">
        <SettingsRow
          label={
            <Label
              title="Нарада з календаря починається"
              sub={`${remindBefore !== null ? `За ${remindBefore} хв до початку · ` : ''}час і календарі задаються в розділі «Календар»`}
            />
          }
        >
          {toggle('show_meeting_reminders', 'Нарада з календаря починається')}
        </SettingsRow>
        <SettingsRow label={<Label title="Нагадувати повідомити учасників про запис" sub="Показується на екрані запису одразу після початку" />}>
          <Switch
            checked={participantsReminder ?? true}
            disabled={participantsReminder === null}
            onCheckedChange={(v) => void setReminder(v)}
            aria-label="Нагадувати повідомити учасників про запис"
          />
        </SettingsRow>
      </SettingsGroup>
    </div>
  );
}
