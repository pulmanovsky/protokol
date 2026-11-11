'use client';

import { useCallback, useEffect, useMemo, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { getVersion } from '@tauri-apps/api/app';
import { AlertTriangle } from 'lucide-react';
import { Dialog, DialogContent, DialogDescription, DialogTitle } from '@/components/ui/dialog';
import { toast } from '@/lib/notice';
import { FEEDBACK_EVENT, FEEDBACK_REPO, FeedbackKind } from '@/lib/feedback';

interface Info {
  macos: string;
  mac_model: string;
  memory: string;
  free_disk: string;
  log_errors: string[];
}

interface Facts {
  version: string;
  info: Info | null;
  models: string;
}

/** Longest link a browser takes without trouble */
const MAX_URL = 7500;

const COPY: Record<FeedbackKind, { title: string; what: string; expected: string; whatHint: string; expectedHint: string }> = {
  problem: {
    title: 'Повідомити про проблему',
    what: 'Що сталося?',
    expected: 'Що мало статися?',
    whatHint: 'Наприклад: після завершення запису документ створився без підсумків доповідачів.',
    expectedHint: 'Наприклад: короткий підсумок під кожним доповідачем.',
  },
  idea: {
    title: 'Запропонувати покращення',
    what: 'Що покращити?',
    expected: 'Для чого це потрібно?',
    whatHint: 'Наприклад: експорт документа одразу у форматі .docx.',
    expectedHint: 'Наприклад: документ надсилається учасникам як вкладення.',
  },
};

export function FeedbackDialog() {
  const [open, setOpen] = useState(false);
  const [kind, setKind] = useState<FeedbackKind>('problem');
  const [what, setWhat] = useState('');
  const [expected, setExpected] = useState('');
  const [facts, setFacts] = useState<Facts | null>(null);
  const [withSystem, setWithSystem] = useState(true);
  const [withMac, setWithMac] = useState(true);
  const [withModels, setWithModels] = useState(true);
  const [withLog, setWithLog] = useState(true);
  const [preview, setPreview] = useState(false);

  const setDefaults = (k: FeedbackKind) => {
    // A suggestion rarely needs the Mac's details: off unless ticked
    const on = k === 'problem';
    setWithSystem(on);
    setWithMac(on);
    setWithModels(on);
    setWithLog(on);
  };

  useEffect(() => {
    const onOpen = (e: Event) => {
      const k = ((e as CustomEvent<FeedbackKind>).detail ?? 'problem') as FeedbackKind;
      setKind(k);
      setDefaults(k);
      setWhat('');
      setExpected('');
      setPreview(false);
      setOpen(true);
    };
    window.addEventListener(FEEDBACK_EVENT, onOpen);
    return () => window.removeEventListener(FEEDBACK_EVENT, onOpen);
  }, []);

  // The facts are read on this Mac each time the form opens
  useEffect(() => {
    if (!open) return;
    let alive = true;
    (async () => {
      const version = await getVersion().catch(() => '');
      const info = await invoke<Info>('api_feedback_info').catch(() => null);
      const stt = await invoke<{ provider?: string; model?: string }>('api_get_transcript_config').catch(() => null);
      const llm = await invoke<{ model?: string }>('api_get_model_config').catch(() => null);
      const models = [stt?.model ? `розпізнавання: ${stt.provider === 'parakeet' ? 'Parakeet' : 'Whisper'} ${stt.model}` : '', llm?.model ? `документ: ${llm.model}` : '']
        .filter(Boolean)
        .join(' · ');
      if (alive) setFacts({ version, info, models });
    })();
    return () => {
      alive = false;
    };
  }, [open]);

  const copy = COPY[kind];
  const errors = facts?.info?.log_errors ?? [];

  const build = useCallback(
    (logLines: string[]) => {
      const title = (what.trim().split('\n')[0] || copy.title).slice(0, 90);
      const env: string[] = [];
      if (withSystem) env.push(`- Протокол ${facts?.version ?? ''}${facts?.info?.macos ? `, ${facts.info.macos}` : ''}`);
      if (withMac && facts?.info) env.push(`- ${[facts.info.mac_model, facts.info.memory && `${facts.info.memory} пам'яті`, facts.info.free_disk && `${facts.info.free_disk} вільно`].filter(Boolean).join(', ')}`);
      if (withModels && facts?.models) env.push(`- Моделі: ${facts.models}`);
      const parts = [
        `## ${kind === 'problem' ? 'Що сталося' : 'Що покращити'}`,
        what.trim() || '—',
        '',
        `## ${kind === 'problem' ? 'Що мало статися' : 'Для чого це потрібно'}`,
        expected.trim() || '—',
      ];
      if (env.length) parts.push('', '## Середовище', ...env);
      if (withLog && logLines.length) parts.push('', '## Помилки з журналу (остання доба)', '```', ...logLines, '```');
      return { title, body: parts.join('\n') };
    },
    [what, expected, kind, copy.title, facts, withSystem, withMac, withModels, withLog],
  );

  const text = useMemo(() => {
    const { title, body } = build(errors);
    return `Заголовок: ${title}\n\n${body}`;
  }, [build, errors]);

  /** The link with as many log lines as fit; the newest lines are kept */
  const link = () => {
    for (let skip = 0; skip <= errors.length; skip++) {
      const { title, body } = build(errors.slice(skip));
      const url = `${FEEDBACK_REPO}/issues/new?labels=${kind === 'problem' ? 'bug' : 'enhancement'}&title=${encodeURIComponent(title)}&body=${encodeURIComponent(body)}`;
      if (url.length <= MAX_URL) return { url, cut: skip > 0 };
    }
    return null;
  };

  const send = async () => {
    if (!what.trim()) {
      toast.error(`Заповніть поле «${copy.what}»`);
      return;
    }
    const made = link();
    if (!made) {
      // Too long for a link: the text goes to the clipboard, the page opens empty
      await navigator.clipboard?.writeText(text).catch(() => {});
      const url = `${FEEDBACK_REPO}/issues/new`;
      await invoke('open_external_url', { url }).catch(() => {});
      toast.info('Текст звернення скопійовано', { description: 'Вставте його у звернення за допомогою ⌘V.' });
    } else {
      await invoke('open_external_url', { url: made.url }).catch(() => {});
      if (made.cut) toast.info('Додано лише останні помилки з журналу', { description: 'Повний текст не вміщається в посилання.' });
    }
    setOpen(false);
  };

  const check = (on: boolean, set: (v: boolean) => void, label: string, value: string) => (
    <label className="flex items-start gap-2.5 px-3 py-2 text-[13px] border-t border-[var(--hairline)] first:border-t-0 cursor-pointer">
      <input type="checkbox" className="mt-0.5" checked={on} onChange={(e) => set(e.target.checked)} />
      <span className="min-w-0">
        <span className="block">{label}</span>
        <span className="block text-xs text-[color:var(--muted-text)] break-words">{value || 'Невідомо'}</span>
      </span>
    </label>
  );

  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogContent className="sm:max-w-[640px] max-h-[88vh] flex flex-col">
        <div className="flex items-center justify-between gap-3 pr-6">
          <DialogTitle>{preview ? 'Текст звернення' : copy.title}</DialogTitle>
          {!preview && (
            <div className="step-switch" role="tablist" aria-label="Тип звернення">
              {(['problem', 'idea'] as const).map((k) => (
                <button
                  key={k}
                  type="button"
                  role="tab"
                  aria-selected={kind === k}
                  data-on={kind === k}
                  onClick={() => {
                    setKind(k);
                    setDefaults(k);
                  }}
                >
                  {k === 'problem' ? 'Проблема' : 'Пропозиція'}
                </button>
              ))}
            </div>
          )}
        </div>
        <DialogDescription className="sr-only">
          Звернення не надсилається автоматично: відкривається заповнена сторінка на GitHub.
        </DialogDescription>

        {/* Only the middle scrolls: the buttons stay visible in a low window */}
        <div className="min-h-0 flex-1 overflow-y-auto custom-scrollbar -mx-1 px-1">
        {preview ? (
          <div className="flex flex-col gap-3">
            <p className="text-[12.5px] text-[color:var(--muted-text)]">
              Саме цей текст буде вставлено у звернення. Його можна змінити на GitHub перед надсиланням.
            </p>
            <pre className="whitespace-pre-wrap break-words rounded-lg border border-[var(--hairline)] bg-[var(--surface-bg)] p-3 font-mono text-[12px] leading-relaxed max-h-[46vh] overflow-y-auto custom-scrollbar">
              {text}
            </pre>
            <p className="text-xs text-[color:var(--muted-text)]">
              У рядках журналу назви записів, імена, будь-який український текст і шляхи до файлів замінено. Сам журнал залишається на цьому Mac.
            </p>
          </div>
        ) : (
          <div className="flex flex-col gap-3.5">
            <label className="flex flex-col gap-1.5 text-[13px] font-semibold">
              {copy.what}
              <textarea
                className="min-h-[72px] px-2.5 py-2 text-[13px] font-normal leading-relaxed"
                value={what}
                placeholder={copy.whatHint}
                onChange={(e) => setWhat(e.target.value)}
                autoFocus
              />
            </label>
            <label className="flex flex-col gap-1.5 text-[13px] font-semibold">
              {copy.expected}
              <textarea
                className="min-h-[52px] px-2.5 py-2 text-[13px] font-normal leading-relaxed"
                value={expected}
                placeholder={copy.expectedHint}
                onChange={(e) => setExpected(e.target.value)}
              />
            </label>

            <div className="flex flex-col gap-1.5">
              <span className="text-[13px] font-semibold">Додати до звернення</span>
              <div className="settings-list">
                {check(withSystem, setWithSystem, 'Версія програми та macOS', [facts?.version && `Протокол ${facts.version}`, facts?.info?.macos].filter(Boolean).join(' · '))}
                {check(withMac, setWithMac, "Модель Mac, пам'ять і вільне місце", [facts?.info?.mac_model, facts?.info?.memory && `${facts.info.memory} пам'яті`, facts?.info?.free_disk && `${facts.info.free_disk} вільно`].filter(Boolean).join(' · '))}
                {check(withModels, setWithModels, 'Моделі на цьому Mac', facts?.models ?? '')}
                {check(
                  withLog,
                  setWithLog,
                  'Помилки з журналу за останню добу',
                  !facts ? 'Читання журналу…' : errors.length ? `Рядків помилок: ${errors.length}. Назви записів, імена та шляхи до файлів замінено` : 'Помилок немає',
                )}
              </div>
              <p className="text-xs text-[color:var(--muted-text)]">Тексти стенограм і документів, аудіо, назви записів та імена не додаються ніколи.</p>
            </div>

            <div className="flex items-start gap-2 rounded-lg bg-[rgba(255,159,10,0.14)] px-3 py-2 text-[12.5px] leading-relaxed text-[#9a5a00] dark:text-[#ffcf8a]">
              <AlertTriangle className="mt-0.5 h-3.5 w-3.5 flex-none" aria-hidden="true" />
              <span>Звернення на GitHub бачать усі. Не вказуйте службової інформації. Для надсилання потрібен обліковий запис GitHub.</span>
            </div>
          </div>
        )}
        </div>

        <div className="flex flex-wrap items-center gap-2 pt-1">
          {preview ? (
            <button type="button" className="settings-button" onClick={() => setPreview(false)}>
              Назад
            </button>
          ) : (
            <button type="button" className="settings-button" onClick={() => setPreview(true)}>
              Переглянути текст…
            </button>
          )}
          <span className="flex-1" />
          {preview ? (
            <button
              type="button"
              className="settings-button"
              onClick={() => {
                void navigator.clipboard?.writeText(text).then(() => toast.success('Текст скопійовано')).catch(() => {});
              }}
            >
              Скопіювати текст
            </button>
          ) : (
            <button type="button" className="settings-button" onClick={() => setOpen(false)}>
              Скасувати
            </button>
          )}
          <button type="button" className="accent-button h-8 rounded-lg px-3 text-sm font-medium" onClick={() => void send()}>
            Відкрити на GitHub
          </button>
        </div>
      </DialogContent>
    </Dialog>
  );
}
