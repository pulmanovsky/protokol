export interface LanguageOption {
  code: string;
  label: string;
}

/**
 * Language options offered in the summary language pickers.
 * Codes must stay in sync with `language_name_from_code` in
 * `frontend/src-tauri/src/summary/processor.rs`.
 */
export const LANGUAGE_OPTIONS: LanguageOption[] = [
  { code: 'en', label: 'Англійська' },
  { code: 'zh', label: 'Китайська' },
  { code: 'zh-tw', label: 'Китайська (традиційна)' },
  { code: 'de', label: 'Німецька' },
  { code: 'es', label: 'Іспанська' },
  { code: 'ru', label: 'Російська' },
  { code: 'ko', label: 'Корейська' },
  { code: 'fr', label: 'Французька' },
  { code: 'ja', label: 'Японська' },
  { code: 'pt', label: 'Португальська' },
  { code: 'it', label: 'Італійська' },
  { code: 'nl', label: 'Нідерландська' },
  { code: 'pl', label: 'Польська' },
  { code: 'ar', label: 'Арабська' },
  { code: 'hi', label: 'Гінді' },
  { code: 'ta', label: 'Тамільська' },
  { code: 'tr', label: 'Турецька' },
  { code: 'vi', label: 'В\'єтнамська' },
  { code: 'th', label: 'Тайська' },
  { code: 'id', label: 'Індонезійська' },
  { code: 'sv', label: 'Шведська' },
  { code: 'cs', label: 'Чеська' },
  { code: 'da', label: 'Данська' },
  { code: 'fi', label: 'Фінська' },
  { code: 'el', label: 'Грецька' },
  { code: 'he', label: 'Іврит' },
  { code: 'hu', label: 'Угорська' },
  { code: 'no', label: 'Норвезька' },
  { code: 'ro', label: 'Румунська' },
  { code: 'uk', label: 'Українська' },
];

export const AUTO_VALUE = '__auto__' as const;

const SUPPORTED_CODES: ReadonlySet<string> = new Set(LANGUAGE_OPTIONS.map((o) => o.code));

/**
 * Normalises a raw locale string (from transcription or storage) into a code we
 * can translate into. Handles BCP-47 regional tags: `pt-BR` -> `pt`, `en_GB` -> `en`.
 * Returns null for unsupported languages so callers can fall back to English
 * rather than sending a code Rust will silently drop.
 */
export function normaliseLanguageCode(raw: string | null | undefined): string | null {
  if (!raw) return null;
  const lower = raw.toLowerCase().replace(/_/g, '-');
  if (SUPPORTED_CODES.has(lower)) return lower;
  const base = lower.split('-')[0];
  if (SUPPORTED_CODES.has(base)) return base;
  return null;
}

export function labelForCode(code: string): string {
  return LANGUAGE_OPTIONS.find((l) => l.code === code)?.label ?? code;
}
