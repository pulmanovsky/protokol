'use client';

import { ReactNode, useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { ChevronDown } from 'lucide-react';

export interface ToolbarMenuItem {
  label: string;
  disabled?: boolean;
  run: () => void;
}

/**
 * A toolbar button with a menu under it («Експорт ⌄»): the same look on the
 * transcript and on the document. ↑/↓ move in the menu, Esc closes it.
 */
export function ToolbarMenu({ label, icon, items, disabled }: { label: string; icon: ReactNode; items: ToolbarMenuItem[]; disabled?: boolean }) {
  const [open, setOpen] = useState(false);
  const [pos, setPos] = useState<{ top: number; right: number } | null>(null);
  const button = useRef<HTMLButtonElement>(null);
  const menu = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const b = button.current?.getBoundingClientRect();
    if (b) setPos({ top: Math.min(b.bottom + 6, window.innerHeight - items.length * 30 - 16), right: Math.max(8, window.innerWidth - b.right) });
    const outside = (e: MouseEvent) => {
      const t = e.target as Node;
      if (!menu.current?.contains(t) && !button.current?.contains(t)) setOpen(false);
    };
    const key = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        setOpen(false);
        button.current?.focus();
      }
    };
    window.addEventListener('mousedown', outside, true);
    window.addEventListener('keydown', key);
    return () => {
      window.removeEventListener('mousedown', outside, true);
      window.removeEventListener('keydown', key);
    };
  }, [open, items.length]);

  useEffect(() => {
    if (open && pos) menu.current?.querySelector<HTMLButtonElement>('button:not(:disabled)')?.focus();
  }, [open, pos]);

  return (
    <>
      <button ref={button} type="button" className="tb-btn" aria-haspopup="menu" aria-expanded={open} disabled={disabled} title={label} onClick={() => setOpen((o) => !o)}>
        {icon}
        <span className="tb-label">{label}</span>
        <ChevronDown className="tb-chevron h-3.5 w-3.5" aria-hidden="true" />
      </button>
      {open &&
        pos &&
        createPortal(
          <div
            ref={menu}
            role="menu"
            className="tl-rate-menu"
            style={{ position: 'fixed', top: pos.top, right: pos.right, minWidth: 220 }}
            onKeyDown={(e) => {
              if (e.key !== 'ArrowDown' && e.key !== 'ArrowUp') return;
              e.preventDefault();
              const all = [...(menu.current?.querySelectorAll<HTMLButtonElement>('button:not(:disabled)') ?? [])];
              const i = all.indexOf(document.activeElement as HTMLButtonElement);
              all[(i + (e.key === 'ArrowDown' ? 1 : all.length - 1)) % all.length]?.focus();
            }}
          >
            {items.map((it) => (
              <button
                key={it.label}
                type="button"
                role="menuitem"
                disabled={it.disabled}
                onClick={() => {
                  setOpen(false);
                  it.run();
                }}
              >
                <span>{it.label}</span>
              </button>
            ))}
          </div>,
          document.body,
        )}
    </>
  );
}
