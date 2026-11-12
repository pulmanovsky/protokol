"use client";
import { LowPowerNotice } from '@/components/LowPowerNotice';
import { useState, useEffect, useRef, useMemo } from 'react';
import { motion } from 'framer-motion';
import { MeetingSummary, SummaryProcessResponse, Transcript } from '@/types';
import { useSidebar } from '@/components/Sidebar/SidebarProvider';
import Analytics from '@/lib/analytics';
import { invoke, convertFileSrc } from '@tauri-apps/api/core';
import { toast } from '@/lib/notice';
import { TranscriptPanel } from '@/components/MeetingDetails/TranscriptPanel';
import { TranscriptWorkspace } from '@/components/MeetingDetails/TranscriptWorkspace';
import { Timeline, type TimelineLine } from '@/components/MeetingDetails/Timeline';
import { speakerColorMap } from '@/lib/speakerColors';
import { SummaryPanel } from '@/components/MeetingDetails/SummaryPanel';
import { TemplatePicker } from '@/components/MeetingDetails/TemplatePicker';
import { TemplateMenu } from '@/components/MeetingDetails/TemplateMenu';
import { parseSummaryContent } from '@/lib/summary-content';
import { estimateLabel, isTranscriptOnly } from '@/lib/documentTemplates';
import { confirmNative } from '@/lib/confirm';
import { DocSourcePanel } from '@/components/MeetingDetails/DocSourcePanel';
import { followPlayhead, setPlayheadRunning, setPlayheadSource } from '@/lib/playhead';
import { toPlayer, toSound, type TimePoints } from '@/lib/playerTime';
import { useNumberCheck } from '@/components/MeetingDetails/NumberCheck';
import { perfCount } from '@/lib/perf';
import { NO_SOURCE, countNoSource } from '@/lib/sourceMarks';
import { DocumentActions, ProtocolTopBar, type ProtocolStatus, type MeetingStep } from '@/components/MeetingDetails/ProtocolTopBar';
import { hasVisibleSummaryContent } from '@/lib/summary-content';
import { ModelConfig } from '@/components/ModelSettingsModal';

// Custom hooks
import { useMeetingData } from '@/hooks/meeting-details/useMeetingData';
import { useSummaryGeneration } from '@/hooks/meeting-details/useSummaryGeneration';
import { useTemplates } from '@/hooks/meeting-details/useTemplates';
import { useSpeakerRecognition } from '@/hooks/meeting-details/useSpeakerRecognition';
import { JUMP_TO_TIME_EVENT, PLAY_FROM_EVENT, PLAY_RANGE_EVENT, TIME_CLICKED_EVENT, mmss, MARK_ALL_REVIEWED_EVENT } from '@/lib/timeline';
import { useCopyOperations } from '@/hooks/meeting-details/useCopyOperations';
import { useMeetingOperations } from '@/hooks/meeting-details/useMeetingOperations';
import { useConfig } from '@/contexts/ConfigContext';

export default function PageContent({
  meeting,
  summaryData,
  initialSummary,
  shouldAutoGenerate = false,
  onAutoGenerateComplete,
  onMeetingUpdated,
  onRefetchTranscripts,
  // Pagination props for efficient transcript loading
  segments,
  hasMore,
  isLoadingMore,
  totalCount,
  loadedCount,
  onLoadMore,
}: {
  meeting: any;
  summaryData: MeetingSummary | null;
  initialSummary: SummaryProcessResponse | null;
  shouldAutoGenerate?: boolean;
  onAutoGenerateComplete?: () => void;
  onMeetingUpdated?: () => Promise<void>;
  onRefetchTranscripts?: () => Promise<void>;
  // Pagination props
  segments?: any[];
  hasMore?: boolean;
  isLoadingMore?: boolean;
  totalCount?: number;
  loadedCount?: number;
  onLoadMore?: () => void;
}) {
  perfCount('render:page');
  console.log('📄 PAGE CONTENT: Initializing with data:', {
    meetingId: meeting.id,
    summaryDataKeys: summaryData ? Object.keys(summaryData) : null,
    transcriptsCount: meeting.transcripts?.length
  });

  // State
  const isRecording = false;
  // Two steps: 1 check the transcript, 2 the document (remembered per meeting)
  const stepKey = `protokol.step.${meeting.id}`;
  const [step, setStepState] = useState<MeetingStep>(() => {
    try {
      return window.localStorage.getItem(stepKey) === 'document' ? 'document' : 'transcript';
    } catch {
      return 'transcript';
    }
  });
  const setStep = (next: MeetingStep) => {
    setStepState(next);
    try {
      window.localStorage.setItem(stepKey, next);
    } catch {
      // not remembered: fine
    }
  };
  const setActiveTab = (tab: 'transcript' | 'summary') => setStep(tab === 'summary' ? 'document' : 'transcript');
  const [reviewProgress, setReviewProgress] = useState<{ reviewed: number; total: number; seconds_reviewed: number; seconds_total: number } | null>(null);
  // «Джерело пункту»: the moment a mark of the document points to
  const [source, setSource] = useState<{ seconds: number; markdown: string; unknown?: string } | null>(null);


  // Ref to store the modal open function from SummaryGeneratorButtonGroup
  const openModelSettingsRef = useRef<(() => void) | null>(null);
  const autoSwitchedSummaryMeetingIdsRef = useRef(new Set<string>());
  const manuallySelectedTabMeetingIdsRef = useRef(new Set<string>());
  const autoGenerationStartedMeetingIdRef = useRef<string | null>(null);

  // Sidebar context
  const { serverAddress } = useSidebar();

  // Get model config from ConfigContext
  const { modelConfig, setModelConfig, isModelConfigLoading } = useConfig();

  // Custom hooks
  const meetingData = useMeetingData({ meeting, summaryData, onMeetingUpdated });
  const peopleCount = useMemo(
    () => new Set(meetingData.transcripts.map((t: { speaker?: string }) => t.speaker).filter((s: string | undefined): s is string => !!s && s !== "Репліки" && s !== "…")).size,
    [meetingData.transcripts],
  );
  const templates = useTemplates(meeting);
  // Speakers are recognised on their own after a recording or an import
  const speakerRecognition = useSpeakerRecognition(meeting.id, async () => {
    await onRefetchTranscripts?.();
  });

  // Callback to register the modal open function
  const handleRegisterModalOpen = (openFn: () => void) => {
    console.log('📝 Registering modal open function in PageContent');
    openModelSettingsRef.current = openFn;
  };

  // Callback to trigger modal open (called from error handler)
  const handleOpenModelSettings = () => {
    console.log('🔔 Opening model settings from PageContent');
    if (openModelSettingsRef.current) {
      openModelSettingsRef.current();
    } else {
      console.warn('⚠️ Modal open function not yet registered');
    }
  };

  // Save model config to backend database and sync via event
  const handleSaveModelConfig = async (config?: ModelConfig) => {
    if (!config) return;
    try {
      await invoke('api_save_model_config', {
        provider: config.provider,
        model: config.model,
        whisperModel: config.whisperModel,
        apiKey: config.apiKey ?? null,
        ollamaEndpoint: config.ollamaEndpoint ?? null,
      });

      // Emit event so ConfigContext and other listeners stay in sync
      const { emit } = await import('@tauri-apps/api/event');
      await emit('model-config-updated', config);

      toast.success('Налаштування моделі збережено');
    } catch (error) {
      console.error('Failed to save model config:', error);
      toast.error('Не вдалося зберегти налаштування моделі');
    }
  };

  const summaryGeneration = useSummaryGeneration({
    initialSummary,
    meeting,
    transcripts: meetingData.transcripts,
    modelConfig: modelConfig,
    isModelConfigLoading,
    selectedTemplate: templates.selectedTemplate,
    onMeetingUpdated,
    updateMeetingTitle: meetingData.updateMeetingTitle,
    setAiSummary: meetingData.setAiSummary,
    onOpenModelSettings: handleOpenModelSettings,
  });

  const copyOperations = useCopyOperations({
    meeting,
    transcripts: meetingData.transcripts,
    meetingTitle: meetingData.meetingTitle,
    aiSummary: meetingData.aiSummary,
    blockNoteSummaryRef: meetingData.blockNoteSummaryRef,
  });

  const meetingOperations = useMeetingOperations({
    meeting,
  });

  // Track page view
  useEffect(() => {
    Analytics.trackPageView('meeting_details');
  }, []);

  // Protocol review status (Чернетка / Затверджено)
  const [status, setStatus] = useState<ProtocolStatus>(meeting.status === 'approved' ? 'approved' : 'draft');
  const [isSavingStatus, setIsSavingStatus] = useState(false);
  const [isExporting, setIsExporting] = useState(false);
  useEffect(() => {
    setStatus(meeting.status === 'approved' ? 'approved' : 'draft');
  }, [meeting.id, meeting.status]);

  const hasProtocol = hasVisibleSummaryContent(meetingData.aiSummary);
  const isGenerating = ['processing', 'summarizing', 'regenerating'].includes(summaryGeneration.summaryStatus);
  // The meeting's own duration (the list's); the loaded transcript lines are
  // only the first pages of a long meeting
  const { meetings: sidebarMeetings } = useSidebar();
  const listedDuration = (sidebarMeetings as any[] | undefined)?.find((m) => m.id === meeting.id)?.durationSeconds;
  const durationSeconds = listedDuration ?? meetingData.transcripts.reduce(
    (max: number, t: any) => Math.max(max, t.audio_end_time ?? 0),
    0,
  );

  // Documents by template: the one shown, the others kept; a note when the
  // transcript changed since the document shown was made
  const [docKey, setDocKey] = useState('');
  const [transcriptChanged, setTranscriptChanged] = useState(false);
  const [staleDismissed, setStaleDismissed] = useState(false);
  const docTemplate = templates.shownTemplate ?? templates.selectedTemplate;
  const checkTranscriptChanged = async () => {
    const id = meeting.id;
    const text = await summaryGeneration.currentTranscriptText();
    if (!text) return;
    try {
      const shown = await invoke<{ templateId: string | null; transcriptChanged: boolean }>('api_shown_document', { meetingId: id, text });
      if (id === meeting.id) setTranscriptChanged(shown.transcriptChanged);
    } catch {
      // the note is a convenience
    }
  };
  useEffect(() => {
    setTranscriptChanged(false);
    setStaleDismissed(false);
  }, [meeting.id]);
  useEffect(() => {
    if (step === 'document' && hasProtocol && !isGenerating) void checkTranscriptChanged();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [step, hasProtocol, isGenerating, meeting.id]);
  // A document just made: it is the one shown now, and kept under its template
  const lastStatusRef = useRef(summaryGeneration.summaryStatus);
  useEffect(() => {
    const was = lastStatusRef.current;
    lastStatusRef.current = summaryGeneration.summaryStatus;
    if (summaryGeneration.summaryStatus === 'completed' && was !== 'completed') {
      void templates.refreshVariants();
      setTranscriptChanged(false);
      setStaleDismissed(false);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [summaryGeneration.summaryStatus]);

  /** Unsaved edits stay with the document they were made in */
  const saveEditsFirst = async () => {
    if (meetingData.isSummaryDirty) await meetingData.saveAllChanges();
  };

  const showDocument = async (templateId: string) => {
    try {
      await saveEditsFirst();
      const ok = await invoke<boolean>('api_show_document_variant', { meetingId: meeting.id, templateId });
      if (!ok) {
        toast.info('Цей документ ще не створено');
        return;
      }
      const existing = await invoke<SummaryProcessResponse>('api_get_summary', { meetingId: meeting.id });
      meetingData.setAiSummary(parseSummaryContent(existing.data));
      setDocKey(`${templateId}:${Date.now()}`);
      templates.setSelectedTemplate(templateId);
      await templates.refreshVariants();
      setStaleDismissed(false);
      void checkTranscriptChanged();
    } catch (error) {
      console.error('Failed to show the document:', error);
      toast.error('Не вдалося відкрити документ');
    }
  };

  const createWithTemplate = async (templateId: string) => {
    if (isGenerating) return;
    // A click in the menu costs minutes of the model: asked once
    const template = templates.templates.find((t) => t.id === templateId);
    if (hasProtocol && template && !isTranscriptOnly(template)) {
      const ok = await confirmNative(
        `Створити документ за шаблоном «${template.name}»?`,
        `Це займе ${estimateLabel(template, durationSeconds ? durationSeconds / 60 : null)}. Поточний документ збережеться, до нього можна повернутися в цьому ж меню.`,
        'Створити',
      );
      if (!ok) return;
    }
    await saveEditsFirst();
    // A document made before templates were kept: keep it under the
    // template it was most likely made with, so it is not lost
    if (hasProtocol && !templates.shownTemplate) {
      await invoke('api_keep_shown_document', { meetingId: meeting.id, templateId: templates.selectedTemplate }).catch(() => {});
    }
    templates.setSelectedTemplate(templateId);
    if (hasProtocol) await summaryGeneration.handleRegenerateSummary(templateId, 'template-menu');
    else await summaryGeneration.handleGenerateSummary('', templateId, 'template-menu');
  };

  const handleToggleApproved = async () => {
    const next: ProtocolStatus = status === 'approved' ? 'draft' : 'approved';
    setIsSavingStatus(true);
    try {
      const result = await invoke<{ status: string; edits?: number; examples?: number }>('api_set_meeting_status', { meetingId: meeting.id, status: next });
      setStatus(next);
      if (next === 'approved') {
        const edits = result?.edits ?? 0;
        toast.success('Підсумок затверджено', {
          description: edits > 0
            ? `Додано в пам'ять: ${edits} ${edits === 1 ? 'виправлення' : edits < 5 ? 'виправлення' : 'виправлень'} для наступних записів цього типу`
            : (result?.examples ?? 0) > 0 ? 'Додано в пам\'ять, як оформлено виступи, для наступних записів цього типу' : undefined,
        });
      } else {
        toast.success('Підсумок повернуто в чернетку');
      }
      onMeetingUpdated?.();
    } catch (error) {
      console.error('Failed to set meeting status:', error);
      toast.error('Не вдалося змінити статус підсумку');
    } finally {
      setIsSavingStatus(false);
    }
  };

  const getProtocolMarkdown = async (): Promise<string> => {
    const fromEditor = await meetingData.blockNoteSummaryRef.current?.getMarkdown?.();
    if (fromEditor) return fromEditor;
    const summary = meetingData.aiSummary as any;
    return typeof summary?.markdown === 'string' ? summary.markdown : '';
  };

  const handleExportDocx = async () => {
    setIsExporting(true);
    try {
      const markdown = await getProtocolMarkdown();
      if (!markdown.trim()) {
        toast.error('Підсумок порожній');
        return;
      }
      const path = await invoke<string | null>('api_export_protocol_docx', {
        title: meetingData.meetingTitle || 'Стенограма',
        markdown,
      });
      if (path) toast.success('Підсумок збережено', { description: path });
    } catch (error) {
      console.error('Failed to export DOCX:', error);
      toast.error(typeof error === 'string' ? error : 'Не вдалося зберегти DOCX');
    } finally {
      setIsExporting(false);
    }
  };

  const handleExportPdf = async () => {
    setIsExporting(true);
    try {
      const markdown = await getProtocolMarkdown();
      if (!markdown.trim()) {
        toast.error('Документ порожній');
        return;
      }
      const path = await invoke<string | null>('api_export_protocol_pdf', {
        title: meetingData.meetingTitle || 'Стенограма',
        markdown,
      });
      if (path) toast.success('PDF збережено', { description: path });
    } catch (error) {
      console.error('Failed to export PDF:', error);
      toast.error(typeof error === 'string' ? error : 'Не вдалося зберегти PDF');
    } finally {
      setIsExporting(false);
    }
  };

  const handleRegenerate = (trigger: string) => {
    if (hasProtocol) void summaryGeneration.handleRegenerateSummary(docTemplate, trigger);
    else void summaryGeneration.handleGenerateSummary('', templates.selectedTemplate, trigger);
  };

  // Points of the document whose source was not found (⟨?⟩): counted in the
  // banner; the editor itself tints them amber (see lib/docLayout: a class
  // set here by hand was wiped by the editor's next redraw)
  const [noSource, setNoSource] = useState(0);
  const docRef = useRef<HTMLDivElement | null>(null);
  useEffect(() => {
    if (step !== 'document') return;
    const tint = () => {
      const root = docRef.current;
      if (!root) return;
      let n = 0;
      root.querySelectorAll('.bn-block-content').forEach((el) => {
        if ((el.textContent ?? '').split('Не враховано:')[0].includes(NO_SOURCE)) n += 1;
      });
      setNoSource(n);
    };
    tint();
    const observer = new MutationObserver(() => window.requestAnimationFrame(tint));
    if (docRef.current) observer.observe(docRef.current, { childList: true, subtree: true, characterData: true });
    return () => observer.disconnect();
  }, [step, meetingData.aiSummary]);

  // Numbers of the document checked against the transcript (marked amber, with a fix when likely)
  const numbers = useNumberCheck(docRef, meeting.id, step === 'document', (blockText, from, to) => {
    const ref = meetingData.blockNoteSummaryRef.current as any;
    return !!ref?.replaceInBlock?.(blockText, from, to);
  });

  // The player under the document: the same timeline as under the transcript
  const docTimelineLines: TimelineLine[] = useMemo(
    () =>
      meetingData.transcripts
        .filter((t: Transcript) => typeof t.audio_start_time === 'number')
        .map((t: Transcript) => ({ id: t.id, start: t.audio_start_time ?? 0, end: t.audio_end_time ?? t.audio_start_time ?? 0, speaker: t.speaker ?? '—' })),
    [meetingData.transcripts],
  );
  const docColors = useMemo(() => speakerColorMap(meetingData.transcripts.map((t: Transcript) => t.speaker)), [meetingData.transcripts]);
  const docColorOf = (s?: string) => (s ? docColors.get(s) : undefined) ?? 'var(--muted-text)';

  // Clicking a time mark (00:41:02 or 41:02) in the protocol highlights that moment in the transcript
  const handleProtocolClick = (event: React.MouseEvent) => {
    // A transcript line handles its own time mark (exact start, not whole seconds)
    if ((event.target as HTMLElement | null)?.closest?.('.transcript-segment')) return;
    const range = (document as any).caretRangeFromPoint?.(event.clientX, event.clientY) as Range | null;
    const node = range?.startContainer;
    if (!node || node.nodeType !== Node.TEXT_NODE) return;
    const text = node.textContent ?? '';
    const offset = range!.startOffset;
    // ⟨?⟩: a point the app could not find in the speaker's words
    const unknown = text.indexOf(NO_SOURCE);
    if (unknown >= 0 && offset >= unknown && offset <= unknown + NO_SOURCE.length) {
      const block = (node.parentElement?.closest('.bn-block-outer') ?? null) as HTMLElement | null;
      let prev = block?.previousElementSibling as HTMLElement | null;
      let seconds = -1;
      // The time of the «ВИСТУПИЛИ» block it belongs to
      for (let k = 0; prev && k < 30 && seconds < 0; k++, prev = prev.previousElementSibling as HTMLElement | null) {
        const m = (prev.textContent ?? '').match(/ВИСТУПИЛИ:.*?\[(\d{1,2}):(\d{2})/);
        if (m) seconds = Number(m[1]) * 60 + Number(m[2]);
      }
      void getProtocolMarkdown().then((markdown) => setSource({ seconds: Math.max(0, seconds), markdown, unknown: (block?.textContent ?? '').trim() }));
      return;
    }
    const pattern = /\b(?:(\d{1,2}):)?(\d{1,2}):(\d{2})\b/g;
    let match: RegExpExecArray | null;
    while ((match = pattern.exec(text))) {
      if (offset < match.index || offset > match.index + match[0].length) continue;
      const [, h, m, sec] = match;
      const seconds = (h ? Number(h) * 3600 : 0) + Number(m) * 60 + Number(sec);
      // The source of this point (who said it, the words, what was left out),
      // and the recording plays from there
      void getProtocolMarkdown().then((markdown) => setSource({ seconds, markdown }));
      void seekTo(Math.max(0, seconds - 0.3), true);
      return;
    }
  };

  // The meeting's recording, played from the docked player under «Стенограма».
  // A time mark in the transcript plays from there at once; one in the
  // document shows the line and moves the player there. The position is set
  // once the file's length is known (set earlier, the player ignores it).
  const audioRef = useRef<HTMLAudioElement | null>(null);
  const [audioPlaying, setAudioPlaying] = useState(false);
  const [audioCurrent, setAudioCurrent] = useState(0);
  const [audioDuration, setAudioDuration] = useState(0);
  // The speed is remembered between meetings
  const [audioRate, setAudioRate] = useState(() => {
    try {
      const saved = Number(window.localStorage.getItem('protokol.playbackRate'));
      return saved >= 0.5 && saved <= 3 ? saved : 1;
    } catch {
      return 1;
    }
  });
  // «Слухати рядок»: playing stops (or starts over) at the line's end
  const rangeRef = useRef<{ start: number; end: number; loop: boolean } | null>(null);
  // The recording's two clocks: the page keeps the sound's, the audio element the file's
  const timePoints = useRef<TimePoints>([]);
  const [rangeLoop, setRangeLoop] = useState(false);
  const atRangeEnd = (audio: HTMLAudioElement) => {
    const r = rangeRef.current;
    if (!r || toSound(timePoints.current, audio.currentTime) < r.end) return;
    if (r.loop) {
      audio.currentTime = toPlayer(timePoints.current, r.start);
    } else {
      audio.pause();
      rangeRef.current = null;
    }
  };
  // While playing the position is read every frame (the current word is
  // highlighted; the audio's own updates come only 4 times a second)
  // The audio's own position moves in steps (a few times a second in
  // WebKit): between them it is carried on by the clock. What follows the
  // sound reads this every frame; the page itself updates 4 times a second.
  const clock = useRef({ t: 0, at: 0, from: 0 });
  useEffect(() => {
    setPlayheadSource(() => {
      const audio = audioRef.current;
      if (!audio) return null;
      const now = performance.now();
      if (audio.paused) {
        clock.current = { t: audio.currentTime, at: now, from: audio.currentTime };
        return toSound(timePoints.current, audio.currentTime);
      }
      if (audio.currentTime !== clock.current.t) clock.current = { ...clock.current, t: audio.currentTime, at: now };
      // Right after a jump the sound is still starting and its position comes unevenly
      const settled = clock.current.t - clock.current.from >= 0.6;
      const ahead = Math.min(settled ? 0.6 : 0.06, ((now - clock.current.at) / 1000) * audio.playbackRate);
      return toSound(timePoints.current, clock.current.t + ahead);
    });
    return () => setPlayheadSource(null);
  }, []);
  useEffect(() => {
    setPlayheadRunning(audioPlaying);
    if (!audioPlaying) return;
    // A listened range stops at its end (the shown time comes from «timeupdate», once a second)
    return followPlayhead(() => {
      const audio = audioRef.current;
      if (audio) atRangeEnd(audio);
    });
  }, [audioPlaying]);
  // One load at a time: two quick requests must not make two players
  const audioLoading = useRef<Promise<HTMLAudioElement | null> | null>(null);
  const ensureAudio = (): Promise<HTMLAudioElement | null> => {
    if (audioRef.current) return Promise.resolve(audioRef.current);
    audioLoading.current ??= loadAudio().finally(() => {
      audioLoading.current = null;
    });
    return audioLoading.current;
  };
  const loadAudio = async (): Promise<HTMLAudioElement | null> => {
    if (!audioRef.current) {
      const path = await invoke<string | null>('api_meeting_audio_path', { meetingId: meeting.id });
      if (!path) {
        toast.info('Звук цього запису не знайдено');
        return null;
      }
      timePoints.current = await invoke<TimePoints>('api_player_time_points', { meetingId: meeting.id }).catch(() => []);
      const audio = new Audio(convertFileSrc(path));
      audio.preload = 'auto';
      // Slower or faster without changing the voice
      (audio as HTMLAudioElement & { preservesPitch?: boolean; webkitPreservesPitch?: boolean }).preservesPitch = true;
      (audio as HTMLAudioElement & { webkitPreservesPitch?: boolean }).webkitPreservesPitch = true;
      audio.addEventListener('play', () => setAudioPlaying(true));
      audio.addEventListener('pause', () => setAudioPlaying(false));
      audio.addEventListener('ended', () => setAudioPlaying(false));
      // While playing the page is drawn again once a second (the clock and the
      // lit words follow the sound on their own); paused, at once
      let shownSecond = -1;
      audio.addEventListener('timeupdate', () => {
        const s = Math.floor(audio.currentTime);
        if (!audio.paused && s === shownSecond) return;
        shownSecond = s;
        setAudioCurrent(toSound(timePoints.current, audio.currentTime));
      });
      audio.addEventListener('loadedmetadata', () => setAudioDuration(toSound(timePoints.current, audio.duration || 0)));
      audioRef.current = audio;
    }
    const audio = audioRef.current;
    if (audio.readyState < 1) {
      await new Promise<void>((resolve, reject) => {
        audio.addEventListener('loadedmetadata', () => resolve(), { once: true });
        audio.addEventListener('error', () => reject(new Error('audio')), { once: true });
        audio.load();
      });
    }
    audio.playbackRate = audioRate;
    return audio;
  };
  const seekTo = async (seconds: number, play: boolean, keepRange = false) => {
    if (!keepRange) {
      rangeRef.current = null;
      setRangeLoop(false);
    }
    try {
      const audio = await ensureAudio();
      if (!audio) return;
      const place = toPlayer(timePoints.current, Math.max(0, seconds));
      audio.currentTime = Math.min(place, audio.duration || place);
      clock.current = { t: audio.currentTime, at: performance.now(), from: audio.currentTime };
      setAudioCurrent(toSound(timePoints.current, audio.currentTime));
      if (play) await audio.play();
    } catch (error) {
      console.error('Playback failed', error);
      toast.error('Не вдалося відтворити запис');
    }
  };
  const playFrom = (seconds: number) => seekTo(Math.max(0, seconds - 0.3), true);
  const togglePlay = async () => {
    const audio = audioRef.current;
    if (audio && !audio.paused) audio.pause();
    else await seekTo(audioCurrent, true, true);
  };
  const skip = (delta: number) => void seekTo(audioCurrent + delta, audioPlaying, true);
  const changeRate = (rate: number) => {
    setAudioRate(rate);
    try {
      window.localStorage.setItem('protokol.playbackRate', String(rate));
    } catch {
      // not remembered: fine
    }
    if (audioRef.current) audioRef.current.playbackRate = rate;
  };
  // A transcript time mark moves the player there; sound starts only from
  // the player itself (▶, ⌥Space, the ⏯ key)
  useEffect(() => {
    const onTime = (e: Event) => {
      const seconds = (e as CustomEvent<{ seconds: number }>).detail?.seconds;
      if (typeof seconds === 'number') void seekTo(Math.max(0, seconds - 0.3), audioPlaying, true);
    };
    window.addEventListener(TIME_CLICKED_EVENT, onTime);
    return () => window.removeEventListener(TIME_CLICKED_EVENT, onTime);
  });
  // Вигляд → Стенограма ⌘1 / Документ ⌘2 in the menu bar
  useEffect(() => {
    const onMenu = (e: Event) => {
      const action = (e as CustomEvent<string>).detail;
      if (action === 'tab-transcript') setActiveTab('transcript');
      else if (action === 'tab-document') setActiveTab('summary');
    };
    window.addEventListener('protokol:menu', onMenu);
    return () => window.removeEventListener('protokol:menu', onMenu);
  });
  // Space plays or pauses, ←/→ move 5 seconds (not while typing)
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const t = e.target as HTMLElement | null;
      // ⌘⇧. faster, ⌘⇧, slower
      if (e.metaKey && e.shiftKey && (e.code === 'Period' || e.code === 'Comma')) {
        const rates = [0.75, 1, 1.5, 2];
        const i = Math.max(0, rates.indexOf(audioRate));
        const next = rates[Math.max(0, Math.min(rates.length - 1, i + (e.code === 'Period' ? 1 : -1)))];
        e.preventDefault();
        changeRate(next);
        toast(`Швидкість ${String(next).replace('.', ',')}×`);
        return;
      }
      // ⌥Space plays or pauses anywhere, even while typing in the transcript
      if (e.altKey && e.code === 'Space' && !e.metaKey && !e.ctrlKey) {
        e.preventDefault();
        void togglePlay();
        return;
      }
      if (t && (t.closest('input, textarea, select, button, a, [contenteditable="true"], [role="dialog"], .bn-editor') || t.isContentEditable)) return;
      if (e.metaKey || e.ctrlKey || e.altKey) return;
      if (e.code === 'Space') {
        e.preventDefault();
        void togglePlay();
      } else if (e.key === 'ArrowLeft' && audioRef.current) {
        e.preventDefault();
        skip(-5);
      } else if (e.key === 'ArrowRight' && audioRef.current) {
        e.preventDefault();
        skip(5);
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  });
  useEffect(() => () => audioRef.current?.pause(), []);
  // The keyboard's ⏯ key
  useEffect(() => {
    const ms = (navigator as Navigator & { mediaSession?: MediaSession }).mediaSession;
    if (!ms) return;
    try {
      ms.setActionHandler('play', () => void togglePlay());
      ms.setActionHandler('pause', () => audioRef.current?.pause());
    } catch {
      // not supported here
    }
    return () => {
      try {
        ms.setActionHandler('play', null);
        ms.setActionHandler('pause', null);
      } catch {
        // nothing to undo
      }
    };
  });
  // «Перевірте» plays a word straight away
  useEffect(() => {
    const onPlay = (e: Event) => {
      const detail = (e as CustomEvent<{ seconds: number; exact?: boolean }>).detail;
      if (typeof detail?.seconds !== 'number') return;
      if (detail.exact) void seekTo(Math.max(0, detail.seconds - 0.05), true);
      else void playFrom(detail.seconds);
    };
    // A line on its own (and over again with «по колу»)
    const onRange = (e: Event) => {
      const r = (e as CustomEvent<{ start: number; end: number; loop?: boolean }>).detail;
      if (typeof r?.start !== 'number' || typeof r?.end !== 'number') return;
      rangeRef.current = { start: r.start, end: r.end, loop: !!r.loop };
      setRangeLoop(!!r.loop);
      void seekTo(Math.max(0, r.start - 0.05), true, true);
    };
    window.addEventListener(PLAY_FROM_EVENT, onPlay);
    window.addEventListener(PLAY_RANGE_EVENT, onRange);
    return () => {
      window.removeEventListener(PLAY_FROM_EVENT, onPlay);
      window.removeEventListener(PLAY_RANGE_EVENT, onRange);
    };
  });

  // The transcript list scrolls itself to the exact line (see VirtualizedTranscriptView)
  const highlightTranscriptAt = (seconds: number) => {
    setActiveTab('transcript');
    window.setTimeout(() => window.dispatchEvent(new CustomEvent(JUMP_TO_TIME_EVENT, { detail: { seconds, seek: true } })), 60);
  };

  // Right after the document is made: show it (not when a meeting is opened)
  const lastSummaryStatus = useRef(summaryGeneration.summaryStatus);
  useEffect(() => {
    const was = lastSummaryStatus.current;
    lastSummaryStatus.current = summaryGeneration.summaryStatus;
    if (
      summaryGeneration.summaryStatus === 'completed'
      && was !== 'completed' && was !== 'idle'
      && !autoSwitchedSummaryMeetingIdsRef.current.has(meeting.id)
      && !manuallySelectedTabMeetingIdsRef.current.has(meeting.id)
    ) {
      autoSwitchedSummaryMeetingIdsRef.current.add(meeting.id);
      setActiveTab('summary');
    }
  }, [meeting.id, meetingData.aiSummary, summaryGeneration.summaryStatus]);

  // Setting «Створювати документ після розпізнавання спікерів» (off by default)
  useEffect(() => {
    if (!speakerRecognition.finishedAt || meetingData.aiSummary || summaryGeneration.summaryStatus !== 'idle') return;
    invoke<boolean>('get_auto_document')
      .then((enabled) => {
        if (enabled) void summaryGeneration.handleGenerateSummary('', undefined, 'auto-after-speakers');
      })
      .catch(() => {});
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [speakerRecognition.finishedAt]);

  // Auto-generate only after the model configuration has settled.
  useEffect(() => {
    if (
      !shouldAutoGenerate
      || speakerRecognition.running // wait for the speakers: the document is better with them
      || summaryGeneration.summaryStatus !== 'idle'
      || isModelConfigLoading
      || meetingData.transcripts.length === 0
      || autoGenerationStartedMeetingIdRef.current === meeting.id
    ) {
      return;
    }

    autoGenerationStartedMeetingIdRef.current = meeting.id;
    console.log(`🤖 Auto-generating summary with ${modelConfig.provider}/${modelConfig.model}...`);
    onAutoGenerateComplete?.();
    void summaryGeneration.handleGenerateSummary('', undefined, 'auto-after-recording');
  }, [
    shouldAutoGenerate,
    speakerRecognition.running,
    meeting.id,
    meetingData.transcripts.length,
    isModelConfigLoading,
    modelConfig.provider,
    modelConfig.model,
    summaryGeneration.handleGenerateSummary,
    summaryGeneration.summaryStatus,
    onAutoGenerateComplete,
  ]);

  // Under the document's toolbar, so the toolbar keeps its place on both pages
  const docBanner = (
    <>
      {hasProtocol && transcriptChanged && !staleDismissed && !isGenerating && (
        <div className="doc-banner doc-banner-info" role="status">
          <span className="flex-1 min-w-0">Стенограму виправлено після створення документа.</span>
          <button type="button" className="doc-banner-action" onClick={() => handleRegenerate('stale-banner')}>
            Оновити документ
          </button>
          <button type="button" className="doc-banner-close" aria-label="Закрити" title="Закрити" onClick={() => setStaleDismissed(true)}>
            <svg width="11" height="11" viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" aria-hidden="true">
              <path d="M2.5 2.5l7 7M9.5 2.5l-7 7" />
            </svg>
          </button>
        </div>
      )}
      {reviewProgress && reviewProgress.total > 0 && (
            reviewProgress.reviewed >= reviewProgress.total ? (
              (noSource > 0 || numbers.count > 0) && (
                <div className="doc-banner doc-banner-warn">
                  {noSource > 0 && `Пунктів без джерела: ${noSource} — перевірте пункти, підсвічені жовтим.`}
                  {numbers.count > 0 && ` Чисел, яких немає в стенограмі: ${numbers.count} — вони підсвічені жовтим.`}
                </div>
              )
            ) : (
              <div className="doc-banner doc-banner-warn">
                Стенограму перевірено не повністю: {reviewProgress.reviewed} з {reviewProgress.total} рядків. Імена й слова в документі можуть бути неточні.
                {noSource > 0 && <> Пунктів без джерела: {noSource}.</>}
                {numbers.count > 0 && <> Чисел, яких немає в стенограмі: {numbers.count}.</>}
                <button type="button" className="ml-2 font-semibold underline" onClick={() => setStep('transcript')}>
                  Перевірити →
                </button>
                {!isRecording && speakerRecognition.progress === null && (
                  <button type="button" className="ml-3 font-semibold underline" onClick={() => window.dispatchEvent(new Event(MARK_ALL_REVIEWED_EVENT))}>
                    Позначити все перевіреним
                  </button>
                )}
              </div>
            )
          )}
    </>
  );

  return (
    <motion.div
      initial={{ opacity: 0, y: 20 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ duration: 0.3, ease: 'easeOut' }}
      className="flex flex-col h-screen min-w-0 gap-3 p-3"
    >
      <ProtocolTopBar
        title={meetingData.meetingTitle || 'Без назви'}
        createdAt={meeting.created_at}
        durationSeconds={durationSeconds}
        status={status}
        hasProtocol={hasProtocol}
        isGenerating={isGenerating}
        isExporting={isExporting}
        isSavingStatus={isSavingStatus}
        onRegenerate={() => handleRegenerate('top-bar')}
        onToggleApproved={handleToggleApproved}
        onExportDocx={handleExportDocx}
        onExportPdf={handleExportPdf}
        step={step}
        onStep={setStep}
        people={peopleCount}
      />
      <LowPowerNotice className="card px-4 py-3" />
      <div className="flex flex-1 min-w-0 min-h-0 overflow-hidden">
        <div className={step === 'transcript' ? 'flex flex-1 min-w-0 min-h-0' : 'hidden'}>
          {!isRecording && speakerRecognition.progress === null ? (
            <TranscriptWorkspace
              meetingId={meeting.id}
              transcripts={meetingData.transcripts}
              hasMore={hasMore}
              onLoadMore={onLoadMore}
              onRefetchTranscripts={onRefetchTranscripts}
              onCopyTranscript={copyOperations.handleCopyTranscript}
              onOpenMeetingFolder={meetingOperations.handleOpenMeetingFolder}
              meetingFolderPath={meeting.folder_path}
              onProgress={setReviewProgress}
              onJumpTo={(seconds) => {
                highlightTranscriptAt(seconds);
                void seekTo(Math.max(0, seconds - 0.3), false);
              }}
              player={{
                playing: audioPlaying,
                current: audioCurrent,
                duration: audioDuration || durationSeconds || 0,
                rate: audioRate,
                onToggle: () => void togglePlay(),
                onSeek: (sec: number) => void seekTo(sec, audioPlaying),
                onSkip: skip,
                onRate: changeRate,
              }}
            />
          ) : (
            <TranscriptPanel
              transcripts={meetingData.transcripts}
              onCopyTranscript={copyOperations.handleCopyTranscript}
              onOpenMeetingFolder={meetingOperations.handleOpenMeetingFolder}
              isRecording={isRecording}
              disableAutoScroll={true}
              usePagination={true}
              segments={segments}
              hasMore={hasMore}
              isLoadingMore={isLoadingMore}
              totalCount={totalCount}
              loadedCount={loadedCount}
              onLoadMore={onLoadMore}
              meetingId={meeting.id}
              meetingFolderPath={meeting.folder_path}
              onRefetchTranscripts={onRefetchTranscripts}
              speakerProgress={speakerRecognition.progress}
              speakerMinutesLeft={speakerRecognition.minutesLeft}
              onStopSpeakers={() => void speakerRecognition.stop()}
              onProgress={setReviewProgress}
              onJumpTo={(seconds) => {
                highlightTranscriptAt(seconds);
                void seekTo(Math.max(0, seconds - 0.3), false);
              }}
              player={{
                playing: audioPlaying,
                current: audioCurrent,
                duration: audioDuration || durationSeconds || 0,
                rate: audioRate,
                looping: rangeLoop,
                onStopLoop: () => {
                  rangeRef.current = null;
                  setRangeLoop(false);
                },
                onToggle: () => void togglePlay(),
                onSeek: (sec: number) => void seekTo(sec, audioPlaying),
                onSkip: skip,
                onRate: changeRate,
              }}
            />
          )}
        </div>
        <div className={step === 'document' ? 'flex flex-1 min-w-0 min-h-0 flex-col gap-3' : 'hidden'}>
          <div className="flex flex-1 min-h-0 min-w-0 gap-3">
            {numbers.popover}
            <div className="flex flex-1 min-w-0 min-h-0" ref={docRef}>
            <SummaryPanel
              meeting={meeting}
              meetingTitle={meetingData.meetingTitle}
              summaryRef={meetingData.blockNoteSummaryRef}
              isSaving={meetingData.isSaving}
              isSummaryDirty={meetingData.isSummaryDirty}
              onSaveAll={meetingData.saveAllChanges}
              onCopySummary={copyOperations.handleCopySummary}
              aiSummary={meetingData.aiSummary}
              summaryStatus={summaryGeneration.summaryStatus}
              transcripts={meetingData.transcripts}
              modelConfig={modelConfig}
              setModelConfig={setModelConfig}
              onSaveModelConfig={handleSaveModelConfig}
              onGenerateSummary={(prompt) => summaryGeneration.handleGenerateSummary(prompt, undefined, 'empty-panel')}
              onStopGeneration={summaryGeneration.handleStopGeneration}
              customPrompt=""
              onSaveSummary={meetingData.handleSaveSummary}
              onSummaryChange={meetingData.handleSummaryChange}
              onDirtyChange={meetingData.setIsSummaryDirty}
              summaryError={summaryGeneration.summaryError}
              onRegenerateSummary={() => summaryGeneration.handleRegenerateSummary(undefined, 'legacy-view')}
              getSummaryStatusMessage={summaryGeneration.getSummaryStatusMessage}
              isModelConfigLoading={isModelConfigLoading}
              onOpenModelSettings={handleRegisterModalOpen}
              onTextClick={handleProtocolClick}
              banner={docBanner}
              docKey={docKey}
              templateSlot={
                <TemplateMenu
                  templates={templates.templates}
                  variants={templates.variants}
                  shown={templates.shownTemplate}
                  fallback={templates.selectedTemplate}
                  minutes={durationSeconds ? durationSeconds / 60 : null}
                  disabled={isGenerating}
                  onShow={(id) => void showDocument(id)}
                  onCreate={(id) => void createWithTemplate(id)}
                />
              }
              emptyState={
                meetingData.transcripts.length > 0 ? (
                  <TemplatePicker
                    templates={templates.templates}
                    selected={templates.selectedTemplate}
                    onSelect={templates.setSelectedTemplate}
                    onCreate={(how) => void summaryGeneration.handleGenerateSummary('', templates.selectedTemplate, how)}
                    minutes={durationSeconds ? durationSeconds / 60 : null}
                    unchecked={reviewProgress ? Math.max(0, reviewProgress.total - reviewProgress.reviewed) : 0}
                    onCheck={() => setStep('transcript')}
                    error={summaryGeneration.summaryError}
                    hasModel={modelConfig.provider !== null && modelConfig.model !== null}
                  />
                ) : undefined
              }
              toolbarRight={
                <DocumentActions
                  hasProtocol={hasProtocol}
                  isGenerating={isGenerating}
                  isExporting={isExporting}
                  onRegenerate={() => handleRegenerate('document-actions')}
                  onExportDocx={handleExportDocx}
                  onExportPdf={handleExportPdf}
                  onCopy={() => void copyOperations.handleCopySummary()}
                />
              }
              footer={
                <Timeline
                  meetingId={meeting.id}
                  lines={docTimelineLines}
                  colorOf={docColorOf}
                  duration={audioDuration || durationSeconds || 0}
                  current={audioCurrent}
                  playing={audioPlaying}
                  rate={audioRate}
                  band={null}
                  onToggle={() => void togglePlay()}
                  onSeek={(sec: number) => void seekTo(sec, audioPlaying)}
                  onSkip={skip}
                  onRate={changeRate}
                  onChooseRange={() => {}}
                  onMoveBorder={() => {}}
                  onScrub={(sec: number) => void seekTo(sec, false)}
                />
              }
            />
            </div>
            {source && (
              <DocSourcePanel
                meetingId={meeting.id}
                seconds={source.seconds}
                unknown={source.unknown}
                markdown={source.markdown}
                transcripts={meetingData.transcripts}
                onListen={(start, end) => window.dispatchEvent(new CustomEvent(PLAY_RANGE_EVENT, { detail: { start, end } }))}
                onOpenInTranscript={(seconds) => {
                  setSource(null);
                  highlightTranscriptAt(seconds);
                  void seekTo(Math.max(0, seconds - 0.3), false);
                }}
                onAdd={(quote) => {
                  const ref = meetingData.blockNoteSummaryRef.current as any;
                  const ok = ref?.insertBulletAfter?.('Не враховано', quote) && ref?.removeFromBlock?.(`«${quote}»`);
                  if (ok) {
                    toast.success('Додано в документ', { description: 'Не забудьте зберегти документ.' });
                    void getProtocolMarkdown().then((markdown) => setSource((s) => (s ? { ...s, markdown } : s)));
                  } else toast.info('Не вдалося знайти це місце в документі — додайте вручну');
                }}
                onDismiss={(quote) => {
                  const ref = meetingData.blockNoteSummaryRef.current as any;
                  if (ref?.removeFromBlock?.(`«${quote}»`)) void getProtocolMarkdown().then((markdown) => setSource((s) => (s ? { ...s, markdown } : s)));
                }}
                onClose={() => setSource(null)}
              />
            )}
          </div>
        </div>
      </div>
    </motion.div>
  );
}
