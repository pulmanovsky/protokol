'use client';

import { useEffect } from 'react';
import { useRouter, usePathname } from 'next/navigation';
import { useSidebar } from '@/components/Sidebar/SidebarProvider';
import { applyTextSize, startRoute, stepTextSize } from '@/lib/uiPrefs';

/** The visible element among several matches (the transcript and the document both have a toolbar) */
function visible<T extends HTMLElement>(selector: string): T | null {
  const all = Array.from(document.querySelectorAll<T>(selector));
  return all.find((el) => el.offsetParent !== null && !(el as unknown as HTMLButtonElement).disabled) ?? null;
}

/**
 * Menu bar items that act on what is on screen (Правка → Знайти, Файл →
 * Експортувати документ, Вигляд → бічна панель). The menu sends
 * `protokol:menu` with the action's name.
 */
export function MenuBridge() {
  const { toggleCollapse } = useSidebar();
  const router = useRouter();
  const pathname = usePathname();
  // Вигляд → «Розмір тексту», kept from the last time
  useEffect(() => applyTextSize(), []);
  // Загальні → «Після запуску відкривати»: once, right after the app starts
  useEffect(() => {
    try {
      if (sessionStorage.getItem('protokol-started')) return;
      sessionStorage.setItem('protokol-started', '1');
    } catch {
      return;
    }
    const to = startRoute();
    if (to && pathname === '/') router.replace(to);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  useEffect(() => {
    const onMenu = (e: Event) => {
      const action = (e as CustomEvent<string>).detail;
      if (action === 'text-bigger' || action === 'text-smaller' || action === 'text-normal') {
        stepTextSize(action === 'text-bigger' ? 1 : action === 'text-smaller' ? -1 : 0);
      } else if (action === 'sidebar') {
        toggleCollapse();
      } else if (action === 'find') {
        // In an open recording: its search; elsewhere: the search of the recordings list
        const open = visible<HTMLInputElement>('.find-input');
        const find = visible<HTMLButtonElement>('.tv-find');
        if (open) {
          open.focus();
          open.select();
        } else if (find) find.click();
        else visible<HTMLInputElement>('input[aria-label="Пошук у записах"]')?.focus();
      } else if (action === 'export') {
        const exportButton = Array.from(document.querySelectorAll<HTMLButtonElement>('button')).find(
          (b) => b.offsetParent !== null && !b.disabled && b.textContent?.trim().startsWith('Експорт'),
        );
        exportButton?.click();
      }
    };
    window.addEventListener('protokol:menu', onMenu);
    return () => window.removeEventListener('protokol:menu', onMenu);
  }, [toggleCollapse]);
  return null;
}
