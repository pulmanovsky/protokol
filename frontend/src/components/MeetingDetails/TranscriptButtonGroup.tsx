"use client";

import { useState, useCallback, useEffect } from 'react';
import { OPEN_SPEAKERS_EVENT } from '@/lib/timeline';
import { Copy, Download, RefreshCw, Users } from 'lucide-react';
import { ToolbarMenu } from './ToolbarMenu';
import Analytics from '@/lib/analytics';
import { RetranscribeDialog } from './RetranscribeDialog';
import { SpeakersDialog } from './SpeakersDialog';
import { useConfig } from '@/contexts/ConfigContext';
import { toast } from '@/lib/notice';
import { invoke } from '@tauri-apps/api/core';


interface TranscriptButtonGroupProps {
  transcriptCount: number;
  onCopyTranscript: () => void;
  onOpenMeetingFolder: () => Promise<void>;
  meetingId?: string;
  meetingFolderPath?: string | null;
  onRefetchTranscripts?: () => Promise<void>;
  onJumpTo?: (seconds: number) => void;
}


export function TranscriptButtonGroup({
  transcriptCount,
  onCopyTranscript,
  onOpenMeetingFolder,
  meetingId,
  meetingFolderPath,
  onRefetchTranscripts,
  onJumpTo,
}: TranscriptButtonGroupProps) {
  const { betaFeatures } = useConfig();
  // The stenogram as a file: header, turns by person (no model involved)
  const saveStenogram = async (kind: 'docx' | 'pdf') => {
    try {
      const { title, markdown } = await invoke<{ title: string; markdown: string }>('api_stenogram_markdown', { meetingId });
      const path = await invoke<string | null>(kind === 'docx' ? 'api_export_protocol_docx' : 'api_export_protocol_pdf', { title, markdown });
      if (path) toast.success('Стенограму збережено', { description: path });
    } catch (e) {
      toast.error(`${e}`);
    }
  };
  const [showRetranscribeDialog, setShowRetranscribeDialog] = useState(false);
  const [showSpeakersDialog, setShowSpeakersDialog] = useState(false);
  // «Переглянути» in the notice after speaker recognition opens this window
  useEffect(() => {
    const open = () => setShowSpeakersDialog(true);
    window.addEventListener(OPEN_SPEAKERS_EVENT, open);
    return () => window.removeEventListener(OPEN_SPEAKERS_EVENT, open);
  }, []);

  const handleRetranscribeComplete = useCallback(async () => {
    // Refetch transcripts to show the updated data
    if (onRefetchTranscripts) {
      await onRefetchTranscripts();
    }
  }, [onRefetchTranscripts]);

  return (
    <>
      {meetingId && meetingFolderPath && (
        <button type="button" className="tv-icon" onClick={() => setShowSpeakersDialog(true)} disabled={transcriptCount === 0} aria-label="Спікери" title="Спікери: голоси, імена, ролі">
          <Users className="h-4 w-4" />
        </button>
      )}
      {betaFeatures.importAndRetranscribe && meetingId && meetingFolderPath && (
        <button
          type="button"
          className="tv-icon"
          onClick={() => {
            Analytics.trackButtonClick('enhance_transcript', 'meeting_details');
            setShowRetranscribeDialog(true);
          }}
          aria-label="Розпізнати заново"
          title="Розпізнати заново"
        >
          <RefreshCw className="h-4 w-4" />
        </button>
      )}
      <span className="tb-sep" aria-hidden="true" />
      <button
        type="button"
        className="tb-btn"
        onClick={() => {
          Analytics.trackButtonClick('copy_transcript', 'meeting_details');
          onCopyTranscript();
        }}
        disabled={transcriptCount === 0}
        title={transcriptCount === 0 ? 'Стенограми ще немає' : 'Копіювати стенограму'}
      >
        <Copy className="h-4 w-4" />
        <span className="tb-label">Копіювати</span>
      </button>
      <ToolbarMenu
        label="Експорт"
        icon={<Download className="h-4 w-4" />}
        items={[
          ...(meetingId ? [{ label: 'Стенограма у DOCX…', disabled: transcriptCount === 0, run: () => void saveStenogram('docx') }, { label: 'Стенограма у PDF…', disabled: transcriptCount === 0, run: () => void saveStenogram('pdf') }] : []),
          {
            label: 'Показати запис у Finder',
            run: () => {
              Analytics.trackButtonClick('open_recording_folder', 'meeting_details');
              void onOpenMeetingFolder();
            },
          },
        ]}
      />
      {meetingId && meetingFolderPath && (
        <SpeakersDialog
          open={showSpeakersDialog}
          onOpenChange={setShowSpeakersDialog}
          meetingId={meetingId}
          onChanged={handleRetranscribeComplete}
          onJumpTo={onJumpTo}
        />
      )}

      {betaFeatures.importAndRetranscribe && meetingId && meetingFolderPath && (
        <RetranscribeDialog
          open={showRetranscribeDialog}
          onOpenChange={setShowRetranscribeDialog}
          meetingId={meetingId}
          meetingFolderPath={meetingFolderPath}
          onComplete={handleRetranscribeComplete}
        />
      )}
    </>
  );
}
