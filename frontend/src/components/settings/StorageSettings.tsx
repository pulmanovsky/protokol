'use client';

import React, { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { FolderOpen } from 'lucide-react';
import { RecordingSettings } from '@/components/RecordingSettings';
import { SettingsGroup, SettingsRow } from '@/components/settings/SettingsGroup';
import { importModelsFromFolder } from '@/lib/importModels';

interface Usage {
  audio: number;
  models: number;
  data: number;
  free: number | null;
}

/** «8,9 ГБ», «420 МБ» */
export function formatBytes(n: number): string {
  const gb = n / 1024 ** 3;
  if (gb >= 1) return `${gb.toLocaleString('uk-UA', { maximumFractionDigits: 1 })} ГБ`;
  const mb = n / 1024 ** 2;
  if (mb >= 1) return `${Math.round(mb).toLocaleString('uk-UA')} МБ`;
  return n <= 0 ? '0 КБ' : `${Math.max(1, Math.round(n / 1024))} КБ`;
}

const PARTS: { key: 'audio' | 'models' | 'data'; label: string; color: string }[] = [
  { key: 'audio', label: 'Аудіозаписи', color: '#ff9f0a' },
  { key: 'models', label: 'Моделі', color: '#0a84ff' },
  { key: 'data', label: 'База і стенограми', color: '#34c759' },
];

function UsageBar() {
  const [usage, setUsage] = useState<Usage | null>(null);
  useEffect(() => {
    invoke<Usage>('api_storage_usage').then(setUsage).catch(() => setUsage(null));
  }, []);
  const total = usage ? usage.audio + usage.models + usage.data : 0;
  return (
    <div className="settings-list">
      <div className="settings-row" style={{ flexDirection: 'column', alignItems: 'stretch', gap: 10 }}>
        <div className="flex justify-between gap-4 text-[13px]">
          <span>Протокол займає {usage ? formatBytes(total) : '…'}</span>
          {usage?.free != null && <span className="text-[color:var(--muted-text)]">Вільно на диску {formatBytes(usage.free)}</span>}
        </div>
        <div className="flex h-2.5 rounded-full overflow-hidden bg-[var(--hairline)]" role="img" aria-label="Скільки місця займає кожна частина">
          {usage &&
            total > 0 &&
            PARTS.map((p) => <span key={p.key} style={{ width: `${(usage[p.key] / total) * 100}%`, background: p.color }} />)}
        </div>
        <div className="flex flex-wrap gap-x-4 gap-y-1 text-xs text-[color:var(--muted-text)]">
          {PARTS.map((p) => (
            <span key={p.key} className="flex items-center gap-1.5">
              <i className="inline-block w-2 h-2 rounded-full" style={{ background: p.color }} />
              {p.label} {usage ? formatBytes(usage[p.key]) : '…'}
            </span>
          ))}
        </div>
      </div>
    </div>
  );
}

function ModelsFolderRow() {
  const [path, setPath] = useState('');
  useEffect(() => {
    invoke<string>('api_models_folder').then(setPath).catch(() => setPath(''));
  }, []);
  return (
    <SettingsRow
      label={
        <span className="flex flex-col min-w-0">
          <span>Папка моделей</span>
          <span className="text-xs text-[color:var(--muted-text)] truncate" title={path}>
            {path || '…'}
          </span>
        </span>
      }
    >
      <button type="button" onClick={() => void invoke('api_models_reveal').catch(() => {})} className="settings-button">
        <FolderOpen className="w-3.5 h-3.5" />
        Показати у Finder
      </button>
    </SettingsRow>
  );
}

/** Settings → Сховище: what takes space, the recordings folder, models from a folder */
export function StorageSettings() {
  return (
    <div className="flex flex-col gap-6">
      <UsageBar />
      <RecordingSettings part="storage" />
      <SettingsGroup title="Моделі" help="База даних і моделі зберігаються разом у папці даних програми.">
        <ModelsFolderRow />
        <SettingsRow
          label={
            <span className="flex flex-col min-w-0">
              <span>Встановити моделі з папки</span>
              <span className="text-xs text-[color:var(--muted-text)]">Для Mac без інтернету: моделі з флешки або спільної папки</span>
            </span>
          }
        >
          <button type="button" onClick={() => void importModelsFromFolder()} className="settings-button">
            <FolderOpen className="w-3.5 h-3.5" />
            Вибрати папку…
          </button>
        </SettingsRow>
      </SettingsGroup>
    </div>
  );
}
