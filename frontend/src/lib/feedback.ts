/**
 * «Повідомити про проблему» / «Запропонувати покращення»: opened from the menu
 * bar (Довідка) and from Settings → Про програму. Nothing is sent by the app:
 * the person sees the whole text and opens a prefilled GitHub issue.
 */
export type FeedbackKind = 'problem' | 'idea';

export const FEEDBACK_EVENT = 'protokol:feedback';
export const FEEDBACK_REPO = 'https://github.com/pulmanovsky/protokol';

export function openFeedback(kind: FeedbackKind = 'problem') {
  window.dispatchEvent(new CustomEvent(FEEDBACK_EVENT, { detail: kind }));
}
