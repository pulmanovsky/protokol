'use client';

import { useEffect, useState } from 'react';
import { GlossarySettings } from '@/components/GlossarySettings';
import { GlossarySuggestions } from '@/components/GlossarySuggestions';
import { GlossaryLearned } from '@/components/GlossaryLearned';
import { LearnedCorrections } from '@/components/LearnedCorrections';
import { LearnedOverview } from '@/components/LearnedOverview';
import { MemoryPanel } from '@/components/MemoryPanel';
import { useCallback } from 'react';

export default function GlossaryPage() {
  // Re-read the glossary after a suggestion is accepted
  const [version, setVersion] = useState(0);
  const bump = useCallback(() => setVersion((v) => v + 1), []);
  // «Пам'ять» first; ?tab=words opens the words (a link from the transcript may ask for either)
  const [tab, setTab] = useState<'memory' | 'words'>('memory');
  useEffect(() => {
    try {
      if (new URLSearchParams(window.location.search).get('tab') === 'words') setTab('words');
    } catch {
      // the default tab
    }
  }, []);
  return (
    <div className="h-full overflow-y-auto custom-scrollbar">
      <div className="flex flex-col gap-6 p-8 max-w-[860px]">
        <header data-tauri-drag-region data-notice-anchor className="flex flex-wrap items-end justify-between gap-4">
          <div className="flex flex-col gap-0.5">
            <h1 data-tauri-drag-region className="text-[30px] font-bold tracking-[-0.6px] text-gray-900">Словник</h1>
            <span data-tauri-drag-region className="text-sm text-[color:var(--muted-text)]">
              {tab === 'memory' ? "Що програма бере з ваших виправлень" : 'Як правильно писати імена, посади та абревіатури'}
            </span>
          </div>
          <div className="step-switch" role="tablist" aria-label="Розділ словника">
            <button type="button" role="tab" aria-selected={tab === 'memory'} data-on={tab === 'memory'} onClick={() => setTab('memory')}>
              Пам&apos;ять
            </button>
            <button type="button" role="tab" aria-selected={tab === 'words'} data-on={tab === 'words'} onClick={() => setTab('words')}>
              Слова
            </button>
          </div>
        </header>
        {tab === 'memory' ? (
          <MemoryPanel version={version} onChanged={bump} />
        ) : (
          <>
            <GlossarySuggestions onChanged={bump} />
            <LearnedOverview version={version} />
            <LearnedCorrections version={version} />
            <GlossaryLearned version={version} onChanged={bump} />
            <GlossarySettings key={version} />
          </>
        )}
      </div>
    </div>
  );
}
