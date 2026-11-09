/** Marks of where a point of the document comes from: ⟨02:14⟩, ⟨02:14 · 12:06⟩, ⟨?⟩ (none found).
 * Shown in the app (a click opens «Джерело пункту»), left out of copies and exports. */
export const NO_SOURCE = '⟨?⟩';

export function withoutSourceMarks(text: string): string {
  return text.replace(/\s?⟨[^⟩]*⟩/g, '');
}

/** Points the app could not find in the speaker's words */
export function countNoSource(markdown: string): number {
  return markdown.split(NO_SOURCE).length - 1;
}
