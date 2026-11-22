/**
 * «Повідомити про проблему» / «Запропонувати покращення»: opened from the menu
 * bar (Довідка) and from Settings → Про програму. Nothing is sent by the app.
 * A problem becomes a report saved on this Mac (components/ProblemReport.tsx)
 * that the person passes on themselves; a suggestion, or a public note about a
 * problem, opens a prefilled GitHub issue the person sees in full first.
 */
import { invoke } from '@tauri-apps/api/core';

export type FeedbackKind = 'problem' | 'idea';

export const FEEDBACK_EVENT = 'protokol:feedback';
export const FEEDBACK_REPO = 'https://github.com/pulmanovsky/protokol';

export function openFeedback(kind: FeedbackKind = 'problem') {
  window.dispatchEvent(new CustomEvent(FEEDBACK_EVENT, { detail: kind }));
}

/** «розпізнавання: Whisper large-v3 · документ: …», the models in use */
export async function readModels(): Promise<string> {
  const stt = await invoke<{ provider?: string; model?: string }>('api_get_transcript_config').catch(() => null);
  const llm = await invoke<{ model?: string }>('api_get_model_config').catch(() => null);
  return [stt?.model ? `розпізнавання: ${stt.provider === 'parakeet' ? 'Parakeet' : 'Whisper'} ${stt.model}` : '', llm?.model ? `документ: ${llm.model}` : '']
    .filter(Boolean)
    .join(' · ');
}
