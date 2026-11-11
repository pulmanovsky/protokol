'use client';

import { Mic, MicOff } from 'lucide-react';

interface MuteButtonProps {
  muted: boolean;
  onToggle: () => void;
  disabled?: boolean;
  /** Listen for Cmd+Shift+M (only one button on screen should do this) */
  withShortcut?: boolean;
  compact?: boolean;
}

/** In-app microphone mute: the stream stays open, our pipeline writes silence */
export function MuteButton({ muted, onToggle, disabled = false, withShortcut = false, compact = false }: MuteButtonProps) {
  // ⇧⌘M is the menu bar's «Запис → Вимкнути або увімкнути мікрофон» (a key handler here would toggle twice)
  void withShortcut;

  const label = muted ? 'Увімкнути мікрофон' : 'Вимкнути мікрофон';
  return (
    <button
      type="button"
      onClick={onToggle}
      disabled={disabled}
      aria-pressed={muted}
      aria-label={`${label} (⇧⌘M)`}
      title={`${label} (⇧⌘M)`}
      className={`inline-flex items-center justify-center gap-2 rounded-full font-semibold transition-colors disabled:opacity-50 ${
        compact ? 'h-8 px-3 text-[13px]' : 'h-10 px-4 text-sm'
      } ${
        muted
          ? 'bg-[rgba(229,53,43,0.14)] text-[#B3261E] dark:text-[#FF8A80]'
          : 'secondary-button'
      }`}
    >
      {muted ? <MicOff className="w-4 h-4" /> : <Mic className="w-4 h-4" />}
      {muted ? 'Мікрофон вимкнено' : compact ? 'Мікрофон' : 'Вимкнути мікрофон'}
    </button>
  );
}
