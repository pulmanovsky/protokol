'use client';

import { useEffect } from 'react';
import { usePathname } from 'next/navigation';
import { invoke } from '@tauri-apps/api/core';
import { toast } from '@/lib/notice';
import { openFeedback } from '@/lib/feedback';
import { watchWindowErrors, type JournalMark } from '@/lib/journal';

/**
 * The main window's part of the journal: its errors go in, the page shown is
 * kept in the black box, a mark made anywhere (menu, menu bar icon, capsule)
 * is confirmed here, and after a run that ended with an error a report is
 * offered once.
 */
export function JournalBridge() {
  const pathname = usePathname();

  useEffect(() => watchWindowErrors('main window'), []);

  useEffect(() => {
    if (pathname) void invoke('api_journal_page', { path: pathname }).catch(() => {});
  }, [pathname]);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    void import('@tauri-apps/api/event').then(({ listen }) =>
      listen<JournalMark>('journal-mark', (e) => {
        toast.success(`Позначку №${e.payload.n} додано до журналу`, {
          description: 'Коли буде зручно, опишіть проблему і збережіть звіт.',
          duration: 8000,
          action: { label: 'Описати', onClick: () => openFeedback('problem') },
        });
      }).then((u) => (unlisten = u)),
    );
    return () => unlisten?.();
  }, []);

  useEffect(() => {
    const timer = setTimeout(() => {
      void invoke<string | null>('api_journal_last_crash')
        .then((crash) => {
          if (!crash) return;
          toast.warning('Минулого разу програма завершила роботу з помилкою', {
            description: 'Звіт про проблему допоможе знайти причину. Він зберігається на цьому Mac і нікуди не надсилається.',
            duration: 20000,
            action: { label: 'Зберегти звіт', onClick: () => openFeedback('problem') },
          });
        })
        .catch(() => {});
    }, 3000);
    return () => clearTimeout(timer);
  }, []);

  return null;
}

/** The small windows (recording panel, Meet toolbar): their errors only */
export function WindowErrors({ name }: { name: string }) {
  useEffect(() => watchWindowErrors(name), [name]);
  return null;
}
