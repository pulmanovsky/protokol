'use client';

import { Sun, Moon, Monitor } from 'lucide-react';
import { useTheme, ThemeMode } from '@/contexts/ThemeContext';

const OPTIONS: { value: ThemeMode; label: string; icon: typeof Sun }[] = [
  { value: 'light', label: 'Світла', icon: Sun },
  { value: 'dark', label: 'Темна', icon: Moon },
  { value: 'system', label: 'Як у системі', icon: Monitor },
];

export function AppearanceSettings() {
  const { mode, setMode } = useTheme();

  return (
    <div className="flex flex-col gap-2">
      <div className="settings-list">
        <div className="settings-row">
          <span id="appearance-theme">Тема</span>
          <div role="radiogroup" aria-labelledby="appearance-theme" className="segmented flex gap-0.5">
            {OPTIONS.map(({ value, label, icon: Icon }) => (
              <button
                key={value}
                type="button"
                role="radio"
                aria-checked={mode === value}
                data-active={mode === value}
                onClick={() => setMode(value)}
                className="segmented-item h-7 px-3 gap-1.5 flex items-center text-[13px] font-medium"
              >
                <Icon className="w-3.5 h-3.5" />
                {label}
              </button>
            ))}
          </div>
        </div>
      </div>
      <p className="settings-help">
        «Як у системі» перемикає світлу й темну тему разом із macOS. Скло вікна теж підлаштовується під тему.
      </p>
    </div>
  );
}
