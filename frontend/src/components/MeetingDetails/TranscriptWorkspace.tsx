'use client';

import { memo, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { toast } from '@/lib/notice';
import { CheckCheck, ChevronLeft, ChevronRight, Copy, Search, Volume2 } from 'lucide-react';
import { copySpeakerText } from '@/lib/copySpeaker';
import { snippetRange } from '@/lib/snippet';
import { Transcript } from '@/types';
import { TranscriptButtonGroup } from './TranscriptButtonGroup';
import { SpeakerPicker, SpeakerScope } from './SpeakerPicker';
import { Timeline, TimelineLine } from './Timeline';
import { speakerColorMap, isUnnamed } from '@/lib/speakerColors';
import { JUMP_TO_TIME_EVENT, MARK_ALL_REVIEWED_EVENT, LINE_CHANGED_EVENT, mmss, playRange } from '@/lib/timeline';
import { followPlayhead, readPlayhead } from '@/lib/playhead';
import { perfAdd, perfCount, perfOn } from '@/lib/perf';
import { WORD_HIGHLIGHT_EVENT, effectiveRows } from '@/lib/lineTimes';
import { popupNativeMenu } from '@/lib/nativeMenu';
import { clickPlaysWord } from '@/lib/clickWord';
import { plural } from '@/lib/plural';
import { useRouter } from 'next/navigation';

/** A word of a line with its moment in the recording */
/** While playing, a clicked word is heard from a little before it */
const CLICK_LEAD_S = 0.7;
/** The strip under its word, or above it when it would run off the window */
function stripTop(word: DOMRect, options: number): number {
  const height = 212 + options * 38;
  const below = word.bottom + 8;
  return below + height <= window.innerHeight - 8 ? below : Math.max(56, word.top - height - 8);
}

/** Short function words: a doubt on them is noise unless recognition was really unsure */
const FUNCTION_WORDS = new Set(['й', 'і', 'у', 'в', 'з', 'а', 'та', 'чи', 'же', 'ж', 'би', 'б', 'не', 'ні', 'що', 'як', 'це']);
const VERY_UNSURE = 0.2;
function needsReview(word: string, probability: number): boolean {
  return !FUNCTION_WORDS.has(bareWord(word).toLowerCase()) || probability < VERY_UNSURE;
}

/** The history's name for «Залишити як є» (WORD_KEPT in journal.rs) */
const WORD_KEPT = 'слово підтверджено';

/** A word without the punctuation around it («репеті,» → «репеті») */
function bareWord(w: string): string {
  return w.replace(/^[^\p{L}\p{N}'’-]+|[^\p{L}\p{N}'’-]+$/gu, '');
}

interface WordTime {
  word: string;
  start: number;
  end: number;
  prob: number;
}
interface Doubt {
  segmentId: string;
  wordIndex: number;
  word: string;
  seconds: number;
  probability: number;
  heard?: string | null;
}
interface VoiceDoubt {
  segment_id: string;
  from: number;
  to: number;
  seconds: number;
  likely: string;
  similarity: number;
  /** The whole line sounds like `likely` (margin: how clearly) */
  whole?: boolean;
  margin?: number;
}
interface Alternative {
  text: string;
  why: string;
}
type Progress = { reviewed: number; total: number; seconds_reviewed: number; seconds_total: number };

/** One place to check, in time order: an unsure word, another voice inside a line, a voice without a name */
interface QueueItem {
  key: string;
  kind: 'word' | 'voice' | 'name';
  rowId: string;
  seconds: number;
  doubt?: Doubt;
  voice?: VoiceDoubt;
  name?: string;
}

export interface WorkspacePlayer {
  playing: boolean;
  current: number;
  duration: number;
  rate: number;
  onToggle: () => void;
  onSeek: (seconds: number) => void;
  onSkip: (delta: number) => void;
  onRate: (rate: number) => void;
}

interface Props {
  meetingId: string;
  transcripts: Transcript[];
  hasMore?: boolean;
  onLoadMore?: () => void;
  onRefetchTranscripts?: () => Promise<void>;
  onCopyTranscript: () => void;
  onOpenMeetingFolder: () => Promise<void>;
  meetingFolderPath?: string | null;
  onJumpTo?: (seconds: number) => void;
  player: WorkspacePlayer;
  onProgress?: (p: Progress | null) => void;
}

const escapeHtml = (s: string) => s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
const wordsOf = (text: string) => text.split(/\s+/).filter(Boolean);

/** The line's text as word spans: the browser edits it, React only writes it while the line is not being edited */
function lineHtml(text: string, doubtful?: Set<number>, auto?: Set<string>): string {
  return wordsOf(text)
    .map((w, i) => {
      const fixed = !!auto?.size && auto.has(bareWord(w).toLowerCase());
      return `<span data-w="${i}" class="tw${doubtful?.has(i) ? ' tw-doubt' : ''}${fixed ? ' tw-auto' : ''}">${escapeHtml(w)}</span>`;
    })
    .join(' ');
}

/** Where a point of a selection falls: the line and the word */
function wordAtPoint(node: Node | null, offset: number, towardsEnd: boolean): { rowId: string; w: number } | null {
  if (!node) return null;
  const el = node.nodeType === Node.TEXT_NODE ? node.parentElement : (node as Element);
  const row = el?.closest('[data-row]');
  if (!el || !row) return null;
  const rowId = row.getAttribute('data-row')!;
  const span = el.closest('[data-w]');
  if (span) return { rowId, w: Number(span.getAttribute('data-w')) };
  // Between words (a space) or at the paragraph itself
  const spans = Array.from(row.querySelectorAll('[data-w]'));
  if (spans.length === 0) return null;
  let probe: Node | null = node.nodeType === Node.TEXT_NODE ? node : (node.childNodes[offset] ?? null);
  if (towardsEnd) {
    probe = probe && node.nodeType === Node.TEXT_NODE ? probe.previousSibling : probe?.previousSibling ?? null;
    while (probe && !(probe instanceof Element && probe.hasAttribute('data-w'))) probe = probe.previousSibling;
    return { rowId, w: probe ? Number((probe as Element).getAttribute('data-w')) : 0 };
  }
  probe = probe && node.nodeType === Node.TEXT_NODE ? probe.nextSibling : probe;
  while (probe && !(probe instanceof Element && probe.hasAttribute('data-w'))) probe = probe.nextSibling;
  return { rowId, w: probe ? Number((probe as Element).getAttribute('data-w')) : spans.length - 1 };
}

const Row = memo(function Row({
  id,
  text,
  time,
  speaker,
  showName,
  color,
  reviewed,
  manual,
  voiceNote,
  maybe,
  onMaybe,
  doubtful,
  autoWords,
  onCommit,
  onNameClick,
  onCopySpeaker,
  onUndoSpeaker,
  registerDirty,
  onTime,
}: {
  id: string;
  text: string;
  time: number;
  speaker?: string;
  showName: boolean;
  color: string;
  reviewed: boolean;
  manual: boolean;
  voiceNote: boolean;
  /** The whole line may be someone else's: «можливо, X?» */
  maybe?: string;
  onMaybe?: (id: string, accept: boolean) => void;
  doubtful?: Set<number>;
  /** Words a learned rule fixed here by itself (lower case), underlined */
  autoWords?: Set<string>;
  /** `finished`: the line was left (Enter, a click away), not a pause in typing */
  onCommit: (id: string, text: string, finished: boolean, before?: string) => void;
  onNameClick: (id: string, speaker: string | undefined, anchor: DOMRect) => void;
  onCopySpeaker: (speaker: string, withTimes: boolean) => void;
  onUndoSpeaker: (id: string) => void;
  registerDirty: (id: string, dirty: boolean) => void;
  onTime: (seconds: number) => void;
}) {
  const ref = useRef<HTMLParagraphElement>(null);
  const timer = useRef<number | null>(null);
  const html = useMemo(() => lineHtml(text, doubtful, autoWords), [text, doubtful, autoWords]);
  // The line is written only while it is not being edited (the caret stays put)
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el || document.activeElement === el) return;
    if (el.innerHTML !== html) el.innerHTML = html;
  }, [html]);
  // Saved during a pause in typing, so leaving the line still has to teach
  const savedInPause = useRef(false);
  // The line as this edit began: what was fixed earlier is not learned again
  const baseline = useRef<string | null>(null);
  const commit = (finished: boolean) => {
    if (timer.current) window.clearTimeout(timer.current);
    timer.current = null;
    const el = ref.current;
    if (!el) return;
    const value = (el.innerText ?? '').replace(/\s+/g, ' ').trim();
    registerDirty(id, false);
    if (value && (value !== text.replace(/\s+/g, ' ').trim() || (finished && savedInPause.current))) {
      savedInPause.current = !finished;
      onCommit(id, value, finished, baseline.current ?? undefined);
    }
    if (finished) baseline.current = null;
    else if (!value || (document.activeElement !== el && el.innerHTML !== html)) el.innerHTML = html;
  };
  const unnamed = isUnnamed(speaker);
  return (
    <div className="tv-row" data-row={id}>
      <div
        className="tv-who"
        onContextMenu={(e) => {
          // The text keeps the system's own menu (copy, spelling); the name and time column gets the line's actions
          e.preventDefault();
          const anchor = e.currentTarget.getBoundingClientRect();
          void popupNativeMenu([
            { text: 'Відтворити звідси', action: () => onTime(time) },
            { text: unnamed ? 'Назвати, хто говорить…' : 'Змінити, хто говорить…', action: () => onNameClick(id, speaker, anchor) },
            ...(manual ? [{ text: 'Повернути розпізнаного спікера', action: () => onUndoSpeaker(id) }] : []),
            'separator',
            { text: 'Копіювати рядок', action: () => void navigator.clipboard?.writeText(`${mmss(time)} ${speaker ? `${speaker}: ` : ''}${text}`).catch(() => {}) },
          ]);
        }}
      >
        {showName && (
          <span className="tv-name-line">
            {speaker && (
              <button
                type="button"
                className="tv-copy-speaker"
                aria-label={`Копіювати весь текст: ${speaker}`}
                title={`Копіювати весь текст: ${speaker} · з ⌥ — із часом`}
                onClick={(e) => onCopySpeaker(speaker, e.altKey)}
              >
                <Copy className="h-3 w-3" aria-hidden="true" />
              </button>
            )}
            {unnamed ? (
              <button type="button" className="tv-name-pill" onClick={(e) => onNameClick(id, speaker, e.currentTarget.getBoundingClientRect())}>
                {speaker ?? 'Хтось'} · назвати
              </button>
            ) : (
              <button type="button" className="tv-name" style={{ color }} onClick={(e) => onNameClick(id, speaker, e.currentTarget.getBoundingClientRect())} title={`${speaker} · змінити, хто говорить у рядку`}>
                {speaker}
              </button>
            )}
          </span>
        )}
        <button type="button" className="tv-time" onClick={() => onTime(time)} title="Перейти сюди в записі">
          {mmss(time)}
          {reviewed ? ' ✓' : ''}
        </button>
        {manual && showName && (
          <button type="button" className="tv-note" onClick={() => onUndoSpeaker(id)} title="Повернути розпізнаного спікера">
            змінено вручну
          </button>
        )}
        {maybe ? (
          <span className="tv-note tv-note-voice tv-maybe">
            <button type="button" onClick={() => onMaybe?.(id, true)} title={`Це сказав ${maybe}: змінити (і запам'ятати голос)`}>
              можливо, {maybe}?
            </button>
            <button type="button" className="tv-maybe-x" onClick={() => onMaybe?.(id, false)} title="Ні, сказав той, хто вказаний" aria-label="Ні">
              ×
            </button>
          </span>
        ) : (
          voiceNote && <span className="tv-note tv-note-voice">інший голос?</span>
        )}
      </div>
      <p
        ref={ref}
        className="tv-text"
        contentEditable
        suppressContentEditableWarning
        spellCheck={false}
        onInput={() => {
          registerDirty(id, true);
          if (timer.current) window.clearTimeout(timer.current);
          timer.current = window.setTimeout(() => commit(false), 1500);
        }}
        onFocus={(e) => {
          if (baseline.current === null) baseline.current = (e.currentTarget.innerText ?? '').replace(/\s+/g, ' ').trim();
        }}
        onBlur={() => commit(true)}
        onKeyDown={(e) => {
          // One line is one paragraph: Enter saves it
          if (e.key === 'Enter' && !e.metaKey && !e.ctrlKey) {
            e.preventDefault();
            commit(true);
          }
        }}
        onPaste={(e) => {
          e.preventDefault();
          document.execCommand('insertText', false, e.clipboardData.getData('text/plain').replace(/\s+/g, ' '));
        }}
      />
    </div>
  );
});

/**
 * «Стенограма» as one editable text with the recording under it. Click
 * and type to correct; select words and say who said them; Tab goes
 * through the places worth checking. Sound starts only from the player
 * (▶, ⌥Пробіл, the ⏯ key). Every change can be undone (⌘Z / ⌘⇧Z).
 */
export function TranscriptWorkspace({
  meetingId,
  transcripts,
  hasMore,
  onLoadMore,
  onRefetchTranscripts,
  onCopyTranscript,
  onOpenMeetingFolder,
  meetingFolderPath,
  onJumpTo,
  player,
  onProgress,
}: Props) {
  perfCount('render:workspace');
  // Word times read again after they were placed by the sound
  const [wordsTick, setWordsTick] = useState(0);
  // Placing the words by the sound, once per meeting (percent; null when not running)
  const [aligning, setAligning] = useState<number | null>(null);
  // «Підсвічувати кожне слово» (Settings): off — no alignment, the whole line is lit
  const [wordsOn, setWordsOn] = useState<boolean | null>(null);
  useEffect(() => {
    invoke<boolean>('api_get_word_highlight')
      .then(setWordsOn)
      .catch(() => setWordsOn(false));
    const onChange = (e: Event) => setWordsOn(!!(e as CustomEvent<boolean>).detail);
    window.addEventListener(WORD_HIGHLIGHT_EVENT, onChange);
    return () => window.removeEventListener(WORD_HIGHLIGHT_EVENT, onChange);
  }, []);
  const wordsOnRef = useRef(wordsOn);
  wordsOnRef.current = wordsOn;
  // The meeting is on the player's timeline (its one-off move done): then words may be placed
  const [timelineReady, setTimelineReady] = useState<string | null>(null);
  // Words placed (or not to be): then the voices inside lines are checked, one heavy job at a time
  const [alignSettled, setAlignSettled] = useState<string | null>(null);
  useEffect(() => {
    if (timelineReady !== meetingId || wordsOn === null) return;
    if (!wordsOn) setAlignSettled(meetingId);
  }, [wordsOn, timelineReady, meetingId]);
  useEffect(() => {
    if (!wordsOn || timelineReady !== meetingId) return;
    let alive = true;
    // Where the player is now goes first
    void invoke('api_align_focus', { meetingId, seconds: readPlayhead() ?? 0 }).catch(() => {});
    invoke<number>('api_align_meeting_words', { meetingId })
      .then((n) => {
        if (alive && n > 0) setWordsTick((t) => t + 1);
      })
      .catch(() => {})
      .finally(() => {
        if (!alive) return;
        setAligning(null);
        setAlignSettled(meetingId);
      });
    return () => {
      alive = false;
    };
  }, [wordsOn, timelineReady, meetingId]);
  // A meeting recognised before the call/mix clock map: moved once to the mix's
  // timeline; then each word placed where it sounds (once, in the background)
  useEffect(() => {
    let alive = true;
    let unlisten: (() => void) | undefined;
    void listen<{ meeting_id: string; progress_percentage: number }>('words-align-progress', (e) => {
      if (!alive || e.payload.meeting_id !== meetingId) return;
      setAligning(e.payload.progress_percentage >= 100 ? null : e.payload.progress_percentage);
    }).then((u) => {
      if (alive) unlisten = u;
      else u();
    });
    // Parts placed by the sound so far: the highlight takes them at once
    let wordsTimer: number | undefined;
    let unlistenSaved: (() => void) | undefined;
    void listen<string>('words-aligned', (e) => {
      if (!alive || e.payload !== meetingId) return;
      window.clearTimeout(wordsTimer);
      wordsTimer = window.setTimeout(() => alive && setWordsTick((t) => t + 1), 150);
    }).then((u) => {
      if (alive) unlistenSaved = u;
      else u();
    });
    invoke<boolean>('api_align_meeting_timeline', { meetingId })
      .then((moved) => {
        if (moved) void onRefetchTranscripts?.();
      })
      .catch(() => {})
      .finally(() => alive && setTimelineReady(meetingId));
    return () => {
      alive = false;
      unlisten?.();
      unlistenSaved?.();
      window.clearTimeout(wordsTimer);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [meetingId]);
  // All lines (the list is short enough to show whole; the rest loads on its own)
  useEffect(() => {
    if (hasMore) onLoadMore?.();
  }, [hasMore, transcripts.length, onLoadMore]);
  const storedRows = useMemo(
    () =>
      [...transcripts]
        .filter((t) => !/^\s*—\s*Мікрофон (вимкнено|увімкнено)\s*—\s*$/.test(t.text ?? ''))
        .sort((a, b) => (a.audio_start_time ?? 0) - (b.audio_start_time ?? 0))
        .map((t, i, all) => ({
          id: t.id,
          text: t.text,
          start: t.audio_start_time ?? 0,
          end: t.audio_end_time ?? all[i + 1]?.audio_start_time ?? (t.audio_start_time ?? 0) + 5,
          speaker: t.speaker,
          reviewed: !!t.reviewed,
          manual: !!t.speaker_manual,
        })),
    [transcripts],
  );
  // Words with times (where they sound in the recording)
  const [lineWords, setLineWords] = useState<Map<string, WordTime[]>>(new Map());
  // Each line where it sounds: its stored times widened to its words' sound
  // (word times are placed by the recording later; the stored ones may be
  // seconds off), without overlapping its neighbours. `stored` is kept for
  // finding a line by a stored time (the document's sources, «Спікери»).
  const rows = useMemo(() => effectiveRows(storedRows, lineWords), [storedRows, lineWords]);
  const rowsRef = useRef(rows);
  rowsRef.current = rows;
  const rowIndex = useMemo(() => new Map(rows.map((r, i) => [r.id, i])), [rows]);
  // A word's place in the recording: its aligned time, else its line's start
  const wordSpan = (rowId: string, w: number): { start: number; end: number } | null => {
    const r = rows[rowIndex.get(rowId) ?? -1];
    if (!r) return null;
    const t = w >= 0 ? lineWords.get(rowId)?.[w] : undefined;
    const start = t?.start ?? r.start;
    return { start, end: t?.end ?? Math.min(r.end, start + 1.5) };
  };
  // A word on its own, with a little around it; the word itself is marked while it sounds
  const heardWord = useRef<{ rowId: string; w: number; start: number; end: number; on: boolean; played: boolean } | null>(null);
  const markHeard = (h: { rowId: string; w: number }, on: boolean) => surfaceRef.current?.querySelector(`[data-row="${h.rowId}"] [data-w="${h.w}"]`)?.classList.toggle('tw-heard', on);
  const playWord = (rowId: string, w: number) => {
    const span = wordSpan(rowId, w);
    if (!span) return;
    if (heardWord.current?.on) markHeard(heardWord.current, false);
    heardWord.current = w >= 0 ? { rowId, w, ...span, on: false, played: false } : null;
    const range = snippetRange(lineWords.get(rowId), w, span);
    playRange(range.start, range.end);
  };
  // A click on a word: while playing the recording goes there; while paused
  // the playhead moves there (the caret is put in the text as usual); ⌥-click
  // plays the word itself
  const onSurfaceClick = (e: React.MouseEvent) => {
    const target = e.target as HTMLElement;
    if (!target.closest('.tv-text')) return;
    const rowId = target.closest('[data-row]')?.getAttribute('data-row');
    if (!rowId) return;
    if (!e.altKey && window.getSelection()?.isCollapsed === false) return;
    const wEl = target.closest('[data-w]');
    const span = wordSpan(rowId, wEl ? Number(wEl.getAttribute('data-w')) : -1);
    if (!span) return;
    if (wEl?.classList.contains('tw-auto')) {
      const fix = autoByWord.get(bareWord(wEl.textContent ?? '').toLowerCase());
      if (fix) setAutoPop({ fix, rowId, w: Number(wEl.getAttribute('data-w')), rect: wEl.getBoundingClientRect() });
    }
    if (e.altKey) {
      e.preventDefault();
      playWord(rowId, wEl ? Number(wEl.getAttribute('data-w')) : -1);
    } else if (player.playing) {
      player.onSeek(Math.max(0, span.start - CLICK_LEAD_S));
    } else {
      // Settings → «Клік по слову — прослухати» (on by default); ⌥-click always plays
      if (clickPlaysWord()) playWord(rowId, wEl ? Number(wEl.getAttribute('data-w')) : -1);
      else player.onSeek(span.start);
    }
  };
  const colors = useMemo(() => speakerColorMap(rows.map((r) => r.speaker)), [rows]);
  const colorOf = (s?: string) => (s ? colors.get(s) : undefined) ?? 'var(--muted-text)';
  const refresh = useCallback(async () => {
    await onRefetchTranscripts?.();
  }, [onRefetchTranscripts]);

  // Unsure words, places where the voice changes inside a line
  const [doubts, setDoubts] = useState<Doubt[]>([]);
  const [voices, setVoices] = useState<VoiceDoubt[]>([]);
  const [progress, setProgress] = useState<Progress | null>(null);
  const [history, setHistory] = useState<{ undo?: string | null; redo?: string | null }>({});
  useEffect(() => {
    invoke<{ id: string; words: WordTime[] }[]>('get_line_words', { meetingId })
      .then((all) => setLineWords(new Map(all.map((l) => [l.id, l.words]))))
      .catch(() => {});
    invoke<Doubt[]>('api_get_doubts', { meetingId }).then(setDoubts).catch(() => setDoubts([]));
    invoke<Progress>('get_review_progress', { meetingId }).then(setProgress).catch(() => {});
    invoke<{ undo?: string; redo?: string }>('get_edit_history', { meetingId }).then(setHistory).catch(() => {});
  }, [meetingId, transcripts]);
  useEffect(() => {
    if (wordsTick === 0) return;
    invoke<{ id: string; words: WordTime[] }[]>('get_line_words', { meetingId })
      .then((all) => setLineWords(new Map(all.map((l) => [l.id, l.words]))))
      .catch(() => {});
  }, [meetingId, wordsTick]);
  // Asked again when lines or their speakers change: only changed lines are
  // listened to again (the rest is kept with the meeting)
  const voiceKey = `${meetingId}:${alignSettled === meetingId}:${storedRows.map((r) => `${r.id}/${r.speaker ?? ''}/${r.text?.length ?? 0}`).join('|')}`;
  useEffect(() => {
    if (alignSettled !== meetingId) return;
    invoke<VoiceDoubt[]>('find_voice_changes', { meetingId })
      .then((found) => {
        const merged: VoiceDoubt[] = [];
        for (const d of [...found].sort((a, b) => a.seconds - b.seconds)) {
          const last = merged[merged.length - 1];
          if (last && last.segment_id === d.segment_id && last.to === d.from && last.likely === d.likely) merged[merged.length - 1] = { ...last, to: d.to };
          else if (!merged.some((m) => m.segment_id === d.segment_id)) merged.push(d);
        }
        setVoices(merged);
      })
      .catch(() => setVoices([]));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [voiceKey]);
  useEffect(() => {
    onProgress?.(progress);
    window.dispatchEvent(new CustomEvent('protokol:review-changed'));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [progress]);
  const doubtMap = useMemo(() => {
    const m = new Map<string, Set<number>>();
    for (const d of doubts) {
      if (!m.has(d.segmentId)) m.set(d.segmentId, new Set());
      m.get(d.segmentId)!.add(d.wordIndex);
    }
    return m;
  }, [doubts]);
  // «Можливо, X?» hints dismissed by the user (kept for this meeting)
  const dismissedKey = `protokol.maybe.dismissed.${meetingId}`;
  const [dismissed, setDismissed] = useState<Set<string>>(() => {
    try {
      return new Set(JSON.parse(localStorage.getItem(dismissedKey) ?? '[]'));
    } catch {
      return new Set();
    }
  });
  const maybeOf = useMemo(() => {
    const m = new Map<string, string>();
    for (const v of voices) if (v.whole && !dismissed.has(`${v.segment_id}:${v.likely}`)) m.set(v.segment_id, v.likely);
    return m;
  }, [voices, dismissed]);
  const voiceRows = useMemo(() => new Set(voices.filter((v) => !v.whole).map((v) => v.segment_id)), [voices]);
  const maybeRef = useRef(maybeOf);
  maybeRef.current = maybeOf;
  const onMaybe = useCallback(
    async (id: string, accept: boolean) => {
      const who = maybeRef.current.get(id);
      if (!who) return;
      if (accept) {
        // As a speaker fix made by hand: the line moves and the voice is learned
        await invoke('reassign_line_speaker', { meetingId, segmentId: id, speaker: who, scope: 'line' }).catch((e) => toast.error(`${e}`));
        await refresh();
      } else {
        setDismissed((d) => {
          const next = new Set(d);
          next.add(`${id}:${who}`);
          try {
            localStorage.setItem(dismissedKey, JSON.stringify([...next]));
          } catch {
            /* kept for this session only */
          }
          return next;
        });
      }
    },
    [meetingId, refresh, dismissedKey],
  );

  // Lines being typed in: saved before anything else happens
  const dirty = useRef(new Set<string>());
  const registerDirty = useCallback((id: string, d: boolean) => {
    if (d) dirty.current.add(id);
    else dirty.current.delete(id);
  }, []);
  const flush = () => (document.activeElement as HTMLElement | null)?.closest?.('.tv-text') && (document.activeElement as HTMLElement).blur();

  const markReviewed = useCallback(
    async (ids: string[], quiet = true) => {
      if (ids.length === 0) return;
      const p = await invoke<Progress>('set_segments_reviewed', { meetingId, segmentIds: ids, reviewed: true, quiet }).catch(() => null);
      if (p) setProgress(p);
    },
    [meetingId],
  );

  // A corrected line is saved (one step of the history) and counts as checked
  // Rules made from a fix typed in the text (wrong → right), and what they changed in the rest of the meeting
  const inlineRules = useRef(new Map<string, string>());
  const applySteps = useRef(0);
  const [moreFixed, setMoreFixed] = useState(0);
  const commitLine = useCallback(
    async (id: string, text: string, finished: boolean, before?: string) => {
      try {
        const pairs = await invoke<{ wrong: string; right: string; remembered: boolean }[]>('api_edit_transcript_line', { meetingId, segmentId: id, text, finished, before });
        await markReviewed([id]);
        // A rule made from this fix: the rest of the meeting is corrected too
        const made = pairs.filter((p) => p.remembered);
        if (made.length) {
          for (const p of made) inlineRules.current.set(p.wrong, p.right);
          const more = await invoke<number>('api_apply_corrections_to_meeting', { meetingId, only: made.map((p) => p.wrong) }).catch(() => 0);
          if (more > 0) {
            applySteps.current += 1;
            setMoreFixed((n) => n + more);
          }
        }
        await refresh();
        // The corrected words placed by the sound again, in the background
        if (wordsOnRef.current) invoke<boolean>('api_realign_line', { meetingId, segmentId: id })
          .then((done) => done && setWordsTick((t) => t + 1))
          .catch(() => {});
      } catch (e) {
        toast.error(`Не вдалося зберегти рядок: ${e}`);
      }
    },
    [meetingId, markReviewed, refresh],
  );

  // ⌘Z / ⌘⇧Z: the meeting's one history (text, speakers, cuts, checks)
  const doubtsRef = useRef(doubts);
  doubtsRef.current = doubts;
  const qiRef = useRef(-1);
  const [reopen, setReopen] = useState<string | null>(null);
  const undo = useCallback(
    async (redo = false) => {
      flush();
      setStrip(null);
      const known = new Set(doubtsRef.current.map((d) => `${d.segmentId}:${d.wordIndex}`));
      const label = await invoke<string | null>(redo ? 'redo_edit' : 'undo_edit', { meetingId }).catch((e) => {
        toast.error(`${e}`);
        return null;
      });
      if (label) {
        await refresh();
        toast(`${redo ? 'Повернуто' : 'Скасовано'}: ${label}`);
        if (label === WORD_KEPT) {
          if (redo) {
            window.setTimeout(() => void showItemRef.current(qiRef.current), 120);
          } else {
            // The word that came back opens again
            const now = await invoke<Doubt[]>('api_get_doubts', { meetingId }).catch(() => [] as Doubt[]);
            const back = now.find((d) => !known.has(`${d.segmentId}:${d.wordIndex}`));
            if (back) setReopen(`w:${back.segmentId}:${back.wordIndex}`);
          }
        }
      } else toast(redo ? 'Нічого повертати' : 'Нічого скасовувати');
    },
    [meetingId, refresh],
  );

  // «Позначити все перевіреним»: one step, undone from the notice too
  const markAllReviewed = useCallback(async () => {
    flush();
    const p = await invoke<Progress>('api_mark_all_reviewed', { meetingId }).catch((e) => {
      toast.error(`${e}`);
      return null;
    });
    if (!p) return;
    setProgress(p);
    await refresh();
    toast('Усі рядки позначено перевіреними', { action: { label: 'Скасувати', onClick: () => void undo() } });
  }, [meetingId, refresh, undo]);
  useEffect(() => {
    const onAsk = () => void markAllReviewed();
    window.addEventListener(MARK_ALL_REVIEWED_EVENT, onAsk);
    return () => window.removeEventListener(MARK_ALL_REVIEWED_EVENT, onAsk);
  }, [markAllReviewed]);

  // «Хто це сказав?» for words chosen with the mouse (or by the check queue)
  type Chosen = { from: { rowId: string; w: number }; to: { rowId: string; w: number }; rect: DOMRect; likely?: string };
  const [chosen, setChosen] = useState<Chosen | null>(null);
  const chosenRef = useRef(chosen);
  chosenRef.current = chosen;
  // The bar stays inside the transcript (a narrow window: its buttons wrap
  // to a second row rather than spill over the sidebar)
  const barRef = useRef<HTMLDivElement>(null);
  const [barPlace, setBarPlace] = useState<{ left: number; top: number; maxWidth: number } | null>(null);
  const surfaceRef = useRef<HTMLDivElement>(null);
  const readSelection = (likely?: string): Chosen | null => {
    const sel = window.getSelection();
    if (!sel || sel.isCollapsed || sel.rangeCount === 0) return null;
    const range = sel.getRangeAt(0);
    if (!surfaceRef.current?.contains(range.commonAncestorContainer)) return null;
    const a = wordAtPoint(range.startContainer, range.startOffset, false);
    const b = wordAtPoint(range.endContainer, range.endOffset, true);
    if (!a || !b) return null;
    const ia = rowIndex.get(a.rowId) ?? -1;
    const ib = rowIndex.get(b.rowId) ?? -1;
    if (ia < 0 || ib < 0 || ia > ib || (ia === ib && a.w > b.w)) return null;
    return { from: a, to: b, rect: range.getBoundingClientRect(), likely };
  };
  const onSurfaceMouseUp = () => {
    window.setTimeout(() => setChosen(readSelection()), 250);
  };
  // Who the chosen words most likely belong to: a voice that sounds like them, the neighbours, then everyone
  const candidates = useMemo(() => {
    if (!chosen) return [] as { name: string; likely: boolean }[];
    const out: { name: string; likely: boolean }[] = [];
    const add = (name: string | undefined, likely = false) => {
      if (!name || isUnnamed(name) || name === 'Репліки' || out.some((o) => o.name === name)) return;
      out.push({ name, likely });
    };
    const voice = voices.find((v) => v.segment_id === chosen.from.rowId);
    add(chosen.likely ?? voice?.likely, true);
    const ia = rowIndex.get(chosen.from.rowId) ?? 0;
    const ib = rowIndex.get(chosen.to.rowId) ?? 0;
    const own = rows[ia]?.speaker;
    for (const k of [ia - 1, ib + 1, ia - 2, ib + 2]) if (rows[k] && rows[k].speaker !== own) add(rows[k].speaker);
    const time = new Map<string, number>();
    rows.forEach((r) => r.speaker && time.set(r.speaker, (time.get(r.speaker) ?? 0) + (r.end - r.start)));
    [...time.entries()].sort((x, y) => y[1] - x[1]).forEach(([n]) => n !== own && add(n));
    return out.slice(0, 4);
  }, [chosen, voices, rows, rowIndex]);
  useLayoutEffect(() => {
    if (!chosen) {
      setBarPlace(null);
      return;
    }
    const pane = surfaceRef.current?.getBoundingClientRect();
    const lo = (pane?.left ?? 0) + 8;
    const hi = (pane?.right ?? window.innerWidth) - 8;
    const maxWidth = Math.max(200, hi - lo);
    const el = barRef.current;
    if (el) el.style.maxWidth = `${maxWidth}px`;
    const w = el?.offsetWidth ?? 0;
    const h = el?.offsetHeight ?? 40;
    setBarPlace({ left: Math.max(lo, Math.min(chosen.rect.left, hi - w)), top: Math.max(8, chosen.rect.top - h - 8), maxWidth });
  }, [chosen, candidates.length]);

  const [picker, setPicker] = useState<{ id: string; current?: string; anchor: DOMRect; chosen?: Chosen } | null>(null);
  const assign = async (c: Chosen, name: string) => {
    setChosen(null);
    window.getSelection()?.removeAllRanges();
    flush();
    const ia = rowIndex.get(c.from.rowId)!;
    const ib = rowIndex.get(c.to.rowId)!;
    try {
      for (let k = ia; k <= ib; k++) {
        const r = rows[k];
        const n = wordsOf(r.text).length;
        const from = k === ia ? c.from.w : 0;
        const to = k === ib ? c.to.w + 1 : n;
        if (from >= to) continue;
        if (from === 0 && to === n) await invoke('reassign_line_speaker', { meetingId, segmentId: r.id, speaker: name, scope: 'line' });
        else await invoke('assign_words_speaker', { meetingId, segmentId: r.id, from, to, speaker: name });
      }
      setVoices((all) => all.filter((v) => v.segment_id !== c.from.rowId));
      await refresh();
      toast.success(`Слова тепер — ${name}`, { description: '⌘Z — скасувати' });
    } catch (e) {
      toast.error(`${e}`);
    }
  };
  const changeLineSpeaker = async (name: string, scope: SpeakerScope) => {
    if (!picker) return;
    const p = picker;
    setPicker(null);
    if (p.chosen) return assign(p.chosen, name);
    try {
      if (scope === 'all') {
        if (p.current) await invoke('rename_speaker', { meetingId, current: p.current, name });
      } else await invoke('reassign_line_speaker', { meetingId, segmentId: p.id, speaker: name, scope });
      await refresh();
    } catch (e) {
      toast.error(`${e}`);
    }
  };
  const undoSpeaker = useCallback(
    async (id: string) => {
      await invoke('undo_line_speaker', { meetingId, segmentId: id }).catch((e) => toast.error(`${e}`));
      await refresh();
    },
    [meetingId, refresh],
  );
  // The time under a name: the player goes there (and keeps playing if it was)
  const seekRef = useRef(player.onSeek);
  const setFollowRef = useRef<(v: boolean) => void>(() => {});
  seekRef.current = player.onSeek;
  const seekRow = useCallback((seconds: number) => {
    setFollowRef.current(true);
    seekRef.current(seconds);
  }, []);
  const onNameClick = useCallback((id: string, current: string | undefined, anchor: DOMRect) => setPicker({ id, current, anchor }), []);
  // The whole meeting as it reads now, a line being typed included
  const onCopySpeaker = useCallback((who: string, withTimes: boolean) => {
    const lines = rowsRef.current.map((r) => ({
      speaker: r.speaker,
      start: r.start,
      text: dirty.current.has(r.id) ? ((surfaceRef.current?.querySelector(`[data-row="${r.id}"] .tv-text`) as HTMLElement | null)?.innerText ?? r.text) : r.text,
    }));
    copySpeakerText(lines, who, withTimes);
  }, []);

  // «Перевірте»: unsure words, other voices, voices without a name — in time order
  const queue: QueueItem[] = useMemo(() => {
    const items: QueueItem[] = [];
    const seen = new Set<string>();
    for (const d of doubts) if (rowIndex.has(d.segmentId) && needsReview(d.word, d.probability) && !seen.has(`${d.segmentId}:${d.wordIndex}`) && seen.add(`${d.segmentId}:${d.wordIndex}`)) items.push({ key: `w:${d.segmentId}:${d.wordIndex}`, kind: 'word', rowId: d.segmentId, seconds: d.seconds, doubt: d });
    // A whole line in the queue only when clearly someone else's; less clear ones are a quiet hint on the line
    for (const v of voices) if (rowIndex.has(v.segment_id) && (!v.whole || (v.margin ?? 0) >= 0.1)) items.push({ key: `v:${v.segment_id}`, kind: 'voice', rowId: v.segment_id, seconds: v.seconds, voice: v });
    const named = new Set<string>();
    for (const r of rows) {
      if (r.speaker && (isUnnamed(r.speaker) || /\?\s*$/.test(r.speaker)) && r.speaker !== '…' && !named.has(r.speaker)) {
        named.add(r.speaker);
        items.push({ key: `n:${r.speaker}`, kind: 'name', rowId: r.id, seconds: r.start, name: r.speaker });
      }
    }
    return items.sort((a, b) => a.seconds - b.seconds);
  }, [doubts, voices, rows, rowIndex]);
  const [qi, setQi] = useState(-1);
  qiRef.current = qi;
  useEffect(() => {
    if (!reopen) return;
    const k = queue.findIndex((q) => q.key === reopen);
    if (k < 0) return;
    setReopen(null);
    void showItemRef.current(k);
  }, [reopen, queue]);
  const [strip, setStrip] = useState<{ item: QueueItem; options: Alternative[]; rect: DOMRect; remember: boolean; typed: string } | null>(null);
  const stripInput = useRef<HTMLInputElement>(null);
  const stripRef = useRef(strip);
  stripRef.current = strip;
  const selectWords = (rowId: string, from: number, to: number): DOMRect | null => {
    const row = surfaceRef.current?.querySelector(`[data-row="${rowId}"]`);
    const a = row?.querySelector(`[data-w="${from}"]`);
    const b = row?.querySelector(`[data-w="${to}"]`);
    if (!a || !b) return null;
    const range = document.createRange();
    range.setStartBefore(a);
    range.setEndAfter(b);
    const sel = window.getSelection();
    sel?.removeAllRanges();
    sel?.addRange(range);
    return range.getBoundingClientRect();
  };
  const showItem = async (k: number) => {
    if (queue.length === 0) return;
    const index = ((k % queue.length) + queue.length) % queue.length;
    const item = queue[index];
    setQi(index);
    setStrip(null);
    setChosen(null);
    flush();
    surfaceRef.current?.querySelector(`[data-row="${item.rowId}"]`)?.scrollIntoView({ block: 'center' });
    player.onSeek(item.seconds);
    await new Promise((r) => window.setTimeout(r, 60));
    if (item.kind === 'voice' && item.voice) {
      const rect = selectWords(item.rowId, item.voice.from, item.voice.to - 1);
      if (rect) setChosen({ from: { rowId: item.rowId, w: item.voice.from }, to: { rowId: item.rowId, w: item.voice.to - 1 }, rect, likely: item.voice.likely });
    } else if (item.kind === 'word' && item.doubt) {
      const rect = selectWords(item.rowId, item.doubt.wordIndex, item.doubt.wordIndex);
      if (!rect) return;
      const options = await invoke<Alternative[]>('api_word_alternatives', { meetingId, segmentId: item.rowId, wordIndex: item.doubt.wordIndex }).catch(() => []);
      setStrip({ item, options, rect, remember: true, typed: bareWord(item.doubt.word) });
    } else if (item.kind === 'name') {
      const el = surfaceRef.current?.querySelector(`[data-row="${item.rowId}"] .tv-name-pill, [data-row="${item.rowId}"] .tv-name`);
      setPicker({ id: item.rowId, current: item.name, anchor: (el ?? surfaceRef.current!).getBoundingClientRect() });
    }
  };
  const showItemRef = useRef(showItem);
  showItemRef.current = showItem;
  // The strip under an unsure word: pick an option, keep the word, or type
  const resolveWord = async (choice: string | null) => {
    const s = strip;
    if (!s || !s.item.doubt) return;
    const d = s.item.doubt;
    setStrip(null);
    const row = rows[rowIndex.get(s.item.rowId) ?? -1];
    try {
      if (choice && row) {
        const words = wordsOf(row.text);
        const old = words[d.wordIndex] ?? '';
        const core = old.replace(/^[^\p{L}\p{N}'’-]+|[^\p{L}\p{N}'’-]+$/gu, '');
        words[d.wordIndex] = core ? old.replace(core, choice) : choice;
        // Teaches only through «Запам'ятати» below, not as a fix typed in the text
        await invoke('api_edit_transcript_line', { meetingId, segmentId: row.id, text: words.join(' '), finished: false, reviewed: true });
        if (s.remember && core && core !== choice) await invoke('api_remember_corrections', { pairs: [{ wrong: core, right: choice }], example: words.join(' '), meetingId }).catch(() => {});
      } else if (dirty.current.has(s.item.rowId)) {
        flush();
      } else {
        await invoke('api_resolve_doubt', { meetingId, seconds: d.seconds, word: d.word, segmentId: d.segmentId }).catch(() => {});
      }
      setDoubts((all) => all.filter((x) => !(x.segmentId === d.segmentId && x.wordIndex === d.wordIndex)));
      await refresh();
      // The next item from the list as it is now (this render's list still has the word just done)
      window.setTimeout(() => void showItemRef.current(qi), 120);
    } catch (e) {
      toast.error(`${e}`);
    }
  };

  // «Додано в пам'ять»: what a fix here stored (a word rule, a voice), shown
  // for a few seconds with «Скасувати»; fixes in quick succession together
  type Learned = { kind: string; from: string; to: string; count: number; meeting?: string };
  const [learned, setLearned] = useState<Learned[]>([]);
  const learnedBatch = useRef<Learned[]>([]);
  const learnedTimer = useRef<number | null>(null);
  const learnedHide = useRef<number | null>(null);
  useEffect(() => {
    let off: (() => void) | undefined;
    void listen<Learned>('learned', (e) => {
      const l = e.payload;
      if (l.meeting !== meetingId || (l.kind !== 'word' && l.kind !== 'voice')) return;
      learnedBatch.current.push(l);
      if (learnedTimer.current) window.clearTimeout(learnedTimer.current);
      learnedTimer.current = window.setTimeout(() => {
        const batch = learnedBatch.current;
        learnedBatch.current = [];
        setLearned((shown) => [...shown, ...batch]);
        if (learnedHide.current) window.clearTimeout(learnedHide.current);
        learnedHide.current = window.setTimeout(() => {
          setLearned([]);
          setMoreFixed(0);
          applySteps.current = 0;
          inlineRules.current.clear();
        }, 4000);
      }, 700);
    }).then((u) => (off = u));
    return () => {
      off?.();
      if (learnedTimer.current) window.clearTimeout(learnedTimer.current);
      if (learnedHide.current) window.clearTimeout(learnedHide.current);
    };
  }, [meetingId]);
  // Words fixed by a learned rule by itself in this meeting (dashed underline),
  // and how much was added to memory today (toolbar)
  type AutoFix = { from: string; to: string; count: number; learnedAt?: string | null };
  const [autoFixes, setAutoFixes] = useState<AutoFix[]>([]);
  const [today, setToday] = useState(0);
  const loadMemory = useCallback(() => {
    invoke<AutoFix[]>('api_auto_fixes', { meetingId }).then(setAutoFixes).catch(() => setAutoFixes([]));
    const midnight = new Date();
    midnight.setHours(0, 0, 0, 0);
    invoke<{ words: number; voices: number }>('api_learned_summary', { since: midnight.toISOString() })
      .then((s) => setToday(s.words + s.voices))
      .catch(() => setToday(0));
  }, [meetingId]);
  useEffect(loadMemory, [loadMemory]);
  useEffect(() => {
    if (learned.length) loadMemory();
  }, [learned.length, loadMemory]);
  const autoByWord = useMemo(() => {
    const m = new Map<string, AutoFix>();
    for (const f of autoFixes) for (const w of f.to.split(/\s+/)) if (bareWord(w)) m.set(bareWord(w).toLowerCase(), f);
    return m;
  }, [autoFixes]);
  const autoWords = useMemo(() => new Set(autoByWord.keys()), [autoByWord]);
  const autoCount = autoFixes.reduce((n, f) => n + f.count, 0);
  const [autoPop, setAutoPop] = useState<{ fix: AutoFix; rowId: string; w: number; rect: DOMRect } | null>(null);
  const router = useRouter();
  useEffect(() => {
    if (!autoPop) return;
    const close = () => setAutoPop(null);
    const key = (e: KeyboardEvent) => e.key === 'Escape' && close();
    window.addEventListener('mousedown', close);
    window.addEventListener('keydown', key);
    return () => {
      window.removeEventListener('mousedown', close);
      window.removeEventListener('keydown', key);
    };
  }, [autoPop]);
  // «Повернути як було»: the word goes back to what was heard (the rule stays)
  const revertAuto = async () => {
    const p = autoPop;
    setAutoPop(null);
    const row = p ? rows[rowIndex.get(p.rowId) ?? -1] : undefined;
    if (!p || !row) return;
    const words = wordsOf(row.text);
    const old = words[p.w] ?? '';
    const core = bareWord(old);
    words[p.w] = core ? old.replace(core, p.fix.from) : p.fix.from;
    try {
      await invoke('api_edit_transcript_line', { meetingId, segmentId: row.id, text: words.join(' '), finished: false });
      await refresh();
    } catch (e) {
      toast.error(`${e}`);
    }
  };

  // «Скасувати»: the rules are forgotten and each fix is undone
  const undoLearned = async () => {
    const items = learned;
    setLearned([]);
    try {
      // A rule from a fix in the text: only the rule goes, the fix stays (and what it changed elsewhere is put back)
      const typed = (l: Learned) => l.kind === 'word' && inlineRules.current.has(l.from);
      for (const l of items) {
        if (typed(l)) await invoke('api_forget_inline_rule', { wrong: l.from, right: inlineRules.current.get(l.from) });
        else if (l.kind === 'word') await invoke('api_delete_correction', { wrong: l.from });
      }
      for (let n = applySteps.current; n > 0; n--) {
        const h = await invoke<{ undo?: string | null }>('get_edit_history', { meetingId }).catch(() => ({ undo: null }));
        if (h.undo === 'виправлення зі словника') await undo(false);
      }
      applySteps.current = 0;
      setMoreFixed(0);
      for (const l of items) if (!typed(l)) await undo(false);
    } catch (e) {
      toast.error(`${e}`);
    }
  };

  // A click outside the strip closes it, as Esc does: nothing saved, the item stays in the queue
  const stripOpen = !!strip;
  useEffect(() => {
    if (!stripOpen) return;
    const away = (e: MouseEvent) => {
      if (!(e.target as HTMLElement | null)?.closest('.tv-strip')) setStrip(null);
    };
    window.addEventListener('mousedown', away, true);
    return () => window.removeEventListener('mousedown', away, true);
  }, [stripOpen]);

  // What the strip's field would change the word to; null when it would stay as it is
  const stripFix = (): string | null => {
    if (!strip || dirty.current.has(strip.item.rowId)) return null;
    const typed = strip.typed.trim();
    return typed && bareWord(typed) !== bareWord(strip.item.doubt?.word ?? '') ? typed : null;
  };
  // Enter in the strip: the blue button, whichever it is now
  const submitStrip = () => {
    if (strip) void resolveWord(stripFix());
  };
  // The strip's field gets the keys as it opens, its word selected (typing replaces it)
  const stripKey = strip ? `${strip.item.rowId}:${strip.item.doubt?.wordIndex}` : '';
  useEffect(() => {
    if (!stripKey) return;
    const el = stripInput.current;
    el?.focus({ preventScroll: true });
    el?.select();
  }, [stripKey]);

  // Keys: ⌘Z/⌘⇧Z history, Tab the next place, ⌘1–4 a person for the chosen
  // words, digits for an option while the strip is open, ⌘↩ line checked,
  // ⌥Space listens (the word under the strip, else play/pause)
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const t = e.target as HTMLElement | null;
      const inField = !!t?.closest('input, textarea, select, [role="dialog"]');
      // In the strip's field ⌘Z undoes typing; with nothing typed there it is the history's
      const fieldUntouched = !!strip && t === stripInput.current && strip.typed.trim() === bareWord(strip.item.doubt?.word ?? '');
      if ((e.metaKey || e.ctrlKey) && e.code === 'KeyZ' && (!inField || fieldUntouched)) {
        e.preventDefault();
        void undo(e.shiftKey);
        return;
      }
      if (chosen && (e.metaKey || e.ctrlKey) && /^Digit[1-4]$/.test(e.code)) {
        const c = candidates[Number(e.code.slice(5)) - 1];
        if (c) {
          e.preventDefault();
          void assign(chosen, c.name);
        }
        return;
      }
      if (chosen && !['Shift', 'Meta', 'Control', 'Alt'].includes(e.key)) setChosen(null);
      if (strip) {
        // An option by its number: ⌘1–5 while typing in the strip's field, else the digit
        const typing = t === stripInput.current;
        if (/^Digit[1-5]$/.test(e.code) && !e.altKey && (typing ? e.metaKey || e.ctrlKey : !e.metaKey)) {
          const o = strip.options[Number(e.code.slice(5)) - 1];
          if (o) {
            e.preventDefault();
            void resolveWord(o.text);
            return;
          }
        }
        if (e.key === 'Enter' && !(e.metaKey || e.ctrlKey)) {
          e.preventDefault();
          submitStrip();
          return;
        }
        if (e.key === 'Escape') {
          setStrip(null);
          return;
        }
      }
      if (e.key === 'Tab' && (!inField || t === stripInput.current) && queue.length > 0) {
        e.preventDefault();
        void showItem(qi + (e.shiftKey ? -1 : 1));
        return;
      }
      if ((e.metaKey || e.ctrlKey) && e.key === 'Enter') {
        const row = t?.closest('[data-row]')?.getAttribute('data-row');
        if (row) {
          e.preventDefault();
          void markReviewed([row], false).then(() => refresh());
        }
        return;
      }
      if (e.altKey && e.code === 'Space' && strip?.item.doubt) {
        e.preventDefault();
        e.stopImmediatePropagation();
        playWord(strip.item.rowId, strip.item.doubt.wordIndex);
      }
    };
    window.addEventListener('keydown', onKey, true);
    return () => window.removeEventListener('keydown', onKey, true);
  });

  // While playing: a thin mark before the word being heard; a line heard
  // from its start to its end counts as checked
  const heardFrom = useRef(new Map<string, number>());
  const lastT = useRef(0);
  const playingRow = useMemo(() => {
    let k = -1;
    for (let i = 0; i < rows.length; i++) if (rows[i].start <= player.current + 0.05) k = i;
    return k;
  }, [rows, player.current]);
  const saidRow = useRef<string | null>(null);
  const [playingOut, setPlayingOut] = useState(false);
  // While playing the text follows the voice, until the secretary scrolls
  // away or works in the text; «До місця, що звучить» follows again
  const [follow, setFollow] = useState(true);
  setFollowRef.current = setFollow;
  const autoScrolling = useRef(false);
  const lastActivity = useRef(0);
  useEffect(() => {
    if (player.playing) setFollow(true);
  }, [player.playing]);
  // While a meeting is being placed by the sound, the part being heard goes
  // first: the player's place is told on play, on a jump and every few seconds
  const focusSent = useRef({ t: -10, playing: false });
  useEffect(() => {
    const last = focusSent.current;
    const started = player.playing && !last.playing;
    last.playing = player.playing;
    if (aligning === null || (!started && Math.abs(player.current - last.t) < 3)) return;
    last.t = player.current;
    void invoke('api_align_focus', { meetingId, seconds: player.current }).catch(() => {});
  }, [aligning, player.playing, player.current, meetingId]);
  useEffect(() => {
    const el = surfaceRef.current;
    if (!el) return;
    // Scrolling by hand (wheel, trackpad, scrollbar) or typing stops following
    const away = () => {
      if (autoScrolling.current) return;
      lastActivity.current = Date.now();
      setFollow(false);
    };
    el.addEventListener('scroll', away, { passive: true });
    el.addEventListener('keydown', away);
    return () => {
      el.removeEventListener('scroll', away);
      el.removeEventListener('keydown', away);
    };
  }, []);
  // …and starts again after 8 s of quiet while it plays
  useEffect(() => {
    if (follow || !player.playing) return;
    const timer = window.setInterval(() => {
      const busy = !!document.activeElement?.closest?.('.tv-text') || !!chosenRef.current || !!stripRef.current;
      if (!busy && Date.now() - lastActivity.current > 8000) setFollow(true);
    }, 1000);
    return () => window.clearInterval(timer);
  }, [follow, player.playing]);
  // Painting the moment heard: done straight on the page (no re-render of
  // the lines), every frame while playing, from the player's own clock
  const outRef = useRef(false);
  const showOut = (v: boolean) => {
    if (outRef.current !== v) {
      outRef.current = v;
      setPlayingOut(v);
    }
  };
  // Words are lit a moment before they sound (their times are where they start)
  const LEAD = 0.05;
  const painted = useRef({ key: '', at: 0 });
  // The lit row's word elements and how far they are lit (changed a word at a time)
  const litWords = useRef<{ row: string | null; els: Element[]; upTo: number }>({ row: null, els: [], upTo: -1 });
  const paint = (t: number, playing: boolean) => {
    const started = perfOn() ? performance.now() : 0;
    const heard = heardWord.current;
    if (heard) {
      const on = playing && t >= heard.start && t < heard.end;
      if (on !== heard.on) {
        heard.on = on;
        markHeard(heard, on);
      }
      if (playing) heard.played = true;
      else if (heard.played) heardWord.current = null;
    }
    const root = surfaceRef.current;
    let k = -1;
    for (let i = 0; i < rows.length; i++) if (rows[i].start <= t + LEAD) k = i;
    const r = rows[k];
    const whole = wordsOnRef.current === false;
    const words = r && !whole ? lineWords.get(r.id) : undefined;
    let w = -1;
    if (words) for (let i = 0; i < words.length && words[i].start <= t + LEAD; i++) w = i;
    // Nothing new to show (same line, same word): only a look at follow now and then
    const now = performance.now();
    const key = r ? `${r.id}:${w}:${whole}` : '';
    const changed = key !== painted.current.key || saidRow.current !== (r?.id ?? null);
    if (playing && !changed && now - painted.current.at < 500) return;
    painted.current = { key, at: now };
    // Following first (reading the layout before anything is changed, so nothing is forced)
    if (root && playing && r) {
      const rowEl0 = root.querySelector(`[data-row="${r.id}"]`);
      if (rowEl0) {
        const a = rowEl0.getBoundingClientRect();
        const bx = root.getBoundingClientRect();
        const out = a.top < bx.top + 8 || a.bottom > bx.bottom - 8;
        const busy = !!document.activeElement?.closest?.('.tv-text') || !!chosenRef.current || !!stripRef.current;
        if (out && followRef.current && !busy && !autoScrolling.current) {
          autoScrolling.current = true;
          root.scrollTo({ top: root.scrollTop + (a.top - bx.top) - bx.height * 0.3, behavior: 'smooth' });
          window.setTimeout(() => {
            autoScrolling.current = false;
          }, 900);
          showOut(false);
        } else if (!autoScrolling.current) showOut(out && (!followRef.current || busy));
      }
    } else showOut(false);
    if (changed) {
      const lit = litWords.current;
      if (saidRow.current && saidRow.current !== r?.id) {
        const old = root?.querySelector(`[data-row="${saidRow.current}"]`);
        old?.classList.remove('tv-row-playing', 'tv-row-lit');
        lit.els.forEach((x) => x.classList.remove('tw-said', 'tw-now'));
        litWords.current = { row: null, els: [], upTo: -1 };
        saidRow.current = null;
      }
      if (r && (playing || t > 0)) {
        const rowEl = root?.querySelector(`[data-row="${r.id}"]`);
        if (rowEl) {
          if (saidRow.current !== r.id) {
            rowEl.classList.add('tv-row-playing');
            saidRow.current = r.id;
          }
          rowEl.classList.toggle('tv-row-lit', whole);
          let cur = litWords.current;
          if (cur.row !== r.id) {
            cur = litWords.current = { row: r.id, els: Array.from(rowEl.querySelectorAll('[data-w]')), upTo: -1 };
          }
          const target = words ? w : -1;
          if (target < cur.upTo) {
            // Back (a jump): lit again from the start
            cur.els.forEach((x) => x.classList.remove('tw-said', 'tw-now'));
            cur.upTo = -1;
          }
          // The word being said is lit at once, with those before it
          for (let i = cur.upTo + 1; i <= target && i < cur.els.length; i++) cur.els[i]?.classList.add('tw-said');
          cur.els[cur.upTo]?.classList.remove('tw-now');
          if (words) cur.els[Math.max(0, target)]?.classList.add('tw-now');
          cur.upTo = target;
        }
      }
    }
    if (started) perfAdd('paint', performance.now() - started, true);
    // Checked by listening
    if (!playing || Math.abs(t - lastT.current) > 2) heardFrom.current.clear();
    lastT.current = t;
    if (playing) {
      for (const row of rows) {
        if (t >= row.start - 0.3 && t <= row.start + 1 && !heardFrom.current.has(row.id)) heardFrom.current.set(row.id, t);
        if (heardFrom.current.has(row.id) && t >= row.end - 0.2 && !row.reviewed) {
          heardFrom.current.delete(row.id);
          void markReviewed([row.id]);
        }
      }
    }
  };
  const paintRef = useRef(paint);
  paintRef.current = paint;
  const followRef = useRef(follow);
  followRef.current = follow;
  // Paused or moved: paint once
  useEffect(() => {
    if (!player.playing) paintRef.current(player.current, false);
  }, [player.current, player.playing, lineWords, rows]);
  // Playing: every frame from the player's clock
  useEffect(() => {
    if (!player.playing) return;
    return followPlayhead((t) => paintRef.current(t, true));
  }, [player.playing]);

  // A line shown from elsewhere («Джерело пункту», «Спікери»): scrolled to, no sound
  useEffect(() => {
    const onJump = (e: Event) => {
      const seconds = (e as CustomEvent<{ seconds: number }>).detail?.seconds;
      if (typeof seconds !== 'number') return;
      // Found by the line's stored time (what the document and «Спікери» know)
      let k = 0;
      rows.forEach((r, i) => {
        if (r.stored.start <= seconds + 0.5) k = i;
      });
      const el = surfaceRef.current?.querySelector(`[data-row="${rows[k]?.id}"]`);
      el?.scrollIntoView({ block: 'center' });
      // …and the player put where it sounds
      const seek = (e as CustomEvent<{ seek?: boolean }>).detail?.seek;
      if (seek && rows[k]) seekRef.current(Math.max(0, rows[k].start - 0.3));
      el?.classList.add('tv-flash');
      window.setTimeout(() => el?.classList.remove('tv-flash'), 1500);
    };
    window.addEventListener(JUMP_TO_TIME_EVENT, onJump);
    window.addEventListener(LINE_CHANGED_EVENT, onJump);
    return () => {
      window.removeEventListener(JUMP_TO_TIME_EVENT, onJump);
      window.removeEventListener(LINE_CHANGED_EVENT, onJump);
    };
  }, [rows]);

  // Search
  const [searching, setSearching] = useState(false);
  const [query, setQuery] = useState('');
  const findNext = () => {
    const q = query.trim().toLowerCase();
    if (!q) return;
    const sel = window.getSelection();
    const fromRow = sel?.anchorNode ? (sel.anchorNode.nodeType === 3 ? sel.anchorNode.parentElement : (sel.anchorNode as Element))?.closest('[data-row]')?.getAttribute('data-row') : null;
    const start = fromRow ? (rowIndex.get(fromRow) ?? -1) + 1 : 0;
    for (let n = 0; n < rows.length; n++) {
      const r = rows[(start + n) % rows.length];
      const words = wordsOf(r.text);
      const i = words.findIndex((w) => w.toLowerCase().includes(q));
      if (i >= 0) {
        surfaceRef.current?.querySelector(`[data-row="${r.id}"]`)?.scrollIntoView({ block: 'center' });
        selectWords(r.id, i, i);
        player.onSeek(lineWords.get(r.id)?.[i]?.start ?? r.start);
        return;
      }
    }
    toast('Не знайдено');
  };

  // The words chosen, as a time band on the timeline
  const band = useMemo((): [number, number] | null => {
    if (!chosen) return null;
    const a = lineWords.get(chosen.from.rowId)?.[chosen.from.w];
    const b = lineWords.get(chosen.to.rowId)?.[chosen.to.w];
    return a && b ? [a.start, b.end] : null;
  }, [chosen, lineWords]);
  const timelineLines: TimelineLine[] = useMemo(() => rows.map((r) => ({ id: r.id, start: r.start, end: r.end, speaker: r.speaker ?? '—' })), [rows]);
  // Choosing on the timeline: the words of that stretch are chosen in the text
  const chooseRange = (a: number, b: number) => {
    const inRange: { rowId: string; w: number }[] = [];
    for (const r of rows) {
      if (r.end < a || r.start > b) continue;
      (lineWords.get(r.id) ?? []).forEach((w, i) => {
        if (w.end > a && w.start < b) inRange.push({ rowId: r.id, w: i });
      });
    }
    if (inRange.length === 0) return;
    const first = inRange[0];
    const last = inRange[inRange.length - 1];
    const el1 = surfaceRef.current?.querySelector(`[data-row="${first.rowId}"] [data-w="${first.w}"]`);
    const el2 = surfaceRef.current?.querySelector(`[data-row="${last.rowId}"] [data-w="${last.w}"]`);
    if (!el1 || !el2) return;
    const range = document.createRange();
    range.setStartBefore(el1);
    range.setEndAfter(el2);
    window.getSelection()?.removeAllRanges();
    window.getSelection()?.addRange(range);
    el1.scrollIntoView({ block: 'center' });
    window.setTimeout(() => setChosen({ from: first, to: last, rect: range.getBoundingClientRect() }), 80);
  };
  // Moving the border between two people's lines: the words that cross it change hands
  const moveBorder = async (leftId: string, rightId: string, t: number) => {
    const L = rows[rowIndex.get(leftId) ?? -1];
    const R = rows[rowIndex.get(rightId) ?? -1];
    if (!L || !R || !L.speaker || !R.speaker) return;
    try {
      if (t < L.end) {
        const words = lineWords.get(L.id) ?? [];
        const k = words.findIndex((w) => w.start >= t);
        if (k <= 0) return;
        const id = await invoke<string>('assign_words_speaker', { meetingId, segmentId: L.id, from: k, to: words.length, speaker: R.speaker });
        await invoke('merge_segments', { meetingId, segmentId: id });
      } else {
        const words = lineWords.get(R.id) ?? [];
        let k = words.findIndex((w) => w.start >= t);
        if (k < 0) k = words.length;
        if (k <= 0 || k >= words.length) return;
        await invoke<string>('assign_words_speaker', { meetingId, segmentId: R.id, from: 0, to: k, speaker: L.speaker });
        await invoke('merge_segments', { meetingId, segmentId: L.id });
      }
      await refresh();
    } catch (e) {
      toast.error(`${e}`);
    }
  };

  const qItem = qi >= 0 ? queue[qi] : undefined;
  return (
    <div className="card tv relative flex h-full min-w-0 w-full flex-col overflow-hidden">
      {/* Toolbar */}
      <div className="tv-toolbar" data-notice-anchor>
        <button type="button" className="tv-icon tv-hist" onClick={() => void undo(false)} disabled={!history.undo} title={history.undo ? `Скасувати: ${history.undo} · ⌘Z` : 'Скасувати · ⌘Z'} aria-label="Скасувати">
          <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" aria-hidden="true"><path d="M9 14L4 9l5-5" /><path d="M4 9h10a6 6 0 010 12h-3" /></svg>
        </button>
        <button type="button" className="tv-icon tv-hist" onClick={() => void undo(true)} disabled={!history.redo} title={history.redo ? `Повторити: ${history.redo} · ⌘⇧Z` : 'Повторити · ⌘⇧Z'} aria-label="Повторити">
          <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" aria-hidden="true"><path d="M15 14l5-5-5-5" /><path d="M20 9H10a6 6 0 000 12h3" /></svg>
        </button>
        <span className="tb-sep tv-hist" aria-hidden="true" />
        {queue.length > 0 && (
          <div className="tv-queue">
            <span title="Перевірте: слова, у яких розпізнавання не впевнене, та голоси без імені">
              <span className="tv-queue-label">Перевірте: </span>
              {qi >= 0 ? qi + 1 : 0} з {queue.length}
            </span>
            <button type="button" aria-label="Попереднє (⇧Tab)" title="Попереднє · ⇧Tab" onClick={() => void showItem(qi - 1)}>
              <ChevronLeft className="h-4 w-4" />
            </button>
            <button type="button" aria-label="Наступне (Tab)" title="Наступне · Tab" className="tv-queue-next" onClick={() => void showItem(qi + 1)}>
              <ChevronRight className="h-4 w-4" />
            </button>
          </div>
        )}
        {progress && progress.reviewed < progress.total && (
          <button type="button" className="tv-icon" aria-label="Позначити все перевіреним" title={`Позначити все перевіреним · зараз ${progress.reviewed} з ${progress.total}`} onClick={() => void markAllReviewed()}>
            <CheckCheck className="h-4 w-4" />
          </button>
        )}
        {autoCount > 0 && (
          <span className="tv-chip-auto" title="Ці слова виправлено за вашими правилами; їх підкреслено пунктиром">
            Виправлено автоматично: {autoCount} {plural(autoCount, 'слово', 'слова', 'слів')}
          </span>
        )}
        {today > 0 && (
          <button type="button" className="tv-chip-memory" onClick={() => router.push('/templates?tab=memory')} title="Що додано в пам'ять: Словник → Пам'ять">
            У пам&apos;яті: +{today} сьогодні
          </button>
        )}
        {aligning !== null && (
          <span className="tv-aligning" title="Слова ставляться точно за звуком — один раз для цього запису">
            <span className="tv-aligning-label">Звірка слів зі звуком… </span>
            {aligning}%
          </span>
        )}
        <div className="flex-1" />
        {searching ? (
          <input
            autoFocus
            className="tv-search"
            placeholder="Пошук"
            aria-label="Пошук у стенограмі"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter') findNext();
              if (e.key === 'Escape') setSearching(false);
            }}
            onBlur={() => !query && setSearching(false)}
          />
        ) : (
          <button type="button" className="tv-icon tv-find" aria-label="Пошук" title="Пошук" onClick={() => setSearching(true)}>
            <Search className="h-4 w-4" />
          </button>
        )}
        <TranscriptButtonGroup
          transcriptCount={rows.length}
          onCopyTranscript={onCopyTranscript}
          onOpenMeetingFolder={onOpenMeetingFolder}
          meetingId={meetingId}
          meetingFolderPath={meetingFolderPath}
          onRefetchTranscripts={onRefetchTranscripts}
          onJumpTo={onJumpTo}
        />
      </div>

      {/* The transcript */}
      <div className="relative flex min-h-0 flex-1 flex-col">
      {playingOut && (
        <button
          type="button"
          className="tv-follow"
          onClick={() => {
            setFollow(true);
            autoScrolling.current = true;
            window.setTimeout(() => {
              autoScrolling.current = false;
            }, 900);
            surfaceRef.current?.querySelector(`[data-row="${rows[playingRow]?.id}"]`)?.scrollIntoView({ block: 'center', behavior: 'smooth' });
          }}
        >
          <Volume2 className="tv-follow-icon" aria-hidden="true" />
          До місця, що звучить
        </button>
      )}
      <div className="tv-surface custom-scrollbar" ref={surfaceRef} onMouseUp={onSurfaceMouseUp} onClick={onSurfaceClick}>
        {rows.map((r, i) => (
          <Row
            key={r.id}
            id={r.id}
            text={r.text}
            time={r.start}
            speaker={r.speaker}
            showName={i === 0 || rows[i - 1].speaker !== r.speaker}
            color={colorOf(r.speaker)}
            reviewed={r.reviewed}
            manual={r.manual}
            voiceNote={voiceRows.has(r.id) && qItem?.rowId !== r.id}
            maybe={maybeOf.get(r.id)}
            onMaybe={onMaybe}
            doubtful={doubtMap.get(r.id)}
            autoWords={autoWords}
            onCommit={commitLine}
            onNameClick={onNameClick}
            onCopySpeaker={onCopySpeaker}
            onUndoSpeaker={undoSpeaker}
            registerDirty={registerDirty}
            onTime={seekRow}
          />
        ))}
        {rows.length === 0 && <p className="py-10 text-center text-[color:var(--muted-text)]">Стенограма порожня</p>}
      </div>
      </div>

      <Timeline
        meetingId={meetingId}
        lines={timelineLines}
        colorOf={colorOf}
        duration={Math.max(player.duration, rows[rows.length - 1]?.end ?? 0)}
        current={player.current}
        playing={player.playing}
        rate={player.rate}
        band={band}
        onToggle={() => {
          setFollow(true);
          player.onToggle();
        }}
        onSeek={(t) => {
          setFollow(true);
          player.onSeek(t);
        }}
        onSkip={player.onSkip}
        onRate={player.onRate}
        onChooseRange={chooseRange}
        onMoveBorder={(l, r, t) => void moveBorder(l, r, t)}
        playingId={rows[playingRow]?.id}
        onHoverLine={(id) => {
          surfaceRef.current?.querySelectorAll('.tv-row-hover').forEach((x) => x.classList.remove('tv-row-hover'));
          if (id) surfaceRef.current?.querySelector(`[data-row="${id}"]`)?.classList.add('tv-row-hover');
        }}
        onScrub={(t) => {
          setFollow(true);
          player.onSeek(t);
          // The text goes with it
          let k = 0;
          rows.forEach((r, i) => {
            if (r.start <= t) k = i;
          });
          const el = surfaceRef.current?.querySelector(`[data-row="${rows[k]?.id}"]`);
          if (el) {
            autoScrolling.current = true;
            el.scrollIntoView({ block: 'center' });
            window.setTimeout(() => {
              autoScrolling.current = false;
            }, 120);
          }
        }}
        onLineClick={(id) => {
          const r = rows[rowIndex.get(id) ?? -1];
          if (!r) return;
          setFollow(true);
          autoScrolling.current = true;
          window.setTimeout(() => {
            autoScrolling.current = false;
          }, 900);
          player.onSeek(r.start);
          surfaceRef.current?.querySelector(`[data-row="${id}"]`)?.scrollIntoView({ block: 'center', behavior: 'smooth' });
        }}
      />

      {chosen && candidates.length > 0 && (
        <div
          ref={barRef}
          className="tv-assign"
          role="toolbar"
          aria-label="Хто це сказав"
          style={{ top: barPlace?.top ?? Math.max(8, chosen.rect.top - 48), left: barPlace?.left ?? 8, maxWidth: barPlace?.maxWidth, visibility: barPlace ? undefined : 'hidden' }}
          onMouseDown={(e) => e.preventDefault()}
        >
          <span className="tv-assign-q">Хто це сказав?</span>
          {candidates.map((c, k) => (
            <button key={c.name} type="button" onClick={() => void assign(chosen, c.name)}>
              <kbd>⌘{k + 1}</kbd>
              <span className="tv-dot" style={{ background: colorOf(c.name) }} />
              {c.name}
              {c.likely && <span className="tv-likely">схоже</span>}
            </button>
          ))}
          <button type="button" className="tv-other" onClick={() => setPicker({ id: chosen.from.rowId, anchor: chosen.rect, chosen })}>
            Інша людина…
          </button>
        </div>
      )}

      {autoPop && (
        <div
          className="tv-auto-pop"
          role="dialog"
          aria-label="Виправлено за вашим правилом"
          style={{ top: autoPop.rect.bottom + 8, left: Math.max(8, Math.min(autoPop.rect.left - 8, window.innerWidth - 300)) }}
          onMouseDown={(e) => e.stopPropagation()}
        >
          <b>Виправлено за вашим правилом</b>
          <span>
            Почуто «{autoPop.fix.from}» → записано «{autoPop.fix.to}».
            {autoPop.fix.learnedAt ? ` У пам'яті з ${new Date(autoPop.fix.learnedAt).toLocaleDateString('uk-UA', { day: '2-digit', month: '2-digit' })}.` : ''}
          </span>
          <div>
            <button type="button" onClick={() => void revertAuto()}>
              Повернути як було
            </button>
            <button type="button" onClick={() => router.push('/templates?tab=memory')}>
              Усі правила у Словнику
            </button>
          </div>
        </div>
      )}

      {learned.length > 0 && (
        <div className="tv-learned" role="status" aria-live="polite">
          <span className="tv-learned-check" aria-hidden="true">✓</span>
          <span className="tv-learned-text">
            {learned.length === 1 && learned[0].kind === 'voice' ? (
              <b>Голос додано в пам&apos;ять: {learned[0].to}</b>
            ) : learned.length === 1 ? (
              <>
                <b>
                  Додано в пам&apos;ять: «{learned[0].from}» → «{learned[0].to}»
                </b>
                <span>{moreFixed > 0 ? `У цьому записі виправлено ще ${moreFixed} ${plural(moreFixed, 'рядок', 'рядки', 'рядків')}` : 'Наступного разу виправиться автоматично'}</span>
              </>
            ) : (
              <>
                <b>
                  Додано в пам&apos;ять: {learned.length} {plural(learned.length, 'виправлення', 'виправлення', 'виправлень')}
                </b>
                <span>{moreFixed > 0 ? `У цьому записі виправлено ще ${moreFixed} ${plural(moreFixed, 'рядок', 'рядки', 'рядків')}` : 'Наступного разу виправляться автоматично'}</span>
              </>
            )}
          </span>
          <button type="button" onClick={() => void undoLearned()}>
            Скасувати
          </button>
        </div>
      )}

      {strip && (
        <div className="tv-strip" role="dialog" aria-label="Непевне слово" style={{ top: stripTop(strip.rect, strip.options.length), left: Math.max(8, Math.min(strip.rect.left - 8, window.innerWidth - 320)) }}>
          <div className="tv-strip-head">
            <span className="tv-strip-what">
              <button
                type="button"
                className="tv-strip-play"
                title="Прослухати слово · ⌥Пробіл"
                aria-label="Прослухати слово"
                onClick={() => {
                  if (strip.item.doubt) playWord(strip.item.rowId, strip.item.doubt.wordIndex);
                }}
              >
                ▶
              </button>
              Розпізнано: «{strip.item.doubt?.heard ?? strip.item.doubt?.word}» · програма не впевнена
            </span>
            <button type="button" className="tv-strip-close" aria-label="Закрити" title="Закрити · Esc" onClick={() => setStrip(null)}>
              ✕
            </button>
          </div>
          <label className="tv-strip-field">
            <span>Правильно:</span>
            <input
              ref={stripInput}
              value={strip.typed}
              spellCheck={false}
              aria-label="Правильне слово"
              onChange={(e) => setStrip({ ...strip, typed: e.target.value })}
            />
          </label>
          {strip.options.map((o, k) => (
            <button key={o.text} type="button" onClick={() => void resolveWord(o.text)}>
              <span>{o.text}</span>
              <span className="tv-strip-why">
                {o.why} · <kbd>⌘{k + 1}</kbd>
              </span>
            </button>
          ))}
          <div className="tv-strip-actions">
            <button type="button" className={stripFix() === null ? 'is-default' : undefined} onClick={() => void resolveWord(null)}>
              Залишити як є
            </button>
            <button type="button" className={stripFix() !== null ? 'is-default' : undefined} disabled={stripFix() === null} onClick={submitStrip}>
              Виправити
            </button>
          </div>
          <label className="tv-strip-remember">
            <input type="checkbox" checked={strip.remember} onChange={(e) => setStrip({ ...strip, remember: e.target.checked })} />
            Запам&apos;ятати для наступних записів
          </label>
          <span className="tv-strip-keys">Enter — синя кнопка · Esc — закрити · Tab — наступне · ⌥Пробіл — прослухати</span>
        </div>
      )}

      {picker && (
        <SpeakerPicker
          meetingId={meetingId}
          current={picker.chosen ? undefined : picker.current}
          anchor={picker.anchor}
          wordsOnly={!!picker.chosen}
          onPick={(name, scope) => void changeLineSpeaker(name, scope)}
          onClose={() => setPicker(null)}
        />
      )}
    </div>
  );
}
