'use client';

import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';

const MENU_W = 112;
const ITEM_H = 28;

/**
 * The playback speed button and its menu. The menu is drawn over the whole
 * window (not inside the timeline, which may be too low to hold it), above
 * the button, or below it when there is no room; kept inside the window.
 */
export function RateMenu({ rate, rates, onRate }: { rate: number; rates: number[]; onRate: (r: number) => void }) {
  const [open, setOpen] = useState(false);
  const [pos, setPos] = useState<{ left: number; top?: number; bottom?: number } | null>(null);
  const [active, setActive] = useState(0);
  const button = useRef<HTMLButtonElement>(null);
  const menu = useRef<HTMLDivElement>(null);
  const label = (r: number) => `${String(r).replace('.', ',')}×`;

  const place = () => {
    const b = button.current?.getBoundingClientRect();
    if (!b) return;
    const h = rates.length * ITEM_H + 10;
    const left = Math.max(8, Math.min(b.left, window.innerWidth - MENU_W - 8));
    if (b.top - h - 6 >= 8) setPos({ left, bottom: window.innerHeight - b.top + 6 });
    else setPos({ left, top: Math.min(b.bottom + 6, window.innerHeight - h - 8) });
  };
  useLayoutEffect(() => {
    if (!open) return;
    place();
    setActive(Math.max(0, rates.indexOf(rate)));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);
  useEffect(() => {
    if (!open) return;
    const outside = (e: MouseEvent) => {
      const t = e.target as Node;
      if (!menu.current?.contains(t) && !button.current?.contains(t)) setOpen(false);
    };
    const keep = () => place();
    window.addEventListener('mousedown', outside, true);
    window.addEventListener('resize', keep);
    window.addEventListener('scroll', keep, true);
    return () => {
      window.removeEventListener('mousedown', outside, true);
      window.removeEventListener('resize', keep);
      window.removeEventListener('scroll', keep, true);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);
  useEffect(() => {
    if (open && pos) menu.current?.querySelectorAll<HTMLButtonElement>('button')[active]?.focus();
  }, [open, pos, active]);

  const choose = (r: number) => {
    onRate(r);
    setOpen(false);
    button.current?.focus();
  };
  const onKey = (e: React.KeyboardEvent) => {
    if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      setOpen(false);
      button.current?.focus();
    } else if (e.key === 'ArrowDown') {
      e.preventDefault();
      setActive((a) => (a + 1) % rates.length);
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      setActive((a) => (a - 1 + rates.length) % rates.length);
    } else if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault();
      choose(rates[active]);
    }
  };

  return (
    <>
      <button
        ref={button}
        type="button"
        className="tl-btn"
        aria-label="Швидкість"
        aria-haspopup="menu"
        aria-expanded={open}
        title="Швидкість · ⌘⇧. швидше, ⌘⇧, повільніше"
        onClick={() => setOpen((o) => !o)}
        onKeyDown={(e) => {
          if (!open && (e.key === 'ArrowUp' || e.key === 'ArrowDown')) {
            e.preventDefault();
            setOpen(true);
          }
        }}
      >
        {label(rate)}
      </button>
      {open &&
        pos &&
        createPortal(
          <div ref={menu} className="tl-rate-menu" role="menu" aria-label="Швидкість" style={{ position: 'fixed', left: pos.left, top: pos.top, bottom: pos.bottom, width: MENU_W }} onKeyDown={onKey}>
            {rates.map((r, i) => (
              <button key={r} type="button" role="menuitemradio" aria-checked={r === rate} tabIndex={i === active ? 0 : -1} onMouseEnter={() => setActive(i)} onClick={() => choose(r)}>
                <span>{label(r)}</span>
                <span>{r === rate ? '✓' : ''}</span>
              </button>
            ))}
          </div>,
          document.body,
        )}
    </>
  );
}
