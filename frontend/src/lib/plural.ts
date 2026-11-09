/** Ukrainian plural: 1 слово, 2–4 слова, 5+ слів (11–14 too) */
export function plural(n: number, one: string, few: string, many: string): string {
  const d = n % 10;
  const t = n % 100;
  if (d === 1 && t !== 11) return one;
  if (d >= 2 && d <= 4 && (t < 12 || t > 14)) return few;
  return many;
}
