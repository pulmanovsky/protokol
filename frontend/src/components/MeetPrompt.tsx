'use client';

import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { usePathname, useRouter } from 'next/navigation';
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog';
import { PENDING_MEETING_TITLE_KEY } from '@/components/CalendarReminder';
import type { MeetPrompt as Prompt } from '@/lib/meet';

/** The texts of the offer, shared by this dialog and the corner panel */
export function promptTexts(prompt: Prompt): { title: string; body: string; yes: string; no: string } {
  if (prompt.kind === 'start') {
    return {
      title: 'Протокол',
      body: 'Нарада в Google Meet почалась, а запис не йде.',
      yes: 'Почати запис',
      no: 'Не цього разу',
    };
  }
  return {
    title: 'Нарада в Google Meet завершилась',
    body: prompt.title ? `Завершити запис «${prompt.title}»? Обробка почнеться одразу.` : 'Завершити запис? Обробка почнеться одразу.',
    yes: 'Завершити запис',
    no: 'Продовжити запис',
  };
}

export function answerPrompt(answer: 'start' | 'stop' | 'no') {
  invoke('meet_prompt_answer', { answer }).catch((e) => console.error('Meet offer answer failed', e));
}

/**
 * In the main window: the offer to start recording when a call in the Meet
 * window starts unrecorded, or to end the recording when the call ends.
 * Nothing starts or stops unless a button is pressed. Also starts the
 * recording when «Почати запис» is pressed in the corner panel.
 */
export function MeetPrompt() {
  const [prompt, setPrompt] = useState<Prompt | null>(null);
  // The same offer is under the Meet window's toolbar: shown in one place at a time
  const [inMeet, setInMeet] = useState(false);
  const pathname = usePathname();
  const router = useRouter();

  useEffect(() => {
    invoke<Prompt | null>('meet_prompt_current')
      .then((p) => setPrompt(p ?? null))
      .catch(() => {});
    invoke<boolean>('meet_pop_shown')
      .then((v) => setInMeet(v === true))
      .catch(() => {});
    const subs = [
      listen<Prompt | null>('meet-prompt', (e) => setPrompt(e.payload ?? null)),
      listen<boolean>('meet-pop-shown', (e) => setInMeet(e.payload === true)),
    ];
    return () => {
      subs.forEach((p) => p.then((f) => f()));
    };
  }, []);

  // «Почати запис»: the same start as from the calendar reminder, named after
  // the calendar event the Meet window was opened for
  useEffect(() => {
    const un = listen<{ title: string | null }>('meet-start-recording', (e) => {
      if (e.payload?.title) sessionStorage.setItem(PENDING_MEETING_TITLE_KEY, e.payload.title);
      if (pathname === '/') {
        window.dispatchEvent(new CustomEvent('start-recording-from-sidebar'));
      } else {
        sessionStorage.setItem('autoStartRecording', 'true');
        router.push('/');
      }
    });
    return () => {
      un.then((f) => f());
    };
  }, [pathname, router]);

  if (!prompt || inMeet) return null;
  const t = promptTexts(prompt);
  const yes = prompt.kind === 'start' ? 'start' : 'stop';

  return (
    <Dialog open onOpenChange={(open) => !open && answerPrompt('no')}>
      <DialogContent className="max-w-[420px] gap-3 rounded-[24px] p-[18px] pb-4">
        <DialogHeader className="text-left space-y-1 pr-6">
          <DialogTitle className="text-[16px] font-bold text-gray-900">{prompt.kind === 'start' ? 'Нарада в Google Meet почалась' : t.title}</DialogTitle>
          <DialogDescription className="text-[13.5px] leading-[1.45] text-gray-700">
            {prompt.kind === 'start' ? 'Запис не йде. Почати запис?' : t.body}
          </DialogDescription>
        </DialogHeader>
        <div className="flex justify-end gap-2">
          <button type="button" onClick={() => answerPrompt('no')} className="sheet-btn">
            {t.no}
          </button>
          <button type="button" onClick={() => answerPrompt(yes)} className="sheet-btn sheet-btn-red">
            {prompt.kind === 'start' && <i aria-hidden="true" />}
            {t.yes}
          </button>
        </div>
      </DialogContent>
    </Dialog>
  );
}
