import { toast } from '@/lib/notice';

/**
 * At the start of a recording, reminds to tell the participants that it is
 * being recorded (Settings → Запис → «Нагадування про запис», on by default).
 * One glass notice with «Більше не показувати».
 */
export async function showRecordingNotification(): Promise<void> {
  try {
    const { Store } = await import('@tauri-apps/plugin-store');
    const store = await Store.load('preferences.json');
    const showNotification = (await store.get<boolean>('show_recording_notification')) ?? true;
    if (!showNotification) return;

    toast.info('Запис почато', {
      description: 'Повідомте учасників про запис.',
      duration: 8000,
      cancel: {
        label: 'Більше не показувати',
        onClick: async () => {
          try {
            await store.set('show_recording_notification', false);
            await store.save();
          } catch (error) {
            console.error('Failed to save the recording reminder setting:', error);
          }
        },
      },
    });
  } catch (notificationError) {
    console.error('Failed to show recording notification:', notificationError);
    // Don't fail the recording if the reminder fails
  }
}
