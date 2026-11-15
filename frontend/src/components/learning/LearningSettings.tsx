'use client';

import React, { useCallback, useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { useRouter } from 'next/navigation';
import { SettingsBlock, SettingsGroup, SettingsLinkRow, SettingsRow } from '@/components/settings/SettingsGroup';
import { confirmNative } from '@/lib/confirm';
import { plural } from '@/lib/plural';
import { toast } from '@/lib/notice';
import {
  MIN_HOURS,
  MODE_LABEL,
  Mode,
  STATUS_CHANGED,
  Status,
  hoursText,
  installEngine,
  loadStatus,
  num,
  pct,
  runDate,
  statusChanged,
  waitText,
} from '@/lib/learning';
import { startNow } from '@/components/learning/LearningPrompts';

function Label({ title, sub }: { title: React.ReactNode; sub?: React.ReactNode }) {
  return (
    <span className="flex flex-col min-w-0">
      <span>{title}</span>
      {sub && <span className="text-xs text-[color:var(--muted-text)]">{sub}</span>}
    </span>
  );
}

export function Pill({ children }: { children: React.ReactNode }) {
  return <span className="learn-pill">{children}</span>;
}

function Bar({ value, label, wide = false }: { value: number; label: string; wide?: boolean }) {
  const v = Math.max(0, Math.min(100, Math.round(value)));
  return (
    <div
      className={`learn-track ${wide ? 'learn-track-wide' : ''}`}
      role="progressbar"
      aria-label={label}
      aria-valuenow={v}
      aria-valuemin={0}
      aria-valuemax={100}
    >
      <div className="learn-fill" style={{ width: `${v}%` }} />
    </div>
  );
}

/** Status of the learning, read again while a run or a download is on */
export function useLearningStatus() {
  const [status, setStatus] = useState<Status | null>(null);
  const load = useCallback(() => {
    loadStatus()
      .then(setStatus)
      .catch(() => {});
  }, []);
  useEffect(() => {
    load();
    window.addEventListener(STATUS_CHANGED, load);
    const subs = [
      listen('learning-stage', load),
      listen('learning-finished', load),
      // the row follows the download as closely as the notice does
      listen<{ percent: number }>('learning-engine-progress', (e) => {
        const percent = e.payload.percent;
        if (percent < 100) setStatus((prev) => (prev ? { ...prev, engine_download: percent } : prev));
      }),
    ];
    return () => {
      window.removeEventListener(STATUS_CHANGED, load);
      subs.forEach((p) => p.then((f) => f()));
    };
  }, [load]);
  const busy = !!status && (status.running || status.engine_download != null);
  useEffect(() => {
    const t = setInterval(load, busy ? 5_000 : 30_000);
    return () => clearInterval(t);
  }, [busy, load]);
  return { status, reload: load };
}

/** A row that opens a page, with a count before › */
function CountRow({ title, sub, count, onClick }: { title: string; sub: string; count: string; onClick: () => void }) {
  return (
    <button type="button" className="settings-row w-full text-left hover:bg-[var(--hover)]" onClick={onClick}>
      <Label title={title} sub={sub} />
      <span className="flex items-center gap-1.5 text-[13px] text-[color:var(--muted-text)] whitespace-nowrap">
        {count}
        <span aria-hidden="true">›</span>
      </span>
    </button>
  );
}

/** What the model in use is */
function modelSub(s: Status): { sub: string; tuned: boolean } {
  const version = s.versions.find((v) => v.id === s.active);
  if (version) {
    const made = `Донавчена ${runDate(version.id)} на ${num(version.train_hours + version.held_hours)} год перевіреного мовлення.`;
    if (s.running) return { sub: `${made} Працює, поки триває навчання нової`, tuned: true };
    const numbers =
      version.wer_before != null && version.wer_after != null
        ? ` На останніх нарадах помилок ${pct(version.wer_after)} замість ${pct(version.wer_before)}`
        : '';
    return { sub: made + numbers, tuned: true };
  }
  if (s.tuned_in_use) return { sub: 'Донавчена модель, встановлена вручну', tuned: true };
  return { sub: 'Whisper Large V3 без донавчання', tuned: false };
}

/** The run on now: what it does and about how long is left */
function RunBlock({ s }: { s: Status }) {
  const stage = s.stage ?? { stage: 'preparing' as const };
  let value = 3;
  let left = 'Усе навчання займе 30–40 хвилин';
  if (stage.stage === 'training') {
    value = 5 + 85 * stage.share;
    if (stage.share >= 0.03 && s.training_minutes != null) {
      const minutes = Math.max(1, Math.round((s.training_minutes * (1 - stage.share)) / stage.share + 5));
      left = `Залишилося близько ${minutes} хв`;
    }
  } else if (stage.stage === 'checking' || stage.stage === 'finished') {
    value = 93;
    left = 'Залишилося кілька хвилин';
  }
  const steps = [
    { on: stage.stage === 'preparing', text: 'Підготовка даних' },
    {
      on: stage.stage === 'training',
      text: stage.stage === 'training' ? `Навчання, прохід ${Math.max(1, Math.min(stage.epoch, stage.epochs))} з ${stage.epochs}` : 'Навчання',
    },
    { on: stage.stage === 'checking' || stage.stage === 'finished', text: 'Перевірка на останніх нарадах' },
  ];

  const stop = async () => {
    if (!(await confirmNative('Зупинити навчання?', 'Поточна модель залишиться. Навчання можна почати знову пізніше.', 'Зупинити'))) return;
    await invoke('api_learning_stop').catch(() => {});
    toast.info('Навчання зупиняється', { description: 'Поточна модель залишається.' });
    statusChanged();
  };

  return (
    <SettingsBlock>
      <div className="flex flex-col gap-2.5 text-[14px] text-[color:rgb(var(--gray-900))]">
        <div className="flex items-center justify-between gap-4">
          <Label
            title="Триває навчання нової моделі"
            sub={`${left}. Під час запису навчання стає на паузу і продовжується після нього`}
          />
          <button type="button" className="settings-button flex-none" onClick={() => void stop()}>
            Зупинити
          </button>
        </div>
        <Bar value={value} label="Перебіг навчання" wide />
        <div className="flex flex-wrap gap-x-5 gap-y-1 text-xs text-[color:var(--muted-text)]">
          {steps.map((step) => (
            <span key={step.text} className={step.on ? 'text-[color:rgb(var(--gray-900))]' : undefined} aria-current={step.on ? 'step' : undefined}>
              {step.text}
            </span>
          ))}
        </div>
      </div>
    </SettingsBlock>
  );
}

/** Settings → Пам'ять → «Розпізнавання мовлення» */
export function LearningSettings() {
  const router = useRouter();
  const { status: s } = useLearningStatus();

  if (!s) return null;

  const model = modelSub(s);
  const off = s.mode === 'off';
  const otherModel = s.wait === 'other_model' || s.wait_by_hand === 'other_model';
  const downloading = s.engine_download;
  const runs = s.history.length;

  const changeMode = async (mode: Mode) => {
    try {
      await invoke('api_learning_set_mode', { mode });
      statusChanged();
    } catch (e) {
      toast.error(`${e}`);
    }
  };

  const newSub = () => {
    if (s.mode === 'nightly') {
      return s.new_hours >= MIN_HOURS ? 'Набралося достатньо: навчання почнеться найближчої ночі' : `Наступне навчання почнеться, коли набереться ${MIN_HOURS} год`;
    }
    return 'Від останнього навчання. Уночі навчання не починається само';
  };

  return (
    <SettingsGroup
      title="Розпізнавання мовлення"
      help={
        s.running
          ? "Якщо Mac почне працювати повільно або забракне пам'яті, навчання зупиниться само, а поточна модель залишиться."
          : 'Донавчена модель зберігається лише на цьому Mac і нікуди не передається. Звичайну модель можна повернути будь-коли в історії донавчання.'
      }
    >
      <SettingsRow label={<Label title="Модель розпізнавання" sub={model.sub} />}>{model.tuned && <Pill>Донавчена</Pill>}</SettingsRow>

      {s.running && <RunBlock s={s} />}

      {!s.can_train_here ? (
        <SettingsRow label={<Label title="Донавчання недоступне на цьому Mac" sub={waitText('not_this_mac')} />} />
      ) : (
        <>
          <SettingsRow label={<Label title="Донавчати" sub="30–40 хвилин, лише коли Mac на живленні і ним не користуються" />} htmlFor="learning-mode">
            <select
              id="learning-mode"
              className="h-7 px-2 text-[13px]"
              value={s.mode ?? 'manual'}
              onChange={(e) => void changeMode(e.target.value as Mode)}
            >
              {(['nightly', 'manual', 'off'] as const).map((m) => (
                <option key={m} value={m}>
                  {MODE_LABEL[m]}
                </option>
              ))}
            </select>
          </SettingsRow>

          {!off && otherModel && (
            <SettingsRow
              label={<Label title="Донавчається лише Whisper Large V3" sub="Зараз для розпізнавання вибрано іншу модель. Її можна змінити в розділі «Стенограма»" />}
            >
              <button type="button" className="settings-button" onClick={() => router.push('/settings?section=transcription')}>
                Відкрити
              </button>
            </SettingsRow>
          )}

          {!off && !s.engine_ready && (
            <SettingsRow
              label={
                <Label
                  title="Модуль навчання"
                  sub={
                    downloading != null
                      ? `Завантаження… ${downloading}%`
                      : `Не встановлено. Близько ${s.engine_download_mb} МБ, завантажується один раз. Без інтернету: з файлу, отриманого разом з підписом .sig`
                  }
                />
              }
            >
              {downloading != null ? (
                <Bar value={downloading} label="Завантаження модуля навчання" />
              ) : (
                <>
                  <button type="button" className="settings-button" onClick={() => void installEngine(true)}>
                    З файлу…
                  </button>
                  <button type="button" className="settings-button" onClick={() => void installEngine()}>
                    Завантажити
                  </button>
                </>
              )}
            </SettingsRow>
          )}

          {!off && !s.running && (
            <SettingsRow label={<Label title="Нове перевірене мовлення" sub={newSub()} />}>
              <span className="text-[13px] text-[color:var(--muted-text)] whitespace-nowrap">
                {hoursText(Math.min(s.new_hours, 99))} з {MIN_HOURS} год
              </span>
              <Bar value={(100 * s.new_hours) / MIN_HOURS} label="Нове перевірене мовлення" />
            </SettingsRow>
          )}

          {!off && !s.running && (
            <SettingsRow
              label={
                <Label
                  title="Покращити розпізнавання зараз"
                  sub={s.wait_by_hand ? waitText(s.wait_by_hand) : 'Не чекаючи ночі. Під час запису навчання стає на паузу'}
                />
              }
            >
              <button type="button" className="settings-button disabled:opacity-50" disabled={!!s.wait_by_hand} onClick={() => void startNow()}>
                Почати
              </button>
            </SettingsRow>
          )}
        </>
      )}

      <CountRow
        title="Історія донавчання"
        sub="Дати, результати, повернення попередньої моделі"
        count={runs ? `${runs} ${plural(runs, 'навчання', 'навчання', 'навчань')}` : ''}
        onClick={() => router.push('/settings?section=memory&view=history')}
      />
      <SettingsLinkRow
        label="Якість розпізнавання"
        sub="Скільки слів доводиться виправляти від наради до наради"
        onClick={() => router.push('/settings?section=memory&view=quality')}
      />
    </SettingsGroup>
  );
}
