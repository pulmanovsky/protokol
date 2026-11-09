import React, { useState, useEffect } from 'react';
import { Switch } from '@/components/ui/switch';
import { FolderOpen } from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import { DeviceSelection, SelectedDevices } from '@/components/DeviceSelection';
import { SettingsGroup, SettingsRow, SettingsBlock } from '@/components/settings/SettingsGroup';
import Analytics from '@/lib/analytics';
import { toast } from '@/lib/notice';
import { useRecordingState } from '@/contexts/RecordingStateContext';
import { useConfig } from '@/contexts/ConfigContext';
import { useAudioSources } from '@/lib/audioSources';

export interface RecordingPreferences {
  save_folder: string;
  auto_save: boolean;
  file_format: string;
  preferred_mic_device: string | null;
  preferred_system_device: string | null;
}

interface RecordingSettingsProps {
  onSave?: (preferences: RecordingPreferences) => void;
}

export function RecordingSettings({ onSave }: RecordingSettingsProps) {
  const [preferences, setPreferences] = useState<RecordingPreferences>({
    save_folder: '',
    auto_save: true,
    file_format: 'mp4',
    preferred_mic_device: null,
    preferred_system_device: null
  });
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [showRecordingNotification, setShowRecordingNotification] = useState(true);
  const { isRecording } = useRecordingState();
  const { setSelectedDevices } = useConfig();
  const [audioSources, setAudioSources] = useAudioSources();
  const [hideFromScreenShare, setHideFromScreenShare] = useState(false);
  const [autoDocument, setAutoDocument] = useState(false);
  useEffect(() => {
    invoke<boolean>('get_auto_document').then(setAutoDocument).catch(() => {});
  }, []);
  const handleAutoDocument = async (enabled: boolean) => {
    setAutoDocument(enabled);
    try {
      await invoke('set_auto_document', { enabled });
    } catch {
      setAutoDocument(!enabled);
      toast.error('Не вдалося зберегти налаштування');
    }
  };
  useEffect(() => {
    invoke<boolean>('api_get_hide_from_screen_share').then(setHideFromScreenShare).catch(() => {});
  }, []);
  const handleHideFromScreenShare = async (enabled: boolean) => {
    setHideFromScreenShare(enabled);
    try {
      await invoke('api_set_hide_from_screen_share', { enabled });
    } catch (error) {
      console.error('Failed to save screen share setting', error);
      setHideFromScreenShare(!enabled);
      toast.error('Не вдалося зберегти налаштування');
    }
  };

  // Load recording preferences on component mount
  useEffect(() => {
    const loadPreferences = async () => {
      try {
        const prefs = await invoke<RecordingPreferences>('get_recording_preferences');
        setPreferences(prefs);
      } catch (error) {
        console.error('Failed to load recording preferences:', error);
        // If loading fails, get default folder path
        try {
          const defaultPath = await invoke<string>('get_default_recordings_folder_path');
          setPreferences(prev => ({ ...prev, save_folder: defaultPath }));
        } catch (defaultError) {
          console.error('Failed to get default folder path:', defaultError);
        }
      } finally {
        setLoading(false);
      }
    };

    loadPreferences();
  }, []);

  // Load recording notification preference
  useEffect(() => {
    const loadNotificationPref = async () => {
      try {
        const { Store } = await import('@tauri-apps/plugin-store');
        const store = await Store.load('preferences.json');
        const show = await store.get<boolean>('show_recording_notification') ?? true;
        setShowRecordingNotification(show);
      } catch (error) {
        console.error('Failed to load notification preference:', error);
      }
    };
    loadNotificationPref();
  }, []);

  const handleAutoSaveToggle = async (enabled: boolean) => {
    const newPreferences = { ...preferences, auto_save: enabled };
    setPreferences(newPreferences);
    await savePreferences(newPreferences);

    // Track auto-save setting change
    await Analytics.track('auto_save_recording_toggled', {
      enabled: enabled.toString()
    });
  };

  const handleDeviceChange = async (devices: SelectedDevices) => {
    const newPreferences = {
      ...preferences,
      preferred_mic_device: devices.micDevice,
      preferred_system_device: devices.systemDevice
    };
    setPreferences(newPreferences);
    // Sync the in-memory selection ConfigContext exposes to the start path.
    // Without this, the picker only writes to disk (loaded into context once at
    // app mount), so a newly-chosen mic isn't honored until the next launch —
    // start keeps sending the stale launch-time device.
    setSelectedDevices(devices);
    await savePreferences(newPreferences);

    // Track default device preference changes
    // Note: Individual device selection analytics are tracked in DeviceSelection component
    await Analytics.track('default_devices_changed', {
      has_preferred_microphone: (!!devices.micDevice).toString(),
      has_preferred_system_audio: (!!devices.systemDevice).toString()
    });
  };

  const handleOpenFolder = async () => {
    try {
      await invoke('open_recordings_folder');
    } catch (error) {
      console.error('Failed to open recordings folder:', error);
    }
  };

  const handleNotificationToggle = async (enabled: boolean) => {
    try {
      setShowRecordingNotification(enabled);
      const { Store } = await import('@tauri-apps/plugin-store');
      const store = await Store.load('preferences.json');
      await store.set('show_recording_notification', enabled);
      await store.save();
      toast.success('Збережено');
      await Analytics.track('recording_notification_preference_changed', {
        enabled: enabled.toString()
      });
    } catch (error) {
      console.error('Failed to save notification preference:', error);
      toast.error('Не вдалося зберегти');
    }
  };

  const savePreferences = async (prefs: RecordingPreferences) => {
    setSaving(true);
    try {
      await invoke('set_recording_preferences', { preferences: prefs });
      onSave?.(prefs);

      // Show success toast with device details
      const micDevice = prefs.preferred_mic_device || 'за замовчуванням';
      const systemDevice = prefs.preferred_system_device || 'за замовчуванням';
      toast.success("Пристрої збережено", {
        description: `Мікрофон: ${micDevice}, звук системи: ${systemDevice}`
      });
    } catch (error) {
      console.error('Failed to save recording preferences:', error);
      toast.error("Не вдалося зберегти пристрої", {
        description: error instanceof Error ? error.message : String(error)
      });
    } finally {
      setSaving(false);
    }
  };

  if (loading) {
    return (
      <div className="animate-pulse">
        <div className="h-4 bg-gray-200 rounded w-1/4 mb-4"></div>
        <div className="h-8 bg-gray-200 rounded mb-4"></div>
      </div>
    );
  }

  return (
    <div className="flex flex-col gap-6">
      <SettingsGroup
        title="Аудіо"
        help={
          preferences.auto_save
            ? 'Кожен запис — окрема папка зі звуком, названа за датою та часом.'
            : 'Аудіо не зберігається: залишаються лише стенограма і підсумок.'
        }
      >
        <SettingsRow label="Зберігати аудіозаписи">
          <Switch checked={preferences.auto_save} onCheckedChange={handleAutoSaveToggle} disabled={saving} />
        </SettingsRow>
        {preferences.auto_save && (
          <SettingsRow
            label={
              <span className="flex flex-col min-w-0">
                <span>Папка для записів</span>
                <span className="text-xs text-[color:var(--muted-text)] truncate" title={preferences.save_folder}>
                  {preferences.save_folder || 'Папка за замовчуванням'}
                </span>
              </span>
            }
          >
            <button type="button" onClick={handleOpenFolder} className="settings-button">
              <FolderOpen className="w-3.5 h-3.5" />
              Відкрити
            </button>
          </SettingsRow>
        )}
      </SettingsGroup>

      <SettingsGroup help="Після запису чи імпорту спікери розпізнаються самі (на цьому Mac). Далі можна одразу створити документ обраного типу.">
        <SettingsRow label="Створювати документ після розпізнавання спікерів">
          <Switch checked={autoDocument} onCheckedChange={handleAutoDocument} />
        </SettingsRow>
      </SettingsGroup>

      <SettingsGroup help="На початку запису з'явиться нагадування повідомити учасників, що розмова записується.">
        <SettingsRow label="Нагадування про запис">
          <Switch checked={showRecordingNotification} onCheckedChange={handleNotificationToggle} />
        </SettingsRow>
      </SettingsGroup>

      <SettingsGroup
        title="Приватність"
        help="Плавучий острівець запису ніколи не потрапляє в показ екрана. Головне вікно можна теж сховати від показу екрана в Zoom, Meet, Teams і від знімків екрана — тоді з нього не вийде зробити скріншот."
      >
        <SettingsRow label="Ховати і головне вікно Протоколу">
          <Switch checked={hideFromScreenShare} onCheckedChange={handleHideFromScreenShare} />
        </SettingsRow>
      </SettingsGroup>

      <SettingsGroup
        title="Джерела звуку"
        help="Що записувати. Потрібне хоча б одне джерело. Під час запису мікрофон можна тимчасово вимкнути кнопкою або ⌘⇧M."
      >
        <SettingsRow label="Мікрофон">
          <Switch
            checked={audioSources.micEnabled}
            disabled={isRecording || !audioSources.systemEnabled}
            onCheckedChange={(checked) => setAudioSources({ ...audioSources, micEnabled: checked })}
          />
        </SettingsRow>
        <SettingsRow label="Звук дзвінка (весь звук Mac)">
          <Switch
            checked={audioSources.systemEnabled}
            disabled={isRecording || !audioSources.micEnabled}
            onCheckedChange={(checked) => setAudioSources({ ...audioSources, systemEnabled: checked })}
          />
        </SettingsRow>
      </SettingsGroup>

      <SettingsGroup
        title="Пристрої за замовчуванням"
        help={
          isRecording
            ? 'Під час запису пристрої змінити не можна. Завершіть запис, щоб змінити їх.'
            : 'Ці мікрофон і джерело звуку системи вибиратимуться для нових записів.'
        }
      >
        <SettingsBlock>
          <DeviceSelection
            selectedDevices={{
              micDevice: preferences.preferred_mic_device,
              systemDevice: preferences.preferred_system_device
            }}
            onDeviceChange={handleDeviceChange}
            disabled={saving || isRecording}
          />
        </SettingsBlock>
      </SettingsGroup>
    </div>
  );
}