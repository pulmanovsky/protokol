import { invoke } from '@tauri-apps/api/core';

/** A native «are you sure?»: `window.confirm` shows nothing in the app's web view (and says yes) */
export async function confirmNative(title: string, message: string, ok: string, cancel = 'Скасувати'): Promise<boolean> {
  try {
    return await invoke<boolean>('api_confirm', { title, message, ok, cancel });
  } catch {
    return false;
  }
}
