'use client';

import React, { useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { getVersion } from '@tauri-apps/api/app';
import { X } from 'lucide-react';
import { Dialog, DialogContent, DialogDescription, DialogTitle } from '@/components/ui/dialog';
import { SettingsGroup, SettingsRow } from '@/components/settings/SettingsGroup';
import { FEEDBACK_REPO } from '@/lib/feedback';
import { toast } from '@/lib/notice';

interface Line {
  key: string;
  label: string;
  value: string;
}

const NEVER =
  'Не входять ніколи: аудіо, тексти стенограм і документів, окремі слова, словник і правила, імена, назви і дати нарад, сама модель.';

/** The issue on GitHub: the lines left in, nothing else */
function issueUrl(lines: Line[], version: string): string {
  const title = `Звіт про якість розпізнавання${version ? ` (Протокол ${version})` : ''}`;
  const body = ['Звіт підготовлено в програмі. У ньому лише числа.', '', ...lines.map((l) => `- **${l.label}:** ${l.value}`)].join('\n');
  return `${FEEDBACK_REPO}/issues/new?title=${encodeURIComponent(title)}&body=${encodeURIComponent(body)}`;
}

/** Settings → Конфіденційність → «Звіт про якість розпізнавання» */
export function LearningReport() {
  const [lines, setLines] = useState<Line[] | null>(null);
  const [open, setOpen] = useState(false);

  const prepare = async () => {
    try {
      setLines(await invoke<Line[]>('api_learning_report'));
      setOpen(true);
    } catch (e) {
      toast.error(`${e}`);
    }
  };

  const pairs = (ls: Line[]) => ls.map((l) => [l.label, l.value] as [string, string]);

  const toGitHub = async () => {
    if (!lines?.length) return;
    const version = await getVersion().catch(() => '');
    try {
      await invoke('open_external_url', { url: issueUrl(lines, version) });
      await invoke('api_learning_report_sent', { how: 'issue', lines: pairs(lines) }).catch(() => {});
      toast.success('Звіт відкрито на GitHub', { description: 'Він надсилається, лише коли ви підтвердите це на сторінці.' });
      setOpen(false);
    } catch (e) {
      toast.error(`${e}`);
    }
  };

  const toFile = async () => {
    if (!lines?.length) return;
    try {
      if (await invoke<boolean>('api_learning_report_save', { lines: pairs(lines) })) {
        toast.success('Звіт збережено у файл', { description: 'Його можна передати розробникам будь-яким способом.' });
        setOpen(false);
      }
    } catch (e) {
      toast.error(`${e}`);
    }
  };

  return (
    <>
      <SettingsGroup title="Звіт про якість розпізнавання" help="Звіт не надсилається сам. Перед надсиланням видно весь його текст.">
        <SettingsRow
          label={
            <span className="flex flex-col min-w-0">
              <span>Надіслати розробникам звіт</span>
              <span className="text-xs text-[color:var(--muted-text)]">Лише числа: як донавчання змінило кількість помилок на цьому Mac</span>
            </span>
          }
        >
          <button type="button" className="settings-button" onClick={() => void prepare()}>
            Підготувати…
          </button>
        </SettingsRow>
      </SettingsGroup>

      <Dialog open={open} onOpenChange={setOpen}>
        <DialogContent className="sm:max-w-[560px] max-h-[88vh] flex flex-col gap-3.5 rounded-[20px]">
          <DialogTitle className="text-[17px] font-bold text-gray-900 pr-6">Звіт про якість розпізнавання</DialogTitle>
          <DialogDescription className="text-[13.5px] leading-[1.5] text-gray-700">
            Звіт допомагає порівняти, як донавчання працює в різних організаціях. У ньому лише числа, будь-який рядок можна прибрати.
          </DialogDescription>
          <div className="settings-list min-h-0 overflow-y-auto custom-scrollbar" role="list" aria-label="Текст звіту">
            {lines?.length === 0 && <div className="settings-row text-[color:var(--muted-text)]">Усі рядки прибрано</div>}
            {lines?.map((l) => (
              <div key={l.key} role="listitem" className="settings-row" style={{ justifyContent: 'flex-start', gap: 12, fontSize: 13 }}>
                <span className="w-[190px] flex-none text-[color:var(--muted-text)]">{l.label}</span>
                <span className="min-w-0 flex-1 break-words">{l.value}</span>
                <button
                  type="button"
                  className="flex h-6 w-6 flex-none items-center justify-center rounded-md text-[color:var(--muted-text)] hover:bg-[var(--hover)]"
                  aria-label={`Прибрати рядок «${l.label}»`}
                  onClick={() => setLines((ls) => (ls ?? []).filter((x) => x.key !== l.key))}
                >
                  <X className="h-3.5 w-3.5" aria-hidden="true" />
                </button>
              </div>
            ))}
          </div>
          <p className="text-xs leading-[1.45] text-[color:var(--muted-text)]">{NEVER} Заявку на GitHub побачать усі.</p>
          <div className="flex flex-wrap items-center justify-end gap-2 pt-1">
            <button type="button" className="settings-button" onClick={() => setOpen(false)}>
              Скасувати
            </button>
            <button type="button" className="settings-button disabled:opacity-50" disabled={!lines?.length} onClick={() => void toFile()}>
              Зберегти у файл
            </button>
            <button
              type="button"
              className="accent-button h-8 rounded-lg px-3 text-sm font-medium disabled:opacity-50"
              disabled={!lines?.length}
              onClick={() => void toGitHub()}
            >
              Відкрити на GitHub ↗
            </button>
          </div>
        </DialogContent>
      </Dialog>
    </>
  );
}
