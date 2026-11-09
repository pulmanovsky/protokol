"use client"

import { useEffect, useState, useRef } from "react"
import { Switch } from "./ui/switch"
import { FolderOpen } from "lucide-react"
import { invoke } from "@tauri-apps/api/core"
import Analytics from "@/lib/analytics"
import { SettingsGroup, SettingsRow } from "./settings/SettingsGroup"
import { clickPlaysWord, setClickPlaysWord } from "@/lib/clickWord"
import { WORD_HIGHLIGHT_EVENT } from "@/lib/lineTimes"
import { UpdateSettings } from "./settings/UpdateSettings"
import { importModelsFromFolder } from "@/lib/importModels"
import { useConfig, NotificationSettings } from "@/contexts/ConfigContext"

/** «Підсвічувати кожне слово»: the default depends on the Mac's memory; the choice is kept */
function ClickWordRow() {
  const [on, setOn] = useState(true)
  useEffect(() => setOn(clickPlaysWord()), [])
  return (
    <SettingsRow
      label={
        <span className="flex flex-col min-w-0">
          <span>Клік по слову — прослухати</span>
          <span className="text-xs text-[color:var(--muted-text)]">Звучить саме це слово голосом того, хто його сказав. Вимкніть, щоб клік лише ставив курсор і переводив запис на це місце.</span>
        </span>
      }
    >
      <Switch
        checked={on}
        onCheckedChange={(v) => {
          setOn(v)
          setClickPlaysWord(v)
        }}
        aria-label="Клік по слову — прослухати"
      />
    </SettingsRow>
  )
}

function WordHighlightRow() {
  const [on, setOn] = useState<boolean | null>(null)
  useEffect(() => {
    invoke<boolean>("api_get_word_highlight").then(setOn).catch(() => setOn(null))
  }, [])
  const change = async (value: boolean) => {
    setOn(value)
    try {
      await invoke("api_set_word_highlight", { on: value })
      window.dispatchEvent(new CustomEvent(WORD_HIGHLIGHT_EVENT, { detail: value }))
    } catch {
      setOn(!value)
    }
  }
  return (
    <SettingsRow
      label={
        <span className="flex flex-col min-w-0">
          <span>Підсвічувати кожне слово</span>
          <span className="text-xs text-[color:var(--muted-text)]">Вирівнює слова по звуку. Вимкніть, якщо Mac повільний: тоді підсвічується весь абзац.</span>
        </span>
      }
    >
      <Switch checked={!!on} disabled={on === null} onCheckedChange={(v) => void change(v)} aria-label="Підсвічувати кожне слово" />
    </SettingsRow>
  )
}

export function PreferenceSettings() {
  const {
    notificationSettings,
    storageLocations,
    isLoadingPreferences,
    loadPreferences,
    updateNotificationSettings
  } = useConfig();

  const [notificationsEnabled, setNotificationsEnabled] = useState<boolean | null>(null);
  const [isInitialLoad, setIsInitialLoad] = useState(true);
  const [previousNotificationsEnabled, setPreviousNotificationsEnabled] = useState<boolean | null>(null);
  const hasTrackedViewRef = useRef(false);

  // Lazy load preferences on mount (only loads if not already cached)
  useEffect(() => {
    loadPreferences();
    // Reset tracking ref on mount (every tab visit)
    hasTrackedViewRef.current = false;
  }, [loadPreferences]);

  // Track preferences viewed analytics on every tab visit (once per mount)
  useEffect(() => {
    if (hasTrackedViewRef.current) return;

    const trackPreferencesViewed = async () => {
      // Wait for notification settings to be available (either from cache or after loading)
      if (notificationSettings) {
        await Analytics.track('preferences_viewed', {
          notifications_enabled: notificationSettings.notification_preferences.show_recording_started ? 'true' : 'false'
        });
        hasTrackedViewRef.current = true;
      } else if (!isLoadingPreferences) {
        // If not loading and no settings available, track with default value
        await Analytics.track('preferences_viewed', {
          notifications_enabled: 'false'
        });
        hasTrackedViewRef.current = true;
      }
    };

    trackPreferencesViewed();
  }, [notificationSettings, isLoadingPreferences]);

  // Update notificationsEnabled when notificationSettings are loaded from global state
  useEffect(() => {
    if (notificationSettings) {
      // Notification enabled means both started and stopped notifications are enabled
      const enabled =
        notificationSettings.notification_preferences.show_recording_started &&
        notificationSettings.notification_preferences.show_recording_stopped;
      setNotificationsEnabled(enabled);
      if (isInitialLoad) {
        setPreviousNotificationsEnabled(enabled);
        setIsInitialLoad(false);
      }
    } else if (!isLoadingPreferences) {
      // If not loading and no settings, use default
      setNotificationsEnabled(true);
      if (isInitialLoad) {
        setPreviousNotificationsEnabled(true);
        setIsInitialLoad(false);
      }
    }
  }, [notificationSettings, isLoadingPreferences, isInitialLoad])

  useEffect(() => {
    // Skip update on initial load or if value hasn't actually changed
    if (isInitialLoad || notificationsEnabled === null || notificationsEnabled === previousNotificationsEnabled) return;
    if (!notificationSettings) return;

    const handleUpdateNotificationSettings = async () => {
      console.log("Updating notification settings to:", notificationsEnabled);

      try {
        // Update the notification preferences
        const updatedSettings: NotificationSettings = {
          ...notificationSettings,
          notification_preferences: {
            ...notificationSettings.notification_preferences,
            show_recording_started: notificationsEnabled,
            show_recording_stopped: notificationsEnabled,
          }
        };

        console.log("Calling updateNotificationSettings with:", updatedSettings);
        await updateNotificationSettings(updatedSettings);
        setPreviousNotificationsEnabled(notificationsEnabled);
        console.log("Successfully updated notification settings to:", notificationsEnabled);

        // Track notification preference change - only fires when user manually toggles
        await Analytics.track('notification_settings_changed', {
          notifications_enabled: notificationsEnabled.toString()
        });
      } catch (error) {
        console.error('Failed to update notification settings:', error);
      }
    };

    handleUpdateNotificationSettings();
  }, [notificationsEnabled, notificationSettings, isInitialLoad, previousNotificationsEnabled, updateNotificationSettings])

  const handleOpenFolder = async (folderType: 'database' | 'models' | 'recordings') => {
    try {
      switch (folderType) {
        case 'database':
          await invoke('open_database_folder');
          break;
        case 'models':
          await invoke('open_models_folder');
          break;
        case 'recordings':
          await invoke('open_recordings_folder');
          break;
      }

      // Track storage folder access
      await Analytics.track('storage_folder_opened', {
        folder_type: folderType
      });
    } catch (error) {
      console.error(`Failed to open ${folderType} folder:`, error);
    }
  };

  // Show loading only if we're actually loading and don't have cached data
  if (isLoadingPreferences && !notificationSettings && !storageLocations) {
    return <div className="max-w-2xl mx-auto p-6">Завантаження…</div>
  }

  // Show loading if notificationsEnabled hasn't been determined yet
  if (notificationsEnabled === null && !isLoadingPreferences) {
    return <div className="max-w-2xl mx-auto p-6">Завантаження…</div>
  }

  // Ensure we have a boolean value for the Switch component
  const notificationsEnabledValue = notificationsEnabled ?? false;

  return (
    <div className="flex flex-col gap-6">
      <SettingsGroup title="Стенограма">
        <WordHighlightRow />
        <ClickWordRow />
      </SettingsGroup>

      <UpdateSettings />

      <SettingsGroup title="Сповіщення">
        <SettingsRow label="Сповіщати про початок і завершення запису">
          <Switch checked={notificationsEnabledValue} onCheckedChange={setNotificationsEnabled} />
        </SettingsRow>
      </SettingsGroup>

      <SettingsGroup
        title="Дані"
        help="База даних і моделі зберігаються разом у папці даних застосунку."
      >
        <SettingsRow
          label={
            <span className="flex flex-col min-w-0">
              <span>Папка записів</span>
              <span className="text-xs text-[color:var(--muted-text)] truncate" title={storageLocations?.recordings}>
                {storageLocations?.recordings || 'Завантаження…'}
              </span>
            </span>
          }
        >
          <button type="button" onClick={() => handleOpenFolder('recordings')} className="settings-button">
            <FolderOpen className="w-3.5 h-3.5" />
            Відкрити
          </button>
        </SettingsRow>
        <SettingsRow
          label={
            <span className="flex flex-col min-w-0">
              <span>Моделі з папки</span>
              <span className="text-xs text-[color:var(--muted-text)]">Для Mac без інтернету: моделі з флешки чи спільної папки</span>
            </span>
          }
        >
          <button type="button" onClick={() => void importModelsFromFolder()} className="settings-button">
            <FolderOpen className="w-3.5 h-3.5" />
            Вибрати…
          </button>
        </SettingsRow>
      </SettingsGroup>

    </div>
  )
}
