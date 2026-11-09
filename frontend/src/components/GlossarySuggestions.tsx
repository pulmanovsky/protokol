'use client';

import { useCallback, useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { toast } from '@/lib/notice';

interface Suggestion {
  variants: string[];
  counts: number[];
  example: string;
}

/**
 * Names, nicknames and units from recent meetings that are not in the
 * glossary yet. One click confirms the right spelling: it joins the glossary
 * (Whisper writes it correctly next time) and the other spellings are
 * corrected before a summary is written.
 */
export function GlossarySuggestions({ onChanged }: { onChanged?: () => void }) {
  const [items, setItems] = useState<Suggestion[] | null>(null);
  const [editing, setEditing] = useState<Record<number, string>>({});

  const load = useCallback(() => {
    invoke<Suggestion[]>('api_glossary_suggestions')
      .then(setItems)
      .catch((error) => {
        console.error('Failed to load suggestions', error);
        setItems([]);
      });
  }, []);

  useEffect(load, [load]);

  const accept = async (right: string, variants: string[]) => {
    try {
      await invoke('api_accept_glossary_suggestion', { right, variants });
      toast.success(`«${right}» додано до словника`);
      onChanged?.();
      load();
    } catch (error) {
      toast.error('Не вдалося зберегти', { description: String(error) });
    }
  };

  const dismiss = async (variants: string[]) => {
    await invoke('api_dismiss_glossary_suggestion', { variants }).catch(() => {});
    load();
  };

  if (items === null) return null;

  return (
    <section className="card p-6 flex flex-col gap-3">
      <div className="flex flex-col gap-1">
        <h2 className="text-[15px] font-bold text-gray-900">Підказки з останніх записів</h2>
        <p className="text-sm text-[color:var(--muted-text)]">
          Імена й назви, яких ще немає в словнику. Якщо одне слово записане по-різному, оберіть правильний
          варіант: інші виправлятимуться перед підсумком, а розпізнавання наступного разу писатиме правильно.
        </p>
      </div>
      {items.length === 0 ? (
        <p className="text-sm text-[color:var(--muted-text)]">Нових підказок немає.</p>
      ) : (
        <ul className="flex flex-col divide-y divide-[var(--hairline,rgba(0,0,0,0.08))]">
          {items.map((item, index) => (
            <li key={item.variants.join('|')} className="py-3 flex flex-col gap-2">
              <div className="flex flex-wrap items-center gap-2">
                {item.variants.map((variant, i) => (
                  <button
                    key={variant}
                    type="button"
                    onClick={() => accept(variant, item.variants)}
                    className="secondary-button h-8 px-3 text-[13px] font-semibold"
                    title={item.variants.length > 1 ? `Правильно «${variant}»` : `Додати «${variant}» до словника`}
                  >
                    {variant}
                    <span className="ml-1.5 text-[color:var(--muted-text)] font-normal">×{item.counts[i]}</span>
                  </button>
                ))}
                {/* «Інакше пишеться» and «сховати» stay together when the row wraps */}
                <span className="ml-auto flex items-center gap-2">
                <input
                  value={editing[index] ?? ''}
                  onChange={(e) => setEditing((s) => ({ ...s, [index]: e.target.value }))}
                  onKeyDown={(e) => {
                    const value = (editing[index] ?? '').trim();
                    if (e.key === 'Enter' && value) accept(value, item.variants);
                  }}
                  placeholder="Інакше пишеться…"
                  className="h-8 w-[170px] rounded-lg border border-[var(--hairline,rgba(0,0,0,0.12))] bg-transparent px-2.5 text-[13px] text-gray-900"
                />
                <button
                  type="button"
                  onClick={() => dismiss(item.variants)}
                  className="h-8 px-2 whitespace-nowrap text-[13px] text-[color:var(--muted-text)] hover:underline"
                >
                  Не ім’я, сховати
                </button>
                </span>
              </div>
              {item.example && (
                <span className="text-[12px] text-[color:var(--muted-text)] truncate" title={item.example}>
                  …{item.example}…
                </span>
              )}
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
