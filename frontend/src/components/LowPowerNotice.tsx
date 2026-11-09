'use client';

import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { BatteryLow } from 'lucide-react';

const CHECK_INTERVAL_MS = 30_000;

/** Warns that macOS Low Power Mode slows down local protocol generation */
export function LowPowerNotice({ className = '' }: { className?: string }) {
  const [lowPower, setLowPower] = useState(false);

  useEffect(() => {
    let cancelled = false;
    const check = () =>
      invoke<boolean>('api_is_low_power_mode')
        .then((on) => !cancelled && setLowPower(on))
        .catch(() => {});
    check();
    const timer = setInterval(check, CHECK_INTERVAL_MS);
    window.addEventListener('focus', check);
    return () => {
      cancelled = true;
      clearInterval(timer);
      window.removeEventListener('focus', check);
    };
  }, []);

  if (!lowPower) return null;

  return (
    <div
      role="status"
      className={`flex gap-2 items-start text-[13px] leading-[1.45] text-gray-900 ${className}`}
    >
      <BatteryLow className="w-[15px] h-[15px] flex-none mt-[1px] text-[color:var(--muted-text)]" aria-hidden="true" />
      <span>
        Увімкнено режим енергозбереження: підсумок створюватиметься в 2–3 рази довше.{' '}
        <button
          type="button"
          onClick={() => invoke('api_open_battery_settings').catch(() => {})}
          className="underline underline-offset-2 hover:no-underline"
        >
          Системні параметри → Батарея
        </button>
      </span>
    </div>
  );
}
