'use client';

import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

/** Which sound sources the next recording uses */
export interface AudioSources {
  micEnabled: boolean;
  systemEnabled: boolean;
}

const STORAGE_KEY = 'protokol.audioSources';
const CHANGED_EVENT = 'protokol:audio-sources-changed';
const DEFAULT_SOURCES: AudioSources = { micEnabled: true, systemEnabled: true };

export function readStoredSources(): AudioSources {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return DEFAULT_SOURCES;
    const parsed = JSON.parse(raw) as Partial<AudioSources>;
    const sources = {
      micEnabled: parsed.micEnabled !== false,
      systemEnabled: parsed.systemEnabled !== false,
    };
    return sources.micEnabled || sources.systemEnabled ? sources : DEFAULT_SOURCES;
  } catch {
    return DEFAULT_SOURCES;
  }
}

/** Saves the choice and passes it to the recorder. At least one source stays on. */
export async function applySources(sources: AudioSources): Promise<void> {
  if (!sources.micEnabled && !sources.systemEnabled) {
    throw new Error('Увімкніть хоча б одне джерело звуку');
  }
  await invoke('set_audio_sources', {
    micEnabled: sources.micEnabled,
    systemEnabled: sources.systemEnabled,
  });
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(sources));
  } catch {
    // The choice still applies for this session
  }
  window.dispatchEvent(new CustomEvent<AudioSources>(CHANGED_EVENT, { detail: sources }));
}

/** Current source choice, shared by every picker on screen */
export function useAudioSources(): [AudioSources, (next: AudioSources) => Promise<void>] {
  const [sources, setSources] = useState<AudioSources>(DEFAULT_SOURCES);

  useEffect(() => {
    const stored = readStoredSources();
    setSources(stored);
    applySources(stored).catch((error) => console.error('Failed to apply audio sources', error));
    const onChange = (event: Event) => setSources((event as CustomEvent<AudioSources>).detail);
    window.addEventListener(CHANGED_EVENT, onChange);
    return () => window.removeEventListener(CHANGED_EVENT, onChange);
  }, []);

  return [sources, applySources];
}

/** In-app microphone mute (the device stays open; macOS volume is untouched) */
export function useMicMute(): [boolean, (muted: boolean) => Promise<void>] {
  const [muted, setMuted] = useState(false);

  useEffect(() => {
    invoke<boolean>('get_mic_muted').then(setMuted).catch(() => {});
    const unlisten = listen<{ muted: boolean }>('mic-mute-changed', (event) => setMuted(event.payload.muted));
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  const set = async (next: boolean) => {
    setMuted(next);
    try {
      await invoke('set_mic_muted', { muted: next });
    } catch (error) {
      console.error('Failed to change microphone mute', error);
      setMuted(!next);
    }
  };

  return [muted, set];
}

export function useCallMute(): [boolean, (muted: boolean) => Promise<void>] {
  const [muted, setMuted] = useState(false);

  useEffect(() => {
    invoke<boolean>('get_call_muted').then(setMuted).catch(() => {});
    const unlisten = listen<{ muted: boolean }>('call-mute-changed', (event) => setMuted(event.payload.muted));
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  const set = async (next: boolean) => {
    setMuted(next);
    try {
      await invoke('set_call_muted', { muted: next });
    } catch (error) {
      console.error('Failed to change call audio mute', error);
      setMuted(!next);
    }
  };

  return [muted, set];
}

/** dBFS for a 0..1 amplitude, floored at -60 */
export function toDb(amplitude: number): number {
  if (amplitude <= 0) return -60;
  return Math.max(-60, 20 * Math.log10(amplitude));
}

/** Meter position 0..100 % for a 0..1 amplitude (-60 dBFS .. 0 dBFS) */
export function meterPercent(amplitude: number): number {
  return ((toDb(amplitude) + 60) / 60) * 100;
}

/** Below about -50 dBFS counts as silence */
export const SILENCE_RMS = 0.003;
