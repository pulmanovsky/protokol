'use client';

import { ReactNode, useEffect, useMemo, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Lock, Video } from 'lucide-react';
import { openMeet, participantsText, useMeetLive } from '@/lib/meet';
import { toast } from '@/lib/notice';
import { VirtualizedTranscriptView } from '@/components/VirtualizedTranscriptView';
import { useTranscripts } from '@/contexts/TranscriptContext';
import { useRecordingState } from '@/contexts/RecordingStateContext';
import { useConfig } from '@/contexts/ConfigContext';
import { LowPowerNotice } from '@/components/LowPowerNotice';
import { LevelMeter } from '@/components/audio/LevelMeter';
import { MuteButton } from '@/components/audio/MuteButton';
import { useLiveAudioLevels } from '@/hooks/useLiveAudioLevels';
import { useAudioSources, useMicMute } from '@/lib/audioSources';

function formatClock(totalSeconds: number): string {
  const s = Math.max(0, Math.floor(totalSeconds));
  const hh = String(Math.floor(s / 3600)).padStart(2, '0');
  const mm = String(Math.floor((s % 3600) / 60)).padStart(2, '0');
  const ss = String(s % 60).padStart(2, '0');
  return `${hh}:${mm}:${ss}`;
}

/** Active recording time, re-synced from the backend and ticking locally in between */
function useRecordingClock(): number {
  const { isRecording, isPaused, activeDuration } = useRecordingState();
  const [seconds, setSeconds] = useState(0);

  useEffect(() => {
    if (activeDuration != null) setSeconds(activeDuration);
  }, [activeDuration]);

  useEffect(() => {
    if (!isRecording) {
      setSeconds(0);
      return;
    }
    if (isPaused) return;
    const timer = setInterval(() => setSeconds((v) => v + 1), 1000);
    return () => clearInterval(timer);
  }, [isRecording, isPaused]);

  return seconds;
}

interface RecordingViewProps {
  /** Pause / finish buttons (RecordingControls, toolbar variant) */
  controls: ReactNode;
  isProcessingStop: boolean;
  isStopping: boolean;
}

export function RecordingView({ controls, isProcessingStop, isStopping }: RecordingViewProps) {
  const { transcripts, meetingTitle } = useTranscripts();
  const { isRecording, isPaused } = useRecordingState();
  const { selectedDevices } = useConfig();
  const seconds = useRecordingClock();
  const [glossaryCount, setGlossaryCount] = useState<number | null>(null);

  useEffect(() => {
    invoke<string[]>('api_get_glossary')
      .then((terms) => setGlossaryCount(terms?.length ?? 0))
      .catch((error) => console.error('Failed to load glossary', error));
  }, []);

  const segments = useMemo(
    () =>
      transcripts.map((t) => ({
        id: t.id,
        timestamp: t.audio_start_time ?? 0,
        endTime: t.audio_end_time,
        text: t.text,
        confidence: t.confidence,
      })),
    [transcripts],
  );

  const live = isRecording && !isPaused;
  const meet = useMeetLive();
  const [audioSources] = useAudioSources();
  const [micMuted, setMicMuted] = useMicMute();
  const levels = useLiveAudioLevels(isRecording);
  const toggleMute = () => setMicMuted(!micMuted);
  const micName = selectedDevices?.micDevice?.replace(/\s*\(input\)$/i, '') || 'за замовчуванням';
  // A source that is on but silent for this long gets a hint
  const SILENCE_HINT_S = 10;

  const sources = [
    {
      key: 'mic',
      kind: 'Мікрофон',
      label: audioSources.micEnabled ? micName : 'вимкнено',
      enabled: audioSources.micEnabled,
      level: levels.mic,
      dimmed: !audioSources.micEnabled || micMuted,
      hint: !audioSources.micEnabled
        ? null
        : micMuted
        ? 'Мікрофон вимкнено: ваш голос не записується'
        : live && levels.mic.silentFor > SILENCE_HINT_S
        ? 'Тиша з мікрофона — перевірте пристрій'
        : null,
    },
    {
      key: 'system',
      kind: 'Звук дзвінка',
      label: audioSources.systemEnabled ? 'весь звук Mac' : 'вимкнено',
      enabled: audioSources.systemEnabled,
      level: levels.system,
      dimmed: !audioSources.systemEnabled,
      hint:
        audioSources.systemEnabled && live && levels.system.silentFor > SILENCE_HINT_S
          ? 'Звуку дзвінка немає. Якщо співрозмовники говорять, перевірте дозвіл на запис звуку'
          : null,
    },
  ];

  return (
    <div className="flex-1 min-w-0 h-full flex flex-col gap-4 p-6">
      <header data-tauri-drag-region data-notice-anchor className="glass-bar flex items-center justify-between gap-4 flex-wrap px-5 py-3 min-h-[64px]">
        <div className="flex items-center gap-4 min-w-0">
          <span
            className={`inline-flex items-center gap-2 h-8 px-3.5 rounded-full text-sm font-bold ${
              live ? 'bg-[rgba(229,53,43,0.14)] text-[#B3261E] dark:text-[#FF8A80]' : 'bg-[var(--selected)] text-[color:var(--muted-text)]'
            }`}
          >
            <span className={`w-2.5 h-2.5 rounded-full ${live ? 'record-dot shadow-[0_0_0_4px_rgba(229,53,43,0.2)]' : 'bg-gray-400'}`} />
            {isPaused ? 'Пауза' : 'Запис'}
          </span>
          <span data-tauri-drag-region className="text-2xl font-semibold tabular-nums tracking-[-0.3px] text-gray-900">{formatClock(seconds)}</span>
          <span data-tauri-drag-region className="text-lg font-bold text-gray-900 truncate">{meetingTitle || 'Новий запис'}</span>
        </div>
        <div className="flex items-center gap-2">
          {audioSources.micEnabled && (
            <MuteButton muted={micMuted} onToggle={toggleMute} disabled={!isRecording} withShortcut />
          )}
          {controls}
        </div>
      </header>

      <div className="flex-1 min-h-0 grid gap-4 grid-cols-1 md:grid-cols-[minmax(0,1fr)_minmax(240px,300px)]">
        <section aria-label="Стенограма" className="card flex flex-col min-h-0 overflow-hidden">
          <div className="flex items-baseline justify-between gap-3 px-7 pt-6 pb-2">
            <h2 className="text-[17px] font-bold text-gray-900">Стенограма наживо</h2>
            <span className="text-[13px] text-[color:var(--muted-text)]">дослівно, як сказано</span>
          </div>
          {/* Protokol's Google Meet window: who it sees speaking, or a way to open it
              during the recording */}
          <div className="mx-7 mt-1 mb-1 flex items-center gap-2.5 rounded-[10px] bg-[var(--system-fill)] px-3 py-2 min-w-0">
            <Video className="w-[18px] h-[18px] flex-none text-gray-900" aria-hidden="true" />
            <span className="flex-1 min-w-0 truncate text-[13px] text-gray-900">
              Google Meet
              {!meet.open ? (
                ' · вікно закрито'
              ) : meet.inCall ? (
                <>
                  {` · ${participantsText(meet.participants)}`}
                  {meet.meetMic === true && ' · мікрофон у Meet вимкнено'}
                  {meet.speaking.length > 0 && (
                    <>
                      {' · говорить: '}
                      <b>{meet.speaking.join(', ')}</b>
                    </>
                  )}
                </>
              ) : (
                ' · чекає на нараду'
              )}
            </span>
            <button
              type="button"
              onClick={() => (meet.open ? invoke('meet_show') : openMeet()).catch((e) => toast.error(`${e}`))}
              className="secondary-button h-7 px-3 text-[12.5px] font-semibold flex-none"
            >
              {meet.open ? 'Показати нараду' : 'Відкрити Google Meet'}
            </button>
          </div>
          <div className="flex-1 min-h-0 px-3 pb-3">
            <VirtualizedTranscriptView
              segments={segments}
              isRecording={isRecording}
              isPaused={isPaused}
              isProcessing={isProcessingStop}
              isStopping={isStopping}
              enableStreaming={isRecording}
              showConfidence={false}
              showStatusBar={false}
            />
          </div>
        </section>

        <aside className="flex flex-col gap-4 min-w-0">
          <section aria-label="Звук" className="card p-5 flex flex-col gap-3.5">
            <h2 className="text-[15px] font-bold text-gray-900">Звук</h2>
            {sources.map((source) => (
              <div key={source.key} className="flex flex-col gap-1.5">
                <span
                  className={`text-[13px] truncate ${source.enabled ? 'text-gray-900' : 'text-[color:var(--muted-text)]'}`}
                  title={source.label}
                >
                  {source.kind} · {source.label}
                </span>
                <LevelMeter
                  rms={source.level.rms}
                  peakHold={source.level.peakHold}
                  dimmed={source.dimmed}
                  label={`Рівень: ${source.kind}`}
                />
                {source.hint && (
                  <span className="text-[12px] leading-[1.4] text-[color:var(--muted-text)]">{source.hint}</span>
                )}
              </div>
            ))}
            {audioSources.micEnabled && (
              <MuteButton muted={micMuted} onToggle={toggleMute} disabled={!isRecording} compact />
            )}
          </section>

          <section aria-label="Після запису" className="card p-5 flex flex-col gap-2.5">
            <h2 className="text-[15px] font-bold text-gray-900">Після запису</h2>
            <span className="text-sm text-gray-900">Формат: стенограма по доповідачах</span>
            <span className="text-sm text-gray-900">
              Словник: {glossaryCount == null ? '—' : `${glossaryCount} ${termsWord(glossaryCount)}`}
            </span>
            <span className="flex gap-2 items-start text-[13px] leading-[1.45] text-[color:var(--muted-text)]">
              <Lock className="w-[15px] h-[15px] flex-none mt-[1px]" aria-hidden="true" />
              Підсумок готується на цьому Mac. Звук і текст нікуди не надсилаються.
            </span>
            <LowPowerNotice />
          </section>
        </aside>
      </div>
    </div>
  );
}

function termsWord(n: number): string {
  const mod10 = n % 10;
  const mod100 = n % 100;
  if (mod10 === 1 && mod100 !== 11) return 'термін';
  if (mod10 >= 2 && mod10 <= 4 && (mod100 < 12 || mod100 > 14)) return 'терміни';
  return 'термінів';
}
