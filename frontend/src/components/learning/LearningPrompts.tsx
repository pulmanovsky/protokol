'use client';

import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { useRouter } from 'next/navigation';
import { ChevronDown } from 'lucide-react';
import { Dialog, DialogContent, DialogDescription, DialogTitle } from '@/components/ui/dialog';
import { DropdownMenu, DropdownMenuCheckboxItem, DropdownMenuContent, DropdownMenuTrigger } from '@/components/ui/dropdown-menu';
import { useRecordingState } from '@/contexts/RecordingStateContext';
import { toast } from '@/lib/notice';
import {
  MIN_GAIN,
  STATUS_CHANGED,
  Status,
  Wait,
  hoursText,
  installEngine,
  loadStatus,
  num,
  pct,
  previousChoice,
  slightlyBetter,
  statusChanged,
  waitText,
} from '@/lib/learning';

/** First look a little after launch, then now and then: checked speech grows as lines are read */
const FIRST_LOOK_MS = 6_000;
const LOOK_EVERY_MS = 15 * 60_000;

type When = 'night' | 'now';

/** «Донавчити вночі» or «зараз», as chosen in the offer (also from Settings → Пам'ять) */
export async function agreeToLearn(when: When, engineReady: boolean) {
  try {
    await invoke('api_learning_set_mode', { mode: 'nightly' });
  } catch (e) {
    toast.error(`${e}`);
    return;
  }
  statusChanged();
  if (!engineReady && !(await installEngine())) return;
  if (when === 'now') {
    await startNow();
  } else {
    toast.success('Навчання почнеться вночі', { description: 'Коли Mac підключено до живлення і ним не користуються.' });
  }
}

/** «Донавчити зараз» / «Почати» */
export async function startNow() {
  try {
    const wait = await invoke<Wait | null>('api_learning_start_now');
    if (wait) {
      toast.info('Навчання зараз не почнеться', { description: waitText(wait) });
    } else {
      toast.success('Навчання почалося', { description: "Триває 30–40 хвилин. Перебіг видно в Налаштуваннях → Пам'ять." });
    }
  } catch (e) {
    toast.error(`${e}`);
  }
  statusChanged();
}

/**
 * In the main window: the offer to fine-tune recognition once enough speech
 * is checked (asked once; «Не зараз» brings it back a week later), and the
 * notice of a run's result, shown until it is closed.
 */
export function LearningPrompts() {
  const router = useRouter();
  const { isRecording } = useRecordingState();
  const [status, setStatus] = useState<Status | null>(null);
  const [offerOpen, setOfferOpen] = useState(false);
  const [when, setWhen] = useState<When>('night');
  const shown = useRef(new Set<string>());

  const look = useCallback(() => {
    loadStatus()
      .then(setStatus)
      .catch(() => {});
  }, []);

  useEffect(() => {
    const first = setTimeout(look, FIRST_LOOK_MS);
    const every = setInterval(look, LOOK_EVERY_MS);
    window.addEventListener(STATUS_CHANGED, look);
    const un = listen('learning-finished', look);
    return () => {
      clearTimeout(first);
      clearInterval(every);
      window.removeEventListener(STATUS_CHANGED, look);
      un.then((f) => f());
    };
  }, [look]);

  // The offer: never over a recording
  useEffect(() => {
    if (status?.offer && !isRecording && !shown.current.has('offer')) {
      shown.current.add('offer');
      setWhen('night');
      setOfferOpen(true);
    }
  }, [status, isRecording]);

  // The result of a run, once per launch until closed
  useEffect(() => {
    const run = status?.unseen;
    if (!status || !run || offerOpen || shown.current.has(run.id)) return;
    shown.current.add(run.id);
    const seen = () => {
      invoke('api_learning_seen', { id: run.id }).catch(() => {});
    };
    if (run.outcome === 'accepted') {
      const back = previousChoice(status);
      toast.success('Розпізнавання покращено', {
        description: `На останніх нарадах помилок ${pct(run.wer_after)} замість ${pct(run.wer_before)}`,
        duration: Infinity,
        onDismiss: seen,
        action: {
          label: 'Повернути попередню',
          onClick: () => {
            invoke('api_learning_use', { choice: back })
              .then(() => {
                toast.success('Повернуто попередню модель', { description: 'Нові записи розпізнаватимуться нею.' });
                statusChanged();
              })
              .catch((e) => toast.error(`${e}`));
          },
        },
      });
    } else {
      const worseOnOne = run.reason === 'OneMeetingWorse';
      const tooLittle = run.reason === 'TooLittleToTell';
      const slight = slightlyBetter(run);
      const numbers = `${pct(run.wer_after)} помилок проти ${pct(run.wer_before)}`;
      const title = tooLittle
        ? 'Нову модель не прийнято'
        : worseOnOne
          ? 'Нова модель гірша на одній з нарад'
          : slight
            ? 'Нова модель недостатньо точна'
            : 'Нова модель не точніша';
      toast.info(title, {
        description: tooLittle
          ? 'Замало перевіреного мовлення для оцінки. Залишено поточну'
          : slight
            ? `Залишено поточну. На останніх нарадах ${numbers}, а для заміни помилок має бути менше щонайменше на ${num(MIN_GAIN)}%`
            : `Залишено поточну. ${worseOnOne ? 'Загалом' : 'На останніх нарадах'} ${numbers}`,
        duration: Infinity,
        onDismiss: seen,
        action: { label: 'Докладніше', onClick: () => router.push('/settings?section=memory&view=history') },
      });
    }
  }, [status, offerOpen, router]);

  const later = () => {
    setOfferOpen(false);
    invoke('api_learning_offer_later').catch(() => {});
  };

  const agree = () => {
    setOfferOpen(false);
    void agreeToLearn(when, !!status?.engine_ready);
  };

  if (!status) return null;

  return (
    <Dialog open={offerOpen} onOpenChange={(open) => !open && later()}>
      <DialogContent className="max-w-[460px] gap-3.5 rounded-[24px] p-6 pb-5">
        <DialogTitle className="text-[17px] font-bold leading-snug text-gray-900 pr-6">Донавчити розпізнавання на ваших нарадах?</DialogTitle>
        <DialogDescription className="text-[13.5px] leading-[1.5] text-gray-700">
          Набралося {hoursText(status.checked_hours)} перевіреного мовлення. На ньому можна донавчити модель розпізнавання, щоб у нових стенограмах
          було менше помилок.
        </DialogDescription>
        <ul className="flex flex-col gap-2 text-[13.5px] leading-[1.45] text-gray-700">
          {[
            'Навчання триває 30–40 хвилин. Його можна почати зараз або вночі, коли Mac підключено до живлення.',
            'Записи і тексти не залишають цей Mac.',
            'Нова модель вмикається, лише якщо на останніх нарадах вона точніша за поточну.',
          ].map((t) => (
            <li key={t} className="flex gap-2.5">
              <span className="mt-[7px] h-1.5 w-1.5 flex-none rounded-full bg-[var(--blue)]" aria-hidden="true" />
              <span>{t}</span>
            </li>
          ))}
        </ul>
        <p className="text-xs leading-[1.4] text-[color:var(--muted-text)]">
          {status.engine_ready ? '' : `Для навчання один раз завантажується модуль, близько ${status.engine_download_mb} МБ. `}
          Змінити рішення можна в Налаштуваннях → Пам&apos;ять.
        </p>
        <div className="mt-1 flex justify-end gap-2">
          <button type="button" className="sheet-btn" onClick={later}>
            Не зараз
          </button>
          <div className="learn-split" role="group" aria-label="Донавчити">
            <button type="button" className="learn-split-main" onClick={agree}>
              {when === 'night' ? 'Донавчити вночі' : 'Донавчити зараз'}
            </button>
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <button type="button" className="learn-split-chev" aria-label="Коли донавчити">
                  <ChevronDown className="h-3.5 w-3.5" aria-hidden="true" />
                </button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end" sideOffset={6} className="w-[300px] rounded-[10px] p-1">
                {(
                  [
                    ['night', 'Донавчити вночі', 'Коли Mac на живленні і ним не користуються'],
                    ['now', 'Донавчити зараз', 'Почнеться одразу, 30–40 хвилин'],
                  ] as const
                ).map(([value, label, sub]) => (
                  <DropdownMenuCheckboxItem key={value} className="items-start rounded-md py-1.5" checked={when === value} onSelect={() => setWhen(value)}>
                    <span className="flex flex-col gap-px">
                      <span className="text-[13px]">{label}</span>
                      <span className="text-[11px] opacity-70">{sub}</span>
                    </span>
                  </DropdownMenuCheckboxItem>
                ))}
              </DropdownMenuContent>
            </DropdownMenu>
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}
