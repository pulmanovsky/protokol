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
  /** «storage»: the audio folder group shown in «Загальні»; «recording»: the «Запис» section */
  part?: 'storage' | 'recording';
}

export function RecordingSettings({ onSave, part = 'recording' }: RecordingSettingsProps) {
  const [preferences, setPreferences] = useState<RecordingPreferences>({
    save_folder: '',
    auto_save: true,
    file_format: 'mp4',
    preferred_mic_device: null,
    preferred_system_device: null
  });
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const { isRecording } = useRecordingState();
  const { setSelectedDevices } = useConfig();
  const [audioSources, setAudioSources] = useAudioSources();
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

  const handleAutoSaveToggle = async (enabled: boolean) => {
    const newPreferences = { ...preferences, auto_save: enabled };
    setPreferences(newPreferences);
    await savePreferences(newPreferences, true);

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

  // «Змінити…»: new recordings and imports go to the chosen folder; existing ones stay where they are
  const handleChooseFolder = async () => {
    const folder = await invoke<string | null>('api_pick_folder', { title: 'Папка для записів' }).catch(() => null);
    if (typeof folder !== 'string' || folder === preferences.save_folder) return;
    const newPreferences = { ...preferences, save_folder: folder };
    setPreferences(newPreferences);
    setSaving(true);
    try {
      await invoke('set_recording_preferences', { preferences: newPreferences });
      onSave?.(newPreferences);
      toast.success('Папку записів змінено', { description: 'Нові записи зберігатимуться в ній. Наявні записи залишаються на своєму місці.' });
    } catch (error) {
      setPreferences(preferences);
      toast.error('Не вдалося вибрати цю папку', { description: error instanceof Error ? error.message : String(error) });
    } finally {
      setSaving(false);
    }
  };

  const savePreferences = async (prefs: RecordingPreferences, quiet = false) => {
    setSaving(true);
    try {
      await invoke('set_recording_preferences', { preferences: prefs });
      onSave?.(prefs);

      if (quiet) return;
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

  if (part === 'storage') {
    return (
      <SettingsGroup
        title="Аудіозаписи"
        help={
          preferences.auto_save
            ? 'Кожен запис зберігається в окремій папці зі звуком, названій за датою та часом. Після зміни папки нові записи зберігаються в новій, наявні залишаються на своєму місці.'
            : 'Аудіо не зберігається: залишаються лише стенограма та документ.'
        }
      >
        <SettingsRow label="Зберігати аудіозаписи">
          <Switch checked={preferences.auto_save} onCheckedChange={handleAutoSaveToggle} disabled={saving} />
        </SettingsRow>
        {preferences.auto_save && (
          <SettingsRow
            label={
              <span className="flex flex-col min-w-0">
                <span>Папка записів</span>
                <span className="text-xs text-[color:var(--muted-text)] truncate" title={preferences.save_folder}>
                  {preferences.save_folder || 'Папка за замовчуванням'}
                </span>
              </span>
            }
          >
            <button type="button" onClick={() => void handleChooseFolder()} disabled={saving || isRecording} className="settings-button">
              Змінити…
            </button>
            <button type="button" onClick={handleOpenFolder} className="settings-button">
              <FolderOpen className="w-3.5 h-3.5" />
              Показати у Finder
            </button>
          </SettingsRow>
        )}
      </SettingsGroup>
    );
  }

  return (
    <div className="flex flex-col gap-6">
      <SettingsGroup
        title="Джерела звуку"
        help={
          isRecording
            ? 'Під час запису пристрої змінити не можна. Завершіть запис, щоб змінити їх.'
            : 'Потрібне хоча б одне джерело. Під час запису мікрофон можна тимчасово вимкнути кнопкою на панелі запису або ⇧⌘M.'
        }
      >
        <SettingsRow label="Записувати мікрофон">
          <Switch
            checked={audioSources.micEnabled}
            disabled={isRecording || !audioSources.systemEnabled}
            onCheckedChange={(checked) => setAudioSources({ ...audioSources, micEnabled: checked })}
            aria-label="Записувати мікрофон"
          />
        </SettingsRow>
        <SettingsRow
          label={
            <span className="flex flex-col min-w-0">
              <span>Записувати звук дзвінка</span>
              <span className="text-xs text-[color:var(--muted-text)]">Увесь звук Mac: Meet, Zoom, Teams</span>
            </span>
          }
        >
          <Switch
            checked={audioSources.systemEnabled}
            disabled={isRecording || !audioSources.micEnabled}
            onCheckedChange={(checked) => setAudioSources({ ...audioSources, systemEnabled: checked })}
            aria-label="Записувати звук дзвінка"
          />
        </SettingsRow>
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
