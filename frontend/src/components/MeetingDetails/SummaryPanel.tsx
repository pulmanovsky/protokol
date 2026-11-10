"use client";

import { DocumentProgress } from './DocumentProgress';
import { MeetingSummary, Summary, Transcript } from '@/types';
import { BlockNoteSummaryView, BlockNoteSummaryViewRef } from '@/components/AISummary/BlockNoteSummaryView';
import { DocSectionCopy } from './DocSectionCopy';
import { EmptyStateSummary } from '@/components/EmptyStateSummary';
import { ModelConfig } from '@/components/ModelSettingsModal';
import { SummaryGeneratorButtonGroup } from './SummaryGeneratorButtonGroup';
import Analytics from '@/lib/analytics';
import React, { useEffect, useRef, useState, RefObject } from 'react';
import { toast } from '@/lib/notice';
import { ChevronDown, Globe, Loader2, Save, Search } from 'lucide-react';
import { speakerColorMap } from '@/lib/speakerColors';
import { Popover, PopoverTrigger, PopoverContent } from '@/components/ui/popover';
import { LanguagePickerPopover } from '@/components/LanguagePickerPopover';
import { useRecentLanguages } from '@/hooks/useRecentLanguages';
import { labelForCode } from '@/lib/summary-languages';
import {
  readMeetingSummaryLanguage,
  saveMeetingSummaryLanguage,
  SummaryLanguageStorage,
} from '@/lib/summary-language-preferences';
import { hasVisibleSummaryContent } from '@/lib/summary-content';

interface SummaryPanelProps {
  meeting: {
    id: string;
    title: string;
    created_at: string;
  };
  meetingTitle: string;
  isSummaryDirty: boolean;
  summaryRef: RefObject<BlockNoteSummaryViewRef>;
  isSaving: boolean;
  onSaveAll: () => Promise<void>;
  onCopySummary: () => Promise<void>;
  aiSummary: MeetingSummary | null;
  summaryStatus: 'idle' | 'processing' | 'summarizing' | 'regenerating' | 'completed' | 'error';
  transcripts: Transcript[];
  modelConfig: ModelConfig;
  setModelConfig: (config: ModelConfig | ((prev: ModelConfig) => ModelConfig)) => void;
  onSaveModelConfig: (config?: ModelConfig) => Promise<void>;
  onGenerateSummary: (customPrompt: string) => Promise<void>;
  onStopGeneration: () => void;
  customPrompt: string;
  onSaveSummary: (summary: MeetingSummary) => Promise<void>;
  onSummaryChange: (summary: Summary) => void;
  onDirtyChange: (isDirty: boolean) => void;
  summaryError: string | null;
  onRegenerateSummary: () => Promise<void>;
  getSummaryStatusMessage: (status: 'idle' | 'processing' | 'summarizing' | 'regenerating' | 'completed' | 'error') => string;
  isModelConfigLoading?: boolean;
  onOpenModelSettings?: (openFn: () => void) => void;
  /** People, create again, «Копіювати», «Експорт»: the right of the toolbar, as on the transcript */
  toolbarRight?: React.ReactNode;
  /** The recording's player under the document */
  footer?: React.ReactNode;
  /** Clicks in the document's text (time marks, points without a source) */
  onTextClick?: (event: React.MouseEvent) => void;
  /** A note under the toolbar (the transcript not fully checked) */
  banner?: React.ReactNode;
  /** The template button in the toolbar (in place of the model's) */
  templateSlot?: React.ReactNode;
  /** What the tab shows before a document exists (the template cards) */
  emptyState?: React.ReactNode;
  /** Changes when another document is shown: the editor starts afresh */
  docKey?: string;
}

export function SummaryPanel({
  meeting,
  meetingTitle,
  isSummaryDirty,
  summaryRef,
  isSaving,
  onSaveAll,
  onCopySummary,
  aiSummary,
  summaryStatus,
  transcripts,
  modelConfig,
  setModelConfig,
  onSaveModelConfig,
  onGenerateSummary,
  onStopGeneration,
  customPrompt,
  onSaveSummary,
  onSummaryChange,
  onDirtyChange,
  summaryError,
  onRegenerateSummary,
  getSummaryStatusMessage,
  isModelConfigLoading = false,
  onOpenModelSettings,
  toolbarRight,
  footer,
  onTextClick,
  banner,
  templateSlot,
  emptyState,
  docKey,
}: SummaryPanelProps) {
  const [searching, setSearching] = useState(false);
  const [query, setQuery] = useState('');
  const [summaryLang, setSummaryLang] = useState<string | null>(null);
  const [summaryLangStorage, setSummaryLangStorage] = useState<SummaryLanguageStorage>('metadata');
  const [langPickerOpen, setLangPickerOpen] = useState(false);
  const languageLoadVersionRef = useRef(0);
  const activeMeetingIdRef = useRef(meeting.id);
  const languageSaveVersionRef = useRef(0);
  const languageSaveLoopRunningRef = useRef(false);
  const latestLanguageSaveRequestRef = useRef<{
    version: number;
    meetingId: string;
    language: string | null;
    rollback: {
      language: string | null;
      storage: SummaryLanguageStorage;
    };
  } | null>(null);
  activeMeetingIdRef.current = meeting.id;
  const { addRecent } = useRecentLanguages();

  const effectiveLangLabel = summaryLang ? labelForCode(summaryLang) : 'Автоматично';
  const isLocalFallbackLanguage = summaryLangStorage === 'local_fallback';
  const autoSubtitle = isLocalFallbackLanguage
    ? 'Збережено на цьому Mac'
    : 'Мова, якою найбільше говорили';

  useEffect(() => {
    let cancelled = false;
    const loadVersion = languageLoadVersionRef.current + 1;
    languageLoadVersionRef.current = loadVersion;

    const loadSummaryLanguage = async () => {
      try {
        const stored = await readMeetingSummaryLanguage(meeting.id);
        if (!cancelled && languageLoadVersionRef.current === loadVersion) {
          setSummaryLang(stored.language);
          setSummaryLangStorage(stored.storage);
        }
      } catch (err) {
        console.error('Failed to load summary language:', err);
        toast.warning('Не вдалося завантажити мову документа', {
          description: 'Поки дані запису не прочитано, мова визначається автоматично.',
        });
        if (!cancelled && languageLoadVersionRef.current === loadVersion) setSummaryLang(null);
      }
    };

    loadSummaryLanguage();

    return () => {
      cancelled = true;
    };
  }, [meeting.id]);

  const persistLatestLanguageSelection = async () => {
    if (languageSaveLoopRunningRef.current) return;
    languageSaveLoopRunningRef.current = true;

    try {
      while (true) {
        const request = latestLanguageSaveRequestRef.current;
        if (!request) return;

        try {
          const saved = await saveMeetingSummaryLanguage(request.meetingId, request.language);
          const latest = latestLanguageSaveRequestRef.current;
          if (
            latest?.version === request.version &&
            activeMeetingIdRef.current === request.meetingId
          ) {
            setSummaryLang(saved.language);
            setSummaryLangStorage(saved.storage);
            if (saved.storage === 'local_fallback') {
              toast.info('Мову документа збережено на цьому комп’ютері', {
                description: 'Для цього запису немає папки зі звуком.',
              });
            }
            if (request.language) {
              addRecent(request.language);
            }
            return;
          }

          if (latest?.version === request.version) return;
        } catch (err) {
          const latest = latestLanguageSaveRequestRef.current;
          if (
            latest?.version === request.version &&
            activeMeetingIdRef.current === request.meetingId
          ) {
            console.error('Failed to persist summary language:', err);
            toast.error('Не вдалося зберегти мову документа');
            setSummaryLang(request.rollback.language);
            setSummaryLangStorage(request.rollback.storage);
            return;
          }

          console.warn('Ignoring failed stale summary language save:', err);
          if (latest?.version === request.version) return;
        }
      }
    } finally {
      languageSaveLoopRunningRef.current = false;
    }
  };

  const handleLangChange = (code: string | null) => {
    const previous = summaryLang;
    const previousStorage = summaryLangStorage;
    const nextStored = code;
    languageLoadVersionRef.current += 1;
    latestLanguageSaveRequestRef.current = {
      version: languageSaveVersionRef.current + 1,
      meetingId: meeting.id,
      language: nextStored,
      rollback: {
        language: previous,
        storage: previousStorage,
      },
    };
    languageSaveVersionRef.current += 1;
    setSummaryLang(nextStored);
    setLangPickerOpen(false);
    void persistLatestLanguageSelection();
  };

  const isSummaryLoading = summaryStatus === 'processing' || summaryStatus === 'summarizing' || summaryStatus === 'regenerating';
  const hasSummary = hasVisibleSummaryContent(aiSummary);
  // Voices still called «Спікер N» in the document: a hint, not part of it
  const unnamedVoices = (() => {
    if (!hasSummary) return 0;
    const found = JSON.stringify(aiSummary).match(/Спікер \d+/g) ?? [];
    return new Set(found).size;
  })();

  const languageSlot = (
    <Popover open={langPickerOpen} onOpenChange={setLangPickerOpen}>
      <PopoverTrigger asChild>
        <button
          type="button"
          className="tb-btn tb-side"
          title={`Мова документа: ${effectiveLangLabel}${isLocalFallbackLanguage ? ' (збережено на цьому комп’ютері)' : ''}`}
          aria-label="Мова документа"
        >
          <Globe className="h-4 w-4" />
          <span className="tb-label">{effectiveLangLabel}</span>
          <ChevronDown className="tb-chevron h-3.5 w-3.5" aria-hidden="true" />
        </button>
      </PopoverTrigger>
      <PopoverContent
        align="end"
        className="w-auto p-0 border-0 shadow-none bg-transparent"
      >
        <LanguagePickerPopover
          value={summaryLang}
          onChange={handleLangChange}
          onClose={() => setLangPickerOpen(false)}
          autoSubtitle={autoSubtitle}
        />
      </PopoverContent>
    </Popover>
  );

  const colors = React.useMemo(() => speakerColorMap(transcripts.map((t) => t.speaker)), [transcripts]);

  return (
    <div className="card tv relative flex h-full min-w-0 w-full flex-col overflow-hidden">
      {/* The same row as on the transcript: the page's own controls on the
          left, the shared actions on the right */}
      <div className="tv-toolbar" data-notice-anchor>
        <button type="button" className="tv-icon tv-hist" onClick={() => summaryRef.current?.undo?.()} disabled={!hasSummary || isSummaryLoading} title="Скасувати · ⌘Z" aria-label="Скасувати">
          <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" aria-hidden="true"><path d="M9 14L4 9l5-5" /><path d="M4 9h10a6 6 0 010 12h-3" /></svg>
        </button>
        <button type="button" className="tv-icon tv-hist" onClick={() => summaryRef.current?.redo?.()} disabled={!hasSummary || isSummaryLoading} title="Повторити · ⌘⇧Z" aria-label="Повторити">
          <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" aria-hidden="true"><path d="M15 14l5-5-5-5" /><path d="M20 9H10a6 6 0 000 12h3" /></svg>
        </button>
        <span className="tb-sep tv-hist" aria-hidden="true" />
        <SummaryGeneratorButtonGroup
          modelConfig={modelConfig}
          setModelConfig={setModelConfig}
          onSaveModelConfig={onSaveModelConfig}
          onGenerateSummary={onGenerateSummary}
          onStopGeneration={onStopGeneration}
          customPrompt={customPrompt}
          summaryStatus={summaryStatus}
          hasTranscripts={transcripts.length > 0}
          hasSummary={hasSummary}
          isModelConfigLoading={isModelConfigLoading}
          onOpenModelSettings={onOpenModelSettings}
          languageSlot={transcripts.length > 0 || hasSummary ? languageSlot : undefined}
          templateSlot={hasSummary ? templateSlot : undefined}
          showModelButton={!templateSlot}
        />
        {hasSummary && !isSummaryLoading && (isSummaryDirty || isSaving) && (
          <button
            type="button"
            className="tb-btn"
            onClick={() => {
              Analytics.trackButtonClick('save_changes', 'meeting_details');
              void onSaveAll();
            }}
            disabled={isSaving}
            title="Зберегти зміни"
          >
            {isSaving ? <Loader2 className="h-4 w-4 animate-spin" /> : <Save className="h-4 w-4" />}
            <span className="tb-label">{isSaving ? 'Збереження…' : 'Зберегти'}</span>
          </button>
        )}
        <div className="flex-1" />
        {searching ? (
          <input
            autoFocus
            className="tv-search"
            placeholder="Пошук"
            aria-label="Пошук у документі"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => {
              // WebKit's own find: selects the next match and scrolls to it
              if (e.key === 'Enter' && query.trim()) (window as any).find?.(query.trim(), false, e.shiftKey, true);
              if (e.key === 'Escape') setSearching(false);
            }}
            onBlur={() => !query && setSearching(false)}
          />
        ) : (
          <button type="button" className="tv-icon tv-find" aria-label="Пошук" title="Пошук" onClick={() => setSearching(true)} disabled={!hasSummary}>
            <Search className="h-4 w-4" />
          </button>
        )}
        {toolbarRight}
      </div>
      {banner && <div className="doc-banner-slot">{banner}</div>}

      {isSummaryLoading ? (
        <div className="flex items-center justify-center flex-1">
          <DocumentProgress />
        </div>
      ) : !hasSummary && emptyState ? (
        <div className="flex flex-1 min-h-0 flex-col">{emptyState}</div>
      ) : !hasSummary ? (
        <div className="flex flex-1 min-h-0 flex-col">
          <EmptyStateSummary
            onGenerate={() => onGenerateSummary(customPrompt)}
            hasModel={modelConfig.provider !== null && modelConfig.model !== null}
            isGenerating={isSummaryLoading}
            error={summaryError}
          />
        </div>
      ) : (
        <div className="doc-surface custom-scrollbar" onClick={onTextClick}>
          {unnamedVoices > 0 && (
            <p className="doc-note">
              У документі {unnamedVoices} {unnamedVoices === 1 ? 'голос' : unnamedVoices < 5 ? 'голоси' : 'голосів'} без імені. Підпишіть їх у «Спікери» над стенограмою: наступного разу ці голоси підпишуться самі.
            </p>
          )}
          <div className="doc-sheet relative">
            <DocSectionCopy summaryRef={summaryRef} />
            <BlockNoteSummaryView
              key={docKey}
              ref={summaryRef}
              summaryData={aiSummary}
              onSave={onSaveSummary}
              onSummaryChange={onSummaryChange}
              onDirtyChange={onDirtyChange}
              status={summaryStatus}
              error={summaryError}
              speakerColors={colors}
              onRegenerateSummary={() => {
                Analytics.trackButtonClick('regenerate_summary', 'meeting_details');
                onRegenerateSummary();
              }}
              meeting={{
                id: meeting.id,
                title: meetingTitle,
                created_at: meeting.created_at
              }}
            />
          </div>
          {/* "Completed" needs no banner: the protocol itself is the result */}
          {summaryStatus !== 'idle' && summaryStatus !== 'completed' && (
            <div className={`doc-note mt-4 px-4 py-3 rounded-[10px] ${summaryStatus === 'error' ? 'bg-red-100 text-red-700' : 'bg-[var(--selected)] text-gray-900'}`}>
              <p className="text-sm font-medium">{getSummaryStatusMessage(summaryStatus)}</p>
            </div>
          )}
        </div>
      )}
      {footer}
    </div>
  );
}
