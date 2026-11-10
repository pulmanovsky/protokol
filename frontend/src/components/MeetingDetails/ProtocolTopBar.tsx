'use client';
import { confirmNative } from '@/lib/confirm';

import { ArrowRight, Check, ChevronDown, Copy, Download, Loader2, RefreshCw, Undo2, Users } from 'lucide-react';
import { OPEN_SPEAKERS_EVENT } from '@/lib/timeline';
import { ToolbarMenu } from './ToolbarMenu';

export type ProtocolStatus = 'draft' | 'approved';
export type MeetingStep = 'transcript' | 'document';

interface ProtocolTopBarProps {
  title: string;
  createdAt?: string;
  durationSeconds: number | null;
  status: ProtocolStatus;
  hasProtocol: boolean;
  isGenerating: boolean;
  isExporting: boolean;
  isSavingStatus: boolean;
  onRegenerate: () => void;
  onToggleApproved: () => void;
  onExportDocx: () => void;
  onExportPdf?: () => void;
  /** Two steps: check the transcript, then the document */
  step: MeetingStep;
  onStep: (step: MeetingStep) => void;
  /** People heard in the meeting */
  people?: number;
}

function duration(durationSeconds: number | null): string | null {
  if (!durationSeconds || durationSeconds <= 0) return null;
  const minutes = Math.max(1, Math.round(durationSeconds / 60));
  return minutes >= 60 ? `${Math.floor(minutes / 60)} год ${minutes % 60} хв` : `${minutes} хв`;
}

/**
 * The right of the document's toolbar, in the same order and look as on the
 * transcript: people, create again, «Копіювати», «Експорт ⌄» (DOCX, PDF).
 */
export function DocumentActions({
  hasProtocol,
  isGenerating,
  isExporting,
  onRegenerate,
  onExportDocx,
  onExportPdf,
  onCopy,
}: Pick<ProtocolTopBarProps, 'hasProtocol' | 'isGenerating' | 'isExporting' | 'onRegenerate' | 'onExportDocx' | 'onExportPdf'> & { onCopy: () => void }) {
  return (
    <>
      <button type="button" className="tv-icon" onClick={() => window.dispatchEvent(new Event(OPEN_SPEAKERS_EVENT))} aria-label="Спікери" title="Спікери: голоси, імена, ролі">
        <Users className="h-4 w-4" />
      </button>
      <button
        type="button"
        className="tv-icon"
        onClick={async () => {
          // A document that exists is not thrown away by a stray click
          if (hasProtocol && !(await confirmNative('Створити документ заново?', 'Поточний текст замінить нова версія. Попередня збережеться як резервна.', 'Створити заново'))) return;
          onRegenerate();
        }}
        disabled={isGenerating}
        aria-label={hasProtocol ? 'Створити заново' : 'Створити документ'}
        title={hasProtocol ? 'Створити заново' : 'Створити документ'}
      >
        {isGenerating ? <Loader2 className="h-4 w-4 animate-spin" /> : <RefreshCw className="h-4 w-4" />}
      </button>
      <span className="tb-sep" aria-hidden="true" />
      <button type="button" className="tb-btn" onClick={onCopy} disabled={!hasProtocol} title={hasProtocol ? 'Копіювати документ' : 'Документа ще немає'}>
        <Copy className="h-4 w-4" />
        <span className="tb-label">Копіювати</span>
      </button>
      <ToolbarMenu
        label="Експорт"
        icon={isExporting ? <Loader2 className="h-4 w-4 animate-spin" /> : <Download className="h-4 w-4" />}
        disabled={!hasProtocol || isExporting}
        items={[{ label: 'Документ у DOCX…', run: onExportDocx }, ...(onExportPdf ? [{ label: 'Документ у PDF…', run: onExportPdf }] : [])]}
      />
    </>
  );
}

/**
 * The meeting's header, the same on both pages: the full title with
 * «тип · тривалість · люди · Затверджено» under it, the «Стенограма |
 * Документ» switch and one main action of a fixed width («Далі: документ»
 * or «Затвердити / У чернетку»). The title takes all the room left.
 */
export function ProtocolTopBar({
  title,
  durationSeconds,
  status,
  hasProtocol,
  isGenerating,
  isExporting,
  isSavingStatus,
  onRegenerate,
  onToggleApproved,
  onExportDocx,
  onExportPdf,
  step,
  onStep,
  people,
}: ProtocolTopBarProps) {
  const approved = status === 'approved';
  const meta = [duration(durationSeconds), people ? `${people} ${people === 1 ? 'людина' : people < 5 ? 'людини' : 'людей'}` : null].filter(Boolean).join(' · ');

  return (
    <header data-tauri-drag-region data-notice-anchor className="protocol-bar glass-bar @container grid items-center gap-3 px-4 py-2.5 min-h-[62px]">
      <div className="flex min-w-0 flex-col gap-0.5">
        <span data-tauri-drag-region className="truncate text-[17px] font-bold leading-tight text-gray-900" title={title}>
          {title}
        </span>
        <div className="flex min-w-0 items-center gap-1.5 text-[12px] text-[color:var(--muted-text)]">
          {meta && <span className="flex-none whitespace-nowrap">{meta}</span>}
          {approved && <span className="pill-approved inline-flex h-[18px] flex-none items-center rounded-full px-2 text-[11.5px] font-semibold">Затверджено</span>}
        </div>
      </div>

      <div className="step-switch" role="tablist" aria-label="Сторінка запису">
        <button type="button" role="tab" aria-selected={step === 'transcript'} data-on={step === 'transcript'} onClick={() => onStep('transcript')} title="Стенограма">
          Стенограма
        </button>
        <button type="button" role="tab" aria-selected={step === 'document'} data-on={step === 'document'} onClick={() => onStep('document')} title="Документ">
          Документ
        </button>
      </div>

      {/* One main action, the same width on both pages, so nothing moves */}
      {step === 'transcript' ? (
        <button
          type="button"
          className="accent-button protocol-main h-9 gap-1.5 whitespace-nowrap text-[13px] font-semibold"
          title="Перейти до документа"
          onClick={() => onStep('document')}
        >
          Далі: документ <ArrowRight className="h-4 w-4" />
        </button>
      ) : (
        <button
          type="button"
          onClick={onToggleApproved}
          disabled={!hasProtocol || isSavingStatus}
          className={`${approved ? 'secondary-button' : 'accent-button'} protocol-main h-9 gap-1.5 whitespace-nowrap text-[13px] font-semibold disabled:opacity-50`}
          title={approved ? 'Повернути документ у чернетку' : 'Затвердити документ'}
        >
          {approved ? <Undo2 className="h-4 w-4" /> : <Check className="h-4 w-4" />}
          {approved ? 'У чернетку' : 'Затвердити'}
        </button>
      )}
    </header>
  );
}
