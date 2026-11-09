'use client';

import { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { ChevronDown, Mic, MicOff, Volume2 } from 'lucide-react';
import { toast } from '@/lib/notice';
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover';
import { Switch } from '@/components/ui/switch';
import { useConfig } from '@/contexts/ConfigContext';
import { useAudioSources } from '@/lib/audioSources';
import { LevelMeter } from './LevelMeter';

interface AudioDevice {
  name: string;
  device_type: 'Input' | 'Output';
}

const OFF = '__off__';
const DEFAULT = '__default__';

/** Device name without the " (input)" suffix the preferences store */
function displayName(value: string | null | undefined): string {
  return value ? value.replace(/\s*\(input\)$/i, '') : '';
}

/** Saves the preferred microphone the same way Settings → Запис does */
async function savePreferredMic(micDevice: string | null) {
  const prefs = await invoke<Record<string, unknown>>('get_recording_preferences');
  await invoke('set_recording_preferences', { preferences: { ...prefs, preferred_mic_device: micDevice } });
}

/** Mic level preview, only while `active` (the picker is open and not recording) */
function useMicPreview(active: boolean, device: string | null) {
  const [level, setLevel] = useState({ rms: 0, peak: 0 });
  useEffect(() => {
    if (!active) return;
    let stopped = false;
    const unlisten = listen<{ rms: number; peak: number }>('mic-preview-level', (event) => {
      if (!stopped) setLevel(event.payload);
    });
    invoke('start_mic_preview', { deviceName: device }).catch((error) =>
      console.warn('Mic preview unavailable', error),
    );
    return () => {
      stopped = true;
      setLevel({ rms: 0, peak: 0 });
      invoke('stop_mic_preview').catch(() => {});
      unlisten.then((fn) => fn());
    };
  }, [active, device]);
  return level;
}

interface SourcePickerProps {
  /** Locked while a recording is running */
  disabled?: boolean;
  align?: 'start' | 'center' | 'end';
}

/** Compact sound source picker next to the record button */
export function SourcePicker({ disabled = false, align = 'end' }: SourcePickerProps) {
  const { selectedDevices, setSelectedDevices } = useConfig();
  const [sources, setSources] = useAudioSources();
  const [open, setOpen] = useState(false);
  const [inputs, setInputs] = useState<AudioDevice[]>([]);
  const peakHold = useRef({ value: 0, at: 0 });

  useEffect(() => {
    if (!open) return;
    invoke<AudioDevice[]>('get_audio_devices')
      .then((devices) => setInputs(devices.filter((d) => d.device_type === 'Input')))
      .catch((error) => console.error('Failed to list audio devices', error));
  }, [open]);

  const micDevice = selectedDevices?.micDevice ?? null;
  const preview = useMicPreview(open && !disabled && sources.micEnabled, micDevice);
  const now = Date.now();
  if (preview.peak >= peakHold.current.value || now - peakHold.current.at > 1500) {
    peakHold.current = { value: preview.peak, at: now };
  }

  const micValue = !sources.micEnabled ? OFF : micDevice ?? DEFAULT;
  const micLabel = !sources.micEnabled
    ? 'Мікрофон вимкнено'
    : displayName(micDevice) || 'Мікрофон Mac';

  const change = async (next: { micEnabled: boolean; systemEnabled: boolean }) => {
    if (!next.micEnabled && !next.systemEnabled) {
      toast.error('Потрібне хоча б одне джерело звуку', {
        description: 'Увімкніть мікрофон або звук дзвінка.',
      });
      return;
    }
    try {
      await setSources(next);
    } catch (error) {
      toast.error('Не вдалося змінити джерела звуку');
    }
  };

  const onMicChange = async (value: string) => {
    if (value === OFF) {
      await change({ micEnabled: false, systemEnabled: sources.systemEnabled });
      return;
    }
    const device = value === DEFAULT ? null : value;
    try {
      await savePreferredMic(device);
      setSelectedDevices({ micDevice: device, systemDevice: selectedDevices?.systemDevice ?? null });
    } catch (error) {
      console.error('Failed to save microphone', error);
      toast.error('Не вдалося зберегти мікрофон');
    }
    if (!sources.micEnabled) await change({ micEnabled: true, systemEnabled: sources.systemEnabled });
  };

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <button
          type="button"
          disabled={disabled}
          className="secondary-button h-11 px-3.5 gap-2 text-sm font-medium max-w-[260px] disabled:opacity-50"
          title="Джерела звуку"
        >
          {sources.micEnabled ? <Mic className="w-4 h-4 flex-none" /> : <MicOff className="w-4 h-4 flex-none" />}
          <span className="truncate">{micLabel}</span>
          {sources.systemEnabled && <Volume2 className="w-4 h-4 flex-none text-[color:var(--muted-text)]" aria-label="і звук дзвінка" />}
          <ChevronDown className="w-4 h-4 flex-none text-[color:var(--muted-text)]" />
        </button>
      </PopoverTrigger>
      <PopoverContent align={align} className="w-[320px] card p-4 flex flex-col gap-4">
        <h3 className="text-[15px] font-bold text-gray-900">Джерела звуку</h3>

        <label className="flex flex-col gap-1.5">
          <span className="text-[13px] font-semibold text-gray-900">Мікрофон</span>
          <select
            value={micValue}
            onChange={(e) => onMicChange(e.target.value)}
            className="h-9 rounded-lg border border-[var(--hairline,rgba(0,0,0,0.12))] bg-transparent px-2.5 text-[13px] text-gray-900"
          >
            <option value={DEFAULT}>За замовчуванням (системний)</option>
            {inputs.map((d) => (
              <option key={d.name} value={`${d.name} (input)`}>
                {d.name}
              </option>
            ))}
            <option value={OFF}>Вимкнено</option>
          </select>
          {sources.micEnabled && (
            <div className="flex flex-col gap-1 pt-1">
              <LevelMeter rms={preview.rms} peakHold={peakHold.current.value} label="Рівень мікрофона" />
              <span className="text-[12px] text-[color:var(--muted-text)]">
                Скажіть щось: смужка показує, що мікрофон вас чує.
              </span>
            </div>
          )}
        </label>

        <div className="flex items-start justify-between gap-3">
          <div className="flex flex-col gap-0.5">
            <span className="text-[13px] font-semibold text-gray-900">Звук дзвінка</span>
            <span className="text-[12px] text-[color:var(--muted-text)]">
              Весь звук Mac: співрозмовники в Zoom, Meet, Teams
            </span>
          </div>
          <Switch
            checked={sources.systemEnabled}
            onCheckedChange={(checked) => change({ micEnabled: sources.micEnabled, systemEnabled: checked })}
            aria-label="Записувати звук дзвінка"
          />
        </div>
      </PopoverContent>
    </Popover>
  );
}
