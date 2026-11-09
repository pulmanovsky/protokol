'use client';

import React, { useState, useEffect, Suspense } from 'react';
import { Settings2, Mic, AudioLines, FileText, CalendarDays, Palette, Info as InfoIcon, ListChecks } from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import { useSearchParams } from 'next/navigation';
import { TranscriptSettings } from '@/components/TranscriptSettings';
import { RecordingSettings } from '@/components/RecordingSettings';
import { PreferenceSettings } from '@/components/PreferenceSettings';
import { SummaryModelSettings } from '@/components/SummaryModelSettings';
import { BetaSettings } from '@/components/BetaSettings';
import { CalendarSettings } from '@/components/CalendarSettings';
import { About } from '@/components/About';
import { MeetingTypesSettings } from '@/components/MeetingTypesSettings';
import { AppearanceSettings } from '@/components/AppearanceSettings';
import { useConfig } from '@/contexts/ConfigContext';

// Sections in the order of the left list (macOS System Settings layout)
const SECTIONS = [
  { value: 'general', label: 'Загальні', icon: Settings2 },
  { value: 'recording', label: 'Запис', icon: Mic },
  { value: 'transcription', label: 'Розпізнавання', icon: AudioLines },
  { value: 'protocol', label: 'Документ і модель', icon: FileText },
  { value: 'types', label: 'Типи записів', icon: ListChecks },
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
  summaryModels: 'protocol',
  calendar: 'calendar',
  beta: 'general',
  about: 'about',
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
  const requested = searchParams.get('section') ?? (tab ? LEGACY_TABS[tab] : null);
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
            <button
              key={value}
              type="button"
              data-active={active === value}
              aria-current={active === value ? 'page' : undefined}
              onClick={() => setActive(value)}
              className="nav-row h-8 px-2.5 gap-2.5 text-sm"
            >
              <Icon className="w-4 h-4 flex-shrink-0" />
              <span className="truncate">{label}</span>
            </button>
          ))}
        </nav>

        {/* The only scroll container on this screen */}
        <div className="flex-1 min-w-0 overflow-y-auto overflow-x-hidden custom-scrollbar" key={active}>
          <div className="settings-pane max-w-[680px] flex flex-col gap-5 pb-8 pr-2">
            <h2 className="text-xl font-bold text-gray-900">{current.label}</h2>
            {active === 'general' && (
              <>
                <PreferenceSettings />
                <h3 className="settings-group-title">Експериментальне</h3>
                <BetaSettings />
              </>
            )}
            {active === 'recording' && <RecordingSettings />}
            {active === 'transcription' && (
              <TranscriptSettings
                transcriptModelConfig={transcriptModelConfig}
                setTranscriptModelConfig={setTranscriptModelConfig}
              />
            )}
            {active === 'types' && <MeetingTypesSettings />}
            {active === 'protocol' && <SummaryModelSettings />}
            {active === 'calendar' && <CalendarSettings />}
            {active === 'appearance' && <AppearanceSettings />}
            {active === 'about' && <About />}
          </div>
        </div>
      </div>
    </div>
  );
}
