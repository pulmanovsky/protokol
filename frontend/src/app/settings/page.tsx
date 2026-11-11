'use client';

import React, { useState, useEffect, Suspense } from 'react';
import { Settings2, Mic, AudioLines, FileText, CalendarDays, Palette, Info as InfoIcon } from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import { useRouter, useSearchParams } from 'next/navigation';
import { TranscriptSettings } from '@/components/TranscriptSettings';
import { RecordingSettings } from '@/components/RecordingSettings';
import { PreferenceSettings } from '@/components/PreferenceSettings';
import { SummaryModelSettings } from '@/components/SummaryModelSettings';
import { ClickWordRow, WordHighlightRow } from '@/components/PreferenceSettings';
import { SettingsGroup, SettingsLinkRow } from '@/components/settings/SettingsGroup';
import { CalendarSettings } from '@/components/CalendarSettings';
import { About } from '@/components/About';
import { DocumentTemplatesSettings } from '@/components/DocumentTemplatesSettings';
import { AppearanceSettings } from '@/components/AppearanceSettings';
import { useConfig } from '@/contexts/ConfigContext';

// Sections in the order of the left list (macOS System Settings layout)
const SECTIONS = [
  { value: 'general', label: 'Загальні', icon: Settings2 },
  { value: 'recording', label: 'Запис', icon: Mic },
  { value: 'transcription', label: 'Стенограма', icon: AudioLines },
  { value: 'document', label: 'Документ', icon: FileText },
  { value: 'calendar', label: 'Календар', icon: CalendarDays },
  { value: 'appearance', label: 'Вигляд', icon: Palette },
  { value: 'about', label: 'Про програму', icon: InfoIcon },
] as const;

type SectionValue = (typeof SECTIONS)[number]['value'];

// Older links used ?tab=<name>
const LEGACY_TABS: Record<string, SectionValue> = {
  general: 'general',
  recording: 'recording',
  Transcriptionmodels: 'transcription',
  summaryModels: 'document',
  calendar: 'calendar',
  beta: 'general',
  about: 'about',
};

// Sections merged in 0.3.11 (links from older screens still work)
const MERGED_SECTIONS: Record<string, SectionValue> = {
  protocol: 'document',
  types: 'document',
};

export default function SettingsPage() {
  return (
    <Suspense fallback={null}>
      <SettingsContent />
    </Suspense>
  );
}

function SettingsContent() {
  const searchParams = useSearchParams();
  const tab = searchParams.get('tab');
  const section = searchParams.get('section');
  const requested = (section && MERGED_SECTIONS[section]) || section || (tab ? LEGACY_TABS[tab] : null);
  // «Протокол → Перевірити оновлення…» opens «Про програму» with ?check=1
  const checkNow = searchParams.get('check') === '1';
  const router = useRouter();
  const { transcriptModelConfig, setTranscriptModelConfig } = useConfig();
  const [active, setActive] = useState<SectionValue>('general');

  useEffect(() => {
    if (requested && SECTIONS.some((s) => s.value === requested)) {
      setActive(requested as SectionValue);
    }
  }, [requested]);

  // Load saved transcript configuration on mount
  useEffect(() => {
    const loadTranscriptConfig = async () => {
      try {
        const config = (await invoke('api_get_transcript_config')) as any;
        if (config) {
          setTranscriptModelConfig({
            provider: config.provider || 'localWhisper',
            model: config.model || 'large-v3',
            apiKey: config.apiKey || null,
          });
        }
      } catch (error) {
        console.error('Failed to load transcript config:', error);
      }
    };
    loadTranscriptConfig();
  }, [setTranscriptModelConfig]);

  const current = SECTIONS.find((s) => s.value === active)!;

  return (
    <div className="h-full min-h-0 flex flex-col">
      <header data-tauri-drag-region data-notice-anchor className="flex-none px-8 pt-8 pb-5">
        <h1 data-tauri-drag-region className="text-[28px] font-bold tracking-[-0.5px] text-gray-900">Налаштування</h1>
      </header>

      <div className="flex-1 min-h-0 flex gap-6 px-8 pb-8">
        <nav aria-label="Розділи налаштувань" className="w-[220px] flex-none flex flex-col gap-0.5">
          {SECTIONS.map(({ value, label, icon: Icon }) => (
            <React.Fragment key={value}>
            {value === 'about' && <div className="h-px bg-[var(--hairline)] mx-2.5 my-2" aria-hidden="true" />}
            <button
              type="button"
              data-active={active === value}
              aria-current={active === value ? 'page' : undefined}
              onClick={() => setActive(value)}
              className="nav-row h-8 px-2.5 gap-2.5 text-sm"
            >
              <Icon className="w-4 h-4 flex-shrink-0" />
              <span className="truncate">{label}</span>
            </button>
            </React.Fragment>
          ))}
        </nav>

        {/* The only scroll container on this screen */}
        <div className="flex-1 min-w-0 overflow-y-auto overflow-x-hidden custom-scrollbar" key={active}>
          <div className="settings-pane max-w-[680px] flex flex-col gap-5 pb-8 pr-2">
            <h2 className="text-xl font-bold text-gray-900">{current.label}</h2>
            {active === 'general' && <PreferenceSettings />}
            {active === 'recording' && <RecordingSettings />}
            {active === 'transcription' && (
              <>
                <TranscriptSettings
                  transcriptModelConfig={transcriptModelConfig}
                  setTranscriptModelConfig={setTranscriptModelConfig}
                />
                <SettingsGroup title="Перегляд і прослуховування">
                  <WordHighlightRow />
                  <ClickWordRow />
                </SettingsGroup>
                <SettingsGroup title="Словник і спікери">
                  <SettingsLinkRow label="Словник" sub="Написання імен, посад та абревіатур" onClick={() => router.push('/templates?tab=words')} />
                  <SettingsLinkRow label="Пам'ять виправлень" sub="Що програма бере з ваших виправлень" onClick={() => router.push('/templates')} />
                  <SettingsLinkRow label="Збережені спікери" sub="Голоси та імена, які пропонуються в нових записах" onClick={() => router.push('/templates?people=1')} />
                </SettingsGroup>
              </>
            )}
            {active === 'document' && (
              <>
                <DocumentTemplatesSettings />
                <SummaryModelSettings />
              </>
            )}
            {active === 'calendar' && <CalendarSettings />}
            {active === 'appearance' && <AppearanceSettings />}
            {active === 'about' && <About checkNow={checkNow} />}
          </div>
        </div>
      </div>
    </div>
  );
}
