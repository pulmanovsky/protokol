import { useCallback, useEffect } from 'react';
import { useRouter } from 'next/navigation';
import { listen } from '@tauri-apps/api/event';
import { toast } from '@/lib/notice';
import type { TranscriptionErrorPayload } from '@/services/transcriptService';

export type ModalType = 'modelSelector' | 'errorAlert';

/**
 * Recording problems as glass notices (the old English modal windows are gone):
 * - no speech model → a notice with «Відкрити налаштування»;
 * - the recording stopped on an error → an error notice;
 * - `transcription-error` events from the Rust side.
 */
export function useModalState() {
  const router = useRouter();

  const showModal = useCallback(
    (name: ModalType, message?: string) => {
      if (name === 'modelSelector') {
        toast.error('Потрібна модель розпізнавання', {
          description: message && message !== 'Потрібна модель розпізнавання' ? message : 'Виберіть або завантажте модель у Налаштуваннях → Розпізнавання.',
          action: { label: 'Відкрити налаштування', onClick: () => router.push('/settings?section=transcription') },
          duration: 10000,
        });
      } else {
        toast.error('Запис зупинено', { description: message, duration: 10000 });
      }
    },
    [router],
  );

  useEffect(() => {
    const unlisten = listen<TranscriptionErrorPayload>('transcription-error', (event) => {
      const { userMessage, actionable } = event.payload;
      if (actionable) {
        showModal('modelSelector', userMessage);
      } else {
        toast.error('', { description: userMessage, duration: 8000 });
      }
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [showModal]);

  return { showModal };
}
