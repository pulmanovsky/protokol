'use client';

import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { toast } from '@/lib/notice';

/**
 * Glossary of names, positions, abbreviations and terms. Speech recognition
 * uses it to spell them right, and the summary writer keeps that spelling.
 */
export function GlossarySettings() {
  const [glossaryText, setGlossaryText] = useState('');
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    invoke<string[]>('api_get_glossary')
      .then((terms) => setGlossaryText(terms.join('\n')))
      .catch((error) => console.error('Failed to load glossary', error));
  }, []);

  const save = async () => {
    setSaving(true);
    try {
      const saved = await invoke<string[]>('api_set_glossary', { terms: glossaryText.split('\n') });
      setGlossaryText(saved.join('\n'));
      toast.success('Словник збережено', { description: `${saved.length} термінів` });
    } catch (error) {
      toast.error('Не вдалося зберегти словник', { description: String(error) });
    } finally {
      setSaving(false);
    }
  };

  return (
    <section className="card p-6 flex flex-col gap-3">
      <p className="text-sm text-[color:var(--muted-text)]">
        Імена, посади, назви відділів, абревіатури та терміни, по одному в рядку. Розпізнавання мовлення пише їх
        правильно, а підсумок використовує саме таке написання.
      </p>
      <textarea
        value={glossaryText}
        onChange={(e) => setGlossaryText(e.target.value)}
        rows={14}
        placeholder={'Петренко Олег Миколайович\nкерівник офісу\nОДА\nСЕД «АСКОД»'}
        className="w-full rounded-xl border border-[var(--hairline,rgba(0,0,0,0.12))] bg-transparent p-3 text-sm font-mono text-gray-900 focus:outline-none"
      />
      <div className="flex justify-end">
        <button type="button" onClick={save} disabled={saving} className="accent-button h-10 px-4 text-sm font-semibold disabled:opacity-50">
          {saving ? 'Збереження…' : 'Зберегти словник'}
        </button>
      </div>
    </section>
  );
}
