'use client';

import { meterPercent } from '@/lib/audioSources';

interface LevelMeterProps {
  rms: number;
  peakHold?: number;
  /** Greyed out: the source is off or muted */
  dimmed?: boolean;
  label?: string;
}

/** Horizontal level meter on a dB scale (-60 … 0 dBFS) with a peak-hold tick */
export function LevelMeter({ rms, peakHold = 0, dimmed = false, label }: LevelMeterProps) {
  const level = dimmed ? 0 : meterPercent(rms);
  const hold = dimmed ? 0 : meterPercent(peakHold);
  const hot = hold > 95;

  return (
    <div
      className={`relative h-1.5 rounded-full bg-[var(--selected)] overflow-hidden ${dimmed ? 'opacity-50' : ''}`}
      role="meter"
      aria-label={label}
      aria-valuemin={-60}
      aria-valuemax={0}
      aria-valuenow={Math.round((level / 100) * 60 - 60)}
    >
      <div
        className="absolute inset-y-0 left-0 rounded-full bg-gray-900 transition-[width] duration-75 ease-out"
        style={{ width: `${level}%` }}
      />
      {hold > 1 && (
        <div
          className={`absolute inset-y-0 w-[2px] rounded-full ${hot ? 'bg-[#E5352B]' : 'bg-gray-500'}`}
          style={{ left: `calc(${hold}% - 2px)` }}
        />
      )}
    </div>
  );
}
