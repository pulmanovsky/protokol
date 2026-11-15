'use client';

import React from 'react';
import { invoke } from '@tauri-apps/api/core';
import { SettingsGroup, SettingsRow } from '@/components/settings/SettingsGroup';
import { toast } from '@/lib/notice';
import { Run, Status, num, outcomeText, pct, runDate, statusChanged } from '@/lib/learning';
import { Pill, useLearningStatus } from '@/components/learning/LearningSettings';

async function use(choice: string, done: string) {
  try {
    await invoke('api_learning_use', { choice });
    toast.success(done, { description: 'Нові записи розпізнаватимуться нею.' });
    statusChanged();
  } catch (e) {
    toast.error(`${e}`);
  }
}

function State({ run, s }: { run: Run; s: Status }) {
  if (run.outcome !== 'accepted') return <span className="text-[color:var(--muted-text)]">{outcomeText(run)}</span>;
  if (run.id === s.active && s.tuned_in_use) return <Pill>Працює зараз</Pill>;
  if (s.versions.some((v) => v.id === run.id)) return <span>Попередня</span>;
  return <span className="text-[color:var(--muted-text)]">Замінена новішою</span>;
}

/** Settings → Пам'ять → «Історія донавчання» */
export function LearningHistory() {
  const { status: s } = useLearningStatus();
  if (!s) return null;
  const base = !s.tuned_in_use;

  return (
    <div className="flex flex-col gap-6">
      <p className="settings-help" style={{ marginTop: -8 }}>
        Помилки рахуються на останніх перевірених нарадах, які не брали участі в навчанні.
      </p>

      <SettingsGroup help="Зберігаються дві останні прийняті моделі, кожна займає близько 3 ГБ.">
        <div role="table" aria-label="Навчання">
          <div role="row" className="learn-grid learn-grid-head">
            <span role="columnheader">Дата</span>
            <span role="columnheader">Мовлення</span>
            <span role="columnheader">Помилок було → стало</span>
            <span role="columnheader">Стан</span>
            <span role="columnheader">
              <span className="sr-only">Дія</span>
            </span>
          </div>
          {s.history.length === 0 && (
            <div role="row" className="learn-grid">
              <span role="cell" className="col-span-5 text-[color:var(--muted-text)]">
                Навчань ще не було
              </span>
            </div>
          )}
          {s.history.map((run) => {
            const back = run.outcome === 'accepted' && run.id !== s.active && s.versions.some((v) => v.id === run.id);
            return (
              <div role="row" className="learn-grid" key={run.id}>
                <span role="cell" className="tabular-nums">
                  {runDate(run.id)}
                </span>
                <span role="cell" className="tabular-nums">
                  {num(run.train_hours + run.held_hours)} год
                </span>
                <span role="cell" className="tabular-nums">
                  {run.wer_before != null && run.wer_after != null ? (
                    `${pct(run.wer_before)} → ${pct(run.wer_after)}`
                  ) : (
                    <span className="text-[color:var(--muted-text)]">не перевірялася</span>
                  )}
                </span>
                <span role="cell" className="min-w-0 text-[13px] leading-snug">
                  <State run={run} s={s} />
                </span>
                <span role="cell">
                  {back && (
                    <button type="button" className="settings-button justify-center" onClick={() => void use(run.id, `Повернуто модель від ${runDate(run.id)}`)}>
                      Повернути
                    </button>
                  )}
                </span>
              </div>
            );
          })}
        </div>
      </SettingsGroup>

      {s.hand_copied && (
        <SettingsGroup title="Встановлена вручну">
          <SettingsRow
            label={
              <span className="flex flex-col min-w-0">
                <span>Донавчена модель з пілотного навчання</span>
                <span className="text-xs text-[color:var(--muted-text)]">Скопійована поруч зі звичайною до появи донавчання на цьому Mac</span>
              </span>
            }
          >
            {s.active === 'unset' && s.tuned_in_use ? (
              <Pill>Працює зараз</Pill>
            ) : (
              <button type="button" className="settings-button" onClick={() => void use('unset', 'Повернуто модель, встановлену вручну')}>
                Повернути
              </button>
            )}
          </SettingsRow>
        </SettingsGroup>
      )}

      <SettingsGroup title="Звичайна модель">
        <SettingsRow
          label={
            <span className="flex flex-col min-w-0">
              <span>Whisper Large V3 без донавчання</span>
              <span className="text-xs text-[color:var(--muted-text)]">Завжди залишається на цьому Mac. Донавчання при цьому не вимикається</span>
            </span>
          }
        >
          {base ? (
            <Pill>Працює зараз</Pill>
          ) : (
            <button type="button" className="settings-button" onClick={() => void use('base', 'Повернуто звичайну модель')}>
              Повернути звичайну
            </button>
          )}
        </SettingsRow>
      </SettingsGroup>
    </div>
  );
}
