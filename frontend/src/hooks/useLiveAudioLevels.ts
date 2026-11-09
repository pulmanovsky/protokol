'use client';

import { useEffect, useRef, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import { SILENCE_RMS } from '@/lib/audioSources';

interface LiveAudioLevelsEvent {
  mic_rms: number;
  mic_peak: number;
  system_rms: number;
  system_peak: number;
  mic_muted: boolean;
  mic_active: boolean;
  system_active: boolean;
}

export interface SourceLevel {
  rms: number;
  peak: number;
  /** Highest peak of the last ~1.5 s, for the peak-hold tick */
  peakHold: number;
  /** Seconds without sound (0 while sound is coming in) */
  silentFor: number;
}

export interface LiveAudioLevels {
  mic: SourceLevel;
  system: SourceLevel;
  micMuted: boolean;
}

const PEAK_HOLD_MS = 1500;
const EMPTY: SourceLevel = { rms: 0, peak: 0, peakHold: 0, silentFor: 0 };

/**
 * Real input levels from the recording pipeline (`live-audio-levels`, ~20 Hz).
 * Only receives data while a recording is running.
 */
export function useLiveAudioLevels(enabled: boolean): LiveAudioLevels {
  const [levels, setLevels] = useState<LiveAudioLevels>({ mic: EMPTY, system: EMPTY, micMuted: false });
  const track = useRef({
    mic: { holdValue: 0, holdAt: 0, soundAt: Date.now() },
    system: { holdValue: 0, holdAt: 0, soundAt: Date.now() },
  });

  useEffect(() => {
    if (!enabled) return;
    const now = Date.now();
    track.current.mic.soundAt = now;
    track.current.system.soundAt = now;

    const update = (key: 'mic' | 'system', rms: number, peak: number, at: number): SourceLevel => {
      const t = track.current[key];
      if (peak >= t.holdValue || at - t.holdAt > PEAK_HOLD_MS) {
        t.holdValue = peak;
        t.holdAt = at;
      }
      if (rms > SILENCE_RMS) t.soundAt = at;
      return { rms, peak, peakHold: t.holdValue, silentFor: (at - t.soundAt) / 1000 };
    };

    const unlisten = listen<LiveAudioLevelsEvent>('live-audio-levels', (event) => {
      const p = event.payload;
      const at = Date.now();
      setLevels({
        mic: update('mic', p.mic_rms, p.mic_peak, at),
        system: update('system', p.system_rms, p.system_peak, at),
        micMuted: p.mic_muted,
      });
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [enabled]);

  return levels;
}
