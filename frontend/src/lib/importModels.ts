import { invoke } from '@tauri-apps/api/core';
import { toast } from '@/lib/notice';

/** «Моделі з папки…»: for Macs without the internet. True when something was installed or already there. */
export async function importModelsFromFolder(): Promise<boolean> {
  const folder = await invoke<string | null>('api_pick_folder', { title: 'Папка з моделями «Протоколу»' }).catch(() => null);
  if (!folder) return false;
  const id = toast.loading('Копіювання моделей…');
  try {
    const r = await invoke<{ copied: string[]; skipped: string[] }>('api_import_models', { folder });
    toast.success(`Моделі встановлено: ${r.copied.length}${r.skipped.length ? `, вже були: ${r.skipped.length}` : ''}`, { id });
    return true;
  } catch (e) {
    toast.error(`${e}`, { id, duration: 15000 });
    return false;
  }
}
