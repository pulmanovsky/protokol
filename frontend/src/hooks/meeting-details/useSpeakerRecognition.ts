import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen, UnlistenFn } from '@tauri-apps/api/event';
import { toast } from '@/lib/notice';
import { OPEN_SPEAKERS_EVENT } from '@/lib/timeline';

const MODELS_PROMPTED_KEY = 'speakerModelsPrompted';

/**
 * Speaker recognition that runs on its own after a recording or an import:
 * starts in the background when the meeting is new and has no speakers yet,
 * reports progress, and refreshes the transcript when done. Asks once to
 * download the speaker models (33 MB) if they are missing.
 */
export function useSpeakerRecognition(meetingId: string | undefined, onDone: () => Promise<void> | void) {
  const [progress, setProgress] = useState<number | null>(null);
  // Minutes left, from how fast it has gone so far
  const startedRef = useRef<number | null>(null);
  const [minutesLeft, setMinutesLeft] = useState<number | null>(null);
  const [finishedAt, setFinishedAt] = useState(0);
  const onDoneRef = useRef(onDone);
  onDoneRef.current = onDone;

  const start = useCallback(async () => {
    if (!meetingId) return;
    setProgress(0);
    try {
      await invoke('start_speaker_recognition', { meetingId });
    } catch (e) {
      setProgress(null);
      console.warn('Speaker recognition not started', e);
    }
  }, [meetingId]);

  useEffect(() => {
    if (!meetingId) return;
    const unlisteners: UnlistenFn[] = [];
    listen<{ meeting_id: string; progress_percentage: number }>('speakers-progress', (e) => {
      if (e.payload.meeting_id !== meetingId) return;
      const pct = e.payload.progress_percentage;
      setProgress(pct);
      if (startedRef.current === null) startedRef.current = Date.now();
      const elapsed = (Date.now() - startedRef.current) / 60000;
      setMinutesLeft(pct >= 20 && pct < 100 ? Math.max(1, Math.round((elapsed * (100 - pct)) / pct)) : null);
    }).then((u) => unlisteners.push(u));
    listen<{ meeting_id: string; ok: boolean; error?: string; speakers: number; suggested: number }>('speakers-done', async (e) => {
      if (e.payload.meeting_id !== meetingId) return;
      setProgress(null);
      setMinutesLeft(null);
      startedRef.current = null;
      if (e.payload.ok) {
        const hint = e.payload.suggested > 0 ? `, підказано імен: ${e.payload.suggested}` : '';
        toast.success(`Розпізнано голосів: ${e.payload.speakers}${hint}`, {
          duration: 15000,
          action: { label: 'Переглянути', onClick: () => window.dispatchEvent(new Event(OPEN_SPEAKERS_EVENT)) },
        });
        await onDoneRef.current();
        setFinishedAt(Date.now());
      } else if (e.payload.error === 'Зупинено') {
        toast.info('Розпізнавання спікерів зупинено. Запустити знову можна у «Спікери».');
      } else {
        toast.error(`Не вдалося розпізнати спікерів: ${e.payload.error ?? ''}`);
      }
    }).then((u) => unlisteners.push(u));

    (async () => {
      if (await invoke<boolean>('speaker_recognition_running', { meetingId }).catch(() => false)) {
        setProgress(0);
        return;
      }
      const status = await invoke<string>('speaker_auto_status', { meetingId }).catch(() => 'skip');
      if (status === 'run') {
        await start();
      } else if (status === 'needs_models') {
        let prompted = false;
        try {
          prompted = localStorage.getItem(MODELS_PROMPTED_KEY) === '1';
          localStorage.setItem(MODELS_PROMPTED_KEY, '1');
        } catch {
          prompted = false;
        }
        if (!prompted) {
          toast('Розрізняти спікерів за голосом?', {
            description: 'Потрібні дві невеликі моделі (33 МБ), один раз. Далі все працює без інтернету.',
            duration: 20000,
            action: {
              label: 'Завантажити',
              onClick: async () => {
                try {
                  await invoke('download_speaker_models');
                  await start();
                } catch (err) {
                  toast.error(`Не вдалося завантажити моделі: ${err}`);
                }
              },
            },
          });
        }
      }
    })();
    return () => unlisteners.forEach((u) => u());
  }, [meetingId, start]);

  const stop = useCallback(async () => {
    if (!meetingId) return;
    await invoke('stop_speaker_recognition', { meetingId }).catch(() => {});
  }, [meetingId]);

  return { progress, minutesLeft, running: progress !== null, finishedAt, start, stop };
}
