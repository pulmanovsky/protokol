import { invoke } from '@tauri-apps/api/core';
import { toast } from '@/lib/notice';
import { plural } from './plural';
import { mmss } from './timeline';
import { speakerParagraphs, type SpokenLine } from './speakerText';

/** Everything one speaker said, to the clipboard (with times when `withTimes`) */
export function copySpeakerText(lines: SpokenLine[], who: string, withTimes = false): void {
  const paragraphs = speakerParagraphs(lines, who, withTimes ? mmss : undefined);
  if (paragraphs.length === 0) {
    toast('Цей спікер нічого не сказав');
    return;
  }
  const text = paragraphs.join('\n\n');
  // By the system's own clipboard: the page's may refuse without a fresh click
  invoke('api_copy_text', { text })
    .catch(() => navigator.clipboard.writeText(text))
    .then(() => toast(`Скопійовано весь текст: ${who} (${paragraphs.length} ${plural(paragraphs.length, 'абзац', 'абзаци', 'абзаців')})`))
    .catch(() => toast.error('Не вдалося скопіювати текст'));
}
