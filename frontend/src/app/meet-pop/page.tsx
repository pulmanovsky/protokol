'use client';

import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { answerPrompt } from '@/components/MeetPrompt';
import { participantsText, useMeetLive, type MeetPrompt } from '@/lib/meet';

/**
 * The Google Meet offer under the capsule of the Meet window's toolbar
 * (meet/mod.rs, shown while that window is in front). Nothing starts or
 * stops unless a button is pressed; the offer goes away by itself once the
 * recording starts or ends.
 */
export default function MeetPop() {
  const [prompt, setPrompt] = useState<MeetPrompt | null>(null);
  const live = useMeetLive();

  useEffect(() => {
    document.documentElement.style.background = 'transparent';
    document.body.style.background = 'transparent';
    invoke<MeetPrompt | null>('meet_prompt_current')
      .then((p) => setPrompt(p ?? null))
      .catch(() => {});
    const un = listen<MeetPrompt | null>('meet-prompt', (e) => setPrompt(e.payload ?? null));
    return () => {
      un.then((f) => f());
    };
  }, []);

  if (!prompt) return null;
  const start = prompt.kind === 'start';
  const details = start
    ? [live.title, live.inCall ? participantsText(live.participants) : null].filter(Boolean).join(' · ')
    : prompt.title
      ? `Завершити запис «${prompt.title}»? Обробка почнеться одразу.`
      : 'Завершити запис? Обробка почнеться одразу.';

  return (
    <div className={`mpop ${start ? '' : 'mpop-stop'}`} role="alertdialog" aria-label={start ? 'Нарада почалась, а запис не йде' : 'Нарада в Meet завершилась'}>
      <p className="mpop-title">{start ? 'Нарада почалась, а запис не йде' : 'Нарада в Meet завершилась'}</p>
      {details && <p className="mpop-sub">{details}</p>}
      <div className="mpop-btns">
        <button type="button" className="mpop-btn" onClick={() => answerPrompt('no')}>
          {start ? 'Не цього разу' : 'Продовжити запис'}
        </button>
        <button type="button" className="mpop-btn mpop-red" onClick={() => answerPrompt(start ? 'start' : 'stop')}>
          {start && <i aria-hidden="true" />}
          {start ? 'Почати запис' : 'Завершити запис'}
        </button>
      </div>
    </div>
  );
}
