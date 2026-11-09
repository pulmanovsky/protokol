"use client";

import { Transcript, TranscriptSegmentData } from '@/types';
import { TranscriptView } from '@/components/TranscriptView';
import { VirtualizedTranscriptView, LineTools, WordTime } from '@/components/VirtualizedTranscriptView';
import { TranscriptButtonGroup } from './TranscriptButtonGroup';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { DoubtReview, Doubt } from './DoubtReview';
import { PlayerBar } from './PlayerBar';
import { SpeakerPicker, SpeakerScope } from './SpeakerPicker';
import { ReviewInspector, QueueItem, Person } from './ReviewInspector';
import { speakerColorMap, isUnnamed, SPEAKER_COLORS } from '@/lib/speakerColors';
import { OPEN_SPEAKERS_EVENT } from '@/lib/timeline';
import { Search } from 'lucide-react';
import type { VoiceDoubt } from '@/components/VirtualizedTranscriptView';
import { segmentIndexAt, markLineChanged, playFromWord, playRange } from '@/lib/timeline';
import { invoke } from '@tauri-apps/api/core';
import { toast } from '@/lib/notice';

interface CorrectionPair {
  wrong: string;
  right: string;
  /** Made for the second time: remembered without asking */
  remembered?: boolean;
}

interface TranscriptPanelProps {
  transcripts: Transcript[];
  onCopyTranscript: () => void;
  onOpenMeetingFolder: () => Promise<void>;
  isRecording: boolean;
  disableAutoScroll?: boolean;

  // Optional pagination props (when using virtualization)
  usePagination?: boolean;
  segments?: TranscriptSegmentData[];
  hasMore?: boolean;
  isLoadingMore?: boolean;
  totalCount?: number;
  loadedCount?: number;
  onLoadMore?: () => void;

  // Retranscription props
  meetingId?: string;
  meetingFolderPath?: string | null;
  onRefetchTranscripts?: () => Promise<void>;
  /** Shows a moment in the transcript and plays the recording from it */
  onJumpTo?: (seconds: number) => void;
  /** Speaker recognition running in the background (percent), or null */
  speakerProgress?: number | null;
  speakerMinutesLeft?: number | null;
  onStopSpeakers?: () => void;
  /** The recording's player (docked under the list) */
  player?: {
    playing: boolean;
    current: number;
    duration: number;
    rate: number;
    looping?: boolean;
    onStopLoop?: () => void;
    onToggle: () => void;
    onSeek: (seconds: number) => void;
    onSkip: (delta: number) => void;
    onRate: (rate: number) => void;
  };
  /** How far the check has got (for the header) */
  onProgress?: (p: { reviewed: number; total: number; seconds_reviewed: number; seconds_total: number } | null) => void;
}

export function TranscriptPanel({
  transcripts,
  onCopyTranscript,
  onOpenMeetingFolder,
  isRecording,
  disableAutoScroll = false,
  usePagination = false,
  segments,
  hasMore,
  isLoadingMore,
  totalCount,
  loadedCount,
  onLoadMore,
  meetingId,
  meetingFolderPath,
  onRefetchTranscripts,
  onJumpTo,
  speakerProgress = null,
  speakerMinutesLeft = null,
  onStopSpeakers,
  player,
  onProgress,
}: TranscriptPanelProps) {
  // Convert transcripts to segments if pagination is not used but we want virtualization
  const convertedSegments = useMemo(() => {
    if (usePagination && segments) {
      // While speakers are being recognised, lines without one show «…»
      if (speakerProgress === null) return segments;
      return segments.map((s) => (/^[^:]{1,40}: /.test(s.text) ? s : { ...s, text: `…: ${s.text}` }));
    }
    // Convert transcripts to segments for virtualization
    return transcripts.map(t => ({
      id: t.id,
      timestamp: t.audio_start_time ?? 0,
      endTime: t.audio_end_time,
      text: t.speaker ? `${t.speaker}: ${t.text}` : speakerProgress !== null ? `…: ${t.text}` : t.text,
      confidence: t.confidence,
      edited: t.edited,
      speakerManual: t.speaker_manual,
      reviewed: t.reviewed,
    }));
  }, [transcripts, usePagination, segments, speakerProgress]);

  // Who is speaking at the player's position, and the turns for the bar
  const speakerAt = useMemo(() => {
    if (!player || (player.current <= 0 && !player.playing)) return undefined;
    const i = segmentIndexAt(convertedSegments, player.current);
    const m = convertedSegments[i]?.text.match(/^([^:\n]{1,40}): /);
    return m ? m[1] : undefined;
  }, [player?.current, player?.playing, convertedSegments]);
  const colorsForTurns = useMemo(() => speakerColorMap(convertedSegments.map((s) => s.text.match(/^([^:\n]{1,40}): /)?.[1])), [convertedSegments]);
  const turnTints = useMemo(() => {
    const ids = new Map<string, number>();
    const out: [number, number, number][] = [];
    convertedSegments.forEach((seg, i) => {
      const who = seg.text.match(/^([^:\n]{1,40}): /)?.[1];
      if (!who) return;
      if (!ids.has(who)) ids.set(who, Math.max(0, SPEAKER_COLORS.indexOf(colorsForTurns.get(who) ?? '')));
      const end = seg.endTime ?? convertedSegments[i + 1]?.timestamp ?? seg.timestamp + 5;
      out.push([seg.timestamp, end, ids.get(who)!]);
    });
    return out;
  }, [convertedSegments, colorsForTurns]);

  // «Перевірте»: words recognition was not sure of
  const [doubts, setDoubts] = useState<Doubt[]>([]);
  const [reviewOpen, setReviewOpen] = useState(false);
  const loadDoubts = useCallback(() => {
    if (!meetingId) return;
    invoke<Doubt[]>('api_get_doubts', { meetingId }).then(setDoubts).catch(() => setDoubts([]));
  }, [meetingId]);
  useEffect(loadDoubts, [loadDoubts, transcripts, segments]);
  const doubtMap = useMemo(() => {
    const m = new Map<string, Set<number>>();
    for (const d of doubts) {
      if (!m.has(d.segmentId)) m.set(d.segmentId, new Set());
      m.get(d.segmentId)!.add(d.wordIndex);
    }
    return m;
  }, [doubts]);
  const lineText = useCallback(
    (id: string) => {
      const seg = convertedSegments.find((x) => x.id === id);
      if (!seg) return undefined;
      const m = seg.text.match(/^([^:\n]{1,40}): ([\s\S]*)$/);
      return m ? m[2] : seg.text;
    },
    [convertedSegments],
  );
  const acceptDoubt = async (d: Doubt) => {
    if (!meetingId) return;
    await invoke('api_resolve_doubt', { meetingId, seconds: d.seconds, word: d.word }).catch(() => {});
    setDoubts((all) => all.filter((x) => !(x.segmentId === d.segmentId && x.wordIndex === d.wordIndex)));
  };
  const correctDoubt = async (d: Doubt, word: string) => {
    const text = lineText(d.segmentId);
    if (!text) return;
    const words = text.split(/\s+/).filter(Boolean);
    words[d.wordIndex] = word;
    await editLine(d.segmentId, words.join(' '));
    loadDoubts();
  };

  // A line corrected by hand: saved, then «Запам'ятати?» for what was misheard
  const editLine = async (id: string, text: string) => {
    if (!meetingId) return;
    try {
      let pairs = await invoke<CorrectionPair[]>('api_edit_transcript_line', { meetingId, segmentId: id, text });
      await onRefetchTranscripts?.();
      markLineChanged(id);
      const remembered = pairs.filter((p) => p.remembered);
      if (remembered.length > 0) {
        const changed = await invoke<number>('api_apply_corrections_to_meeting', { meetingId }).catch(() => 0);
        await onRefetchTranscripts?.();
        toast.success(`Додано в пам'ять: ${remembered.map((p) => `«${p.wrong}» → «${p.right}»`).join(', ')}`, {
          description: `Виправлено вдруге — далі виправлятиметься автоматично${changed > 0 ? `; у цьому записі ще рядків: ${changed}` : ''}. Вимкнути можна у «Словнику».`,
        });
      }
      pairs = pairs.filter((p) => !p.remembered);
      if (pairs.length === 0) {
        if (remembered.length === 0) toast.success('Виправлено');
        return;
      }
      const list = pairs.map((p) => `«${p.wrong}» → «${p.right}»`).join(', ');
      toast(`Запам'ятати: ${list}?`, {
        description: 'Так — це виправлятиметься автоматично у нових записах і в решті цього запису.',
        duration: 15000,
        action: {
          label: 'Так',
          onClick: async () => {
            try {
              await invoke('api_remember_corrections', { pairs, example: text, meetingId });
              const changed = await invoke<number>('api_apply_corrections_to_meeting', { meetingId });
              await onRefetchTranscripts?.();
              toast.success(changed > 0 ? `Додано в пам'ять. У цьому записі виправлено ще рядків: ${changed}` : 'Додано в пам\'ять');
            } catch (e) {
              toast.error(`Не вдалося запам'ятати: ${e}`);
            }
          },
        },
        cancel: { label: 'Лише тут', onClick: () => {} },
      });
    } catch (e) {
      toast.error(`Не вдалося зберегти: ${e}`);
      throw e;
    }
  };
  // «Змінити спікера» of a line
  const [picker, setPicker] = useState<{ id: string; current?: string; anchor: DOMRect; words?: { from: number; to: number } } | null>(null);
  const changeSpeaker = async (name: string, scope: SpeakerScope) => {
    if (!picker) return;
    const { id, current, words } = picker;
    setPicker(null);
    await changeSpeakerFor(id, current, name, scope, words);
  };
  const changeSpeakerFor = async (id: string, current: string | undefined, name: string, scope: SpeakerScope = 'line', words?: { from: number; to: number }) => {
    if (!meetingId) return;
    if (current === name && !words) return;
    try {
      // «Це сказав…» for the chosen words: they become a line of their own
      if (words) {
        const line = await invoke<string>('assign_words_speaker', { meetingId, segmentId: id, from: words.from, to: words.to, speaker: name });
        await onRefetchTranscripts?.();
        pushUndo('«Це сказав…»', async () => {
          await invoke('undo_line_speaker', { meetingId, segmentId: line });
          if (line !== id) await invoke('merge_segments', { meetingId, segmentId: id });
          const next = convertedSegments[convertedSegments.findIndex((x) => x.id === id) + 1];
          if (next && words.to < (lineWords.get(id)?.length ?? 0)) await invoke('merge_segments', { meetingId, segmentId: id }).catch(() => {});
        });
        markLineChanged(line);
        setActiveId(line);
        toast.success(`Слова тепер — ${name}`, { description: 'Рядок розрізано. Об\'єднати знову — «Об\'єднати з наступним».' });
        return;
      }
      if (scope === 'all') {
        if (!current) return;
        await invoke('rename_speaker', { meetingId, current, name });
        await onRefetchTranscripts?.();
        markLineChanged(id);
        toast.success(`Усі рядки «${current}» тепер — ${name}`);
        return;
      }
      const done = await invoke<{ lines: string[] }>('reassign_line_speaker', { meetingId, segmentId: id, speaker: name, scope });
      await onRefetchTranscripts?.();
      pushUndo('зміну спікера', async () => {
        for (const line of done.lines) await invoke('undo_line_speaker', { meetingId, segmentId: line });
      });
      done.lines.forEach(markLineChanged);
      toast.success(done.lines.length > 1 ? `${done.lines.length} рядки тепер — ${name}` : `Рядок тепер — ${name}`, {
        description: 'Зміна збережеться і після нового розпізнавання спікерів.',
        duration: 8000,
        action: {
          label: 'Повернути',
          onClick: async () => {
            for (const line of done.lines) await invoke('undo_line_speaker', { meetingId, segmentId: line }).catch(() => {});
            await onRefetchTranscripts?.();
          },
        },
      });
    } catch (e) {
      toast.error(`Не вдалося змінити спікера: ${e}`);
    }
  };
  const undoSpeaker = async (id: string) => {
    if (!meetingId) return;
    try {
      // The run of lines given to this person together is put back together
      const i = convertedSegments.findIndex((x) => x.id === id);
      const who = (t: string) => t.match(/^([^:\n]{1,40}): /)?.[1];
      const ids: string[] = [];
      for (let j = i; j >= 0 && j < convertedSegments.length; j++) {
        const seg = convertedSegments[j];
        if (!seg.speakerManual || who(seg.text) !== who(convertedSegments[i].text)) break;
        ids.push(seg.id);
      }
      for (const line of ids.length ? ids : [id]) await invoke('undo_line_speaker', { meetingId, segmentId: line });
      await onRefetchTranscripts?.();
      markLineChanged(id);
    } catch (e) {
      toast.error(`${e}`);
    }
  };

  // Words of each line with times: a click plays from a word, the word
  // being heard is lit
  const [lineWords, setLineWords] = useState<Map<string, WordTime[]>>(new Map());
  useEffect(() => {
    if (!meetingId || isRecording) return;
    invoke<{ id: string; exact: boolean; words: WordTime[] }[]>('get_line_words', { meetingId })
      .then((all) => setLineWords(new Map(all.map((l) => [l.id, l.words]))))
      .catch(() => setLineWords(new Map()));
  }, [meetingId, isRecording, transcripts, segments]);

  // «Перевірено»: lines the secretary has gone through, and how far they got
  const [progress, setProgress] = useState<{ reviewed: number; total: number; seconds_reviewed: number; seconds_total: number } | null>(null);
  useEffect(() => {
    if (!meetingId || isRecording) return;
    invoke<typeof progress>('get_review_progress', { meetingId }).then(setProgress).catch(() => setProgress(null));
  }, [meetingId, isRecording, transcripts, segments]);
  useEffect(() => {
    onProgress?.(progress);
    window.dispatchEvent(new CustomEvent('protokol:review-changed'));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [progress]);

  // ⌘Z: the last change made here is put back
  const undoStack = useRef<{ label: string; run: () => Promise<void> }[]>([]);
  const pushUndo = (label: string, run: () => Promise<void>) => {
    undoStack.current.push({ label, run });
    if (undoStack.current.length > 30) undoStack.current.shift();
  };
  const undoLast = async () => {
    const last = undoStack.current.pop();
    if (!last) return toast('Нічого скасовувати');
    try {
      await last.run();
      await onRefetchTranscripts?.();
      toast.success(`Скасовано: ${last.label}`);
    } catch (e) {
      toast.error(`${e}`);
    }
  };

  // The line in focus: keys act on it
  const [activeId, setActiveId] = useState<string | null>(null);
  const [editRequest, setEditRequest] = useState<{ id: string; n: number } | undefined>(undefined);
  const segById = (id: string) => convertedSegments.find((s) => s.id === id);
  const lineEnd = (i: number) => convertedSegments[i].endTime ?? convertedSegments[i + 1]?.timestamp ?? convertedSegments[i].timestamp + 5;
  const toggleReviewed = async (id: string, next = true) => {
    if (!meetingId) return;
    const seg = segById(id);
    try {
      const p = await invoke<typeof progress>('set_segments_reviewed', { meetingId, segmentIds: [id], reviewed: !seg?.reviewed });
      setProgress(p);
      pushUndo('позначку «перевірено»', async () => {
        setProgress(await invoke<typeof progress>('set_segments_reviewed', { meetingId, segmentIds: [id], reviewed: !!seg?.reviewed }));
      });
      await onRefetchTranscripts?.();
      // Checked: on to the next line
      if (next && !seg?.reviewed) {
        const i = convertedSegments.findIndex((s) => s.id === id);
        const after = convertedSegments[i + 1];
        if (after) setActiveId(after.id);
      }
    } catch (e) {
      toast.error(`${e}`);
    }
  };
  const mergeWithNext = async (id: string) => {
    if (!meetingId) return;
    const seg = segById(id);
    const firstWords = (seg ? (seg.text.match(/^([^:\n]{1,40}): ([\s\S]*)$/)?.[2] ?? seg.text) : '').split(/\s+/).filter(Boolean).length;
    try {
      await invoke('merge_segments', { meetingId, segmentId: id });
      await onRefetchTranscripts?.();
      markLineChanged(id);
      pushUndo('об\'єднання рядків', async () => {
        await invoke('split_segment', { meetingId, segmentId: id, wordIndex: firstWords });
      });
      toast.success('Рядки об\'єднано', {
        duration: 8000,
        action: {
          label: 'Повернути',
          onClick: async () => {
            await invoke('split_segment', { meetingId, segmentId: id, wordIndex: firstWords }).catch((e) => toast.error(`${e}`));
            await onRefetchTranscripts?.();
          },
        },
      });
    } catch (e) {
      toast.error(`${e}`);
    }
  };
  const splitAt = async (id: string, wordIndex: number) => {
    if (!meetingId) return;
    try {
      const second = await invoke<string>('split_segment', { meetingId, segmentId: id, wordIndex });
      await onRefetchTranscripts?.();
      pushUndo('розрізання рядка', async () => {
        await invoke('merge_segments', { meetingId, segmentId: id });
      });
      markLineChanged(second);
      setActiveId(second);
      toast.success('Рядок розрізано', {
        duration: 8000,
        action: {
          label: 'Повернути',
          onClick: async () => {
            await invoke('merge_segments', { meetingId, segmentId: id }).catch((e) => toast.error(`${e}`));
            await onRefetchTranscripts?.();
          },
        },
      });
    } catch (e) {
      toast.error(`${e}`);
    }
  };
  const tools: LineTools | undefined =
    isRecording || speakerProgress !== null || !meetingId
      ? undefined
      : {
          words: lineWords,
          activeId,
          onActivate: setActiveId,
          onWord: (id, index) => {
            const w = lineWords.get(id)?.[index];
            if (w) playFromWord(w.start);
          },
          onPlayLine: (id, loop) => {
            const i = convertedSegments.findIndex((s) => s.id === id);
            if (i >= 0) playRange(convertedSegments[i].timestamp, lineEnd(i), loop);
          },
          onToggleReviewed: (id) => void toggleReviewed(id),
          onMerge: (id) => void mergeWithNext(id),
          editRequest,
        };

  // Words chosen with the mouse in one line: cut there, or «Це сказав…»
  const [chosen, setChosen] = useState<{ id: string; from: number; to: number; count: number; rect: DOMRect; likely?: string } | null>(null);

  // Places where the voice seems to change inside a line (looked for once
  // the transcript is ready; a few seconds)
  const [voiceDoubts, setVoiceDoubts] = useState<VoiceDoubt[]>([]);
  const voiceKey = `${meetingId}:${convertedSegments.length}:${speakerProgress === null}`;
  useEffect(() => {
    if (!meetingId || isRecording || speakerProgress !== null || convertedSegments.length === 0) return;
    invoke<typeof voiceDoubts>('find_voice_changes', { meetingId })
      .then((found) => {
        // Neighbouring sentences of one line that sound like the same person: one place
        const merged: VoiceDoubt[] = [];
        for (const d of [...found].sort((a, b) => a.seconds - b.seconds)) {
          const last = merged[merged.length - 1];
          if (last && last.segment_id === d.segment_id && last.to === d.from && last.likely === d.likely) {
            merged[merged.length - 1] = { ...last, to: d.to, similarity: Math.max(last.similarity, d.similarity) };
          } else if (!merged.some((m) => m.segment_id === d.segment_id)) {
            merged.push(d);
          }
        }
        setVoiceDoubts(merged);
      })
      .catch(() => setVoiceDoubts([]));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [voiceKey]);
  const [voiceAt, setVoiceAt] = useState(0);
  const showVoiceDoubt = (k: number) => {
    const d = voiceDoubts[k];
    if (!d) return;
    setVoiceAt(k);
    setActiveId(d.segment_id);
    // Once the line is in view: its words chosen, with the likely person
    window.setTimeout(() => {
      const first = document.querySelector(`[data-seg="${d.segment_id}"] [data-w="${d.from}"]`);
      const last = document.querySelector(`[data-seg="${d.segment_id}"] [data-w="${d.to - 1}"]`);
      if (!first || !last) return;
      const range = document.createRange();
      range.setStartBefore(first);
      range.setEndAfter(last);
      const sel = window.getSelection();
      sel?.removeAllRanges();
      sel?.addRange(range);
      setChosen({ id: d.segment_id, from: d.from, to: d.to, count: lineWords.get(d.segment_id)?.length ?? 0, rect: range.getBoundingClientRect(), likely: d.likely });
      playRange(d.seconds, lineWords.get(d.segment_id)?.[d.to - 1]?.end ?? d.seconds + 4);
    }, 250);
  };
  const dropVoiceDoubt = (id: string, from: number) => setVoiceDoubts((all) => all.filter((d) => !(d.segment_id === id && d.from === from)));

  // What is shown: search and «Лише неперевірені»
  const [search, setSearch] = useState('');
  const [onlyUnchecked, setOnlyUnchecked] = useState(false);
  const visibleSegments = useMemo(() => {
    const q = search.trim().toLowerCase();
    if (!q && !onlyUnchecked) return convertedSegments;
    return convertedSegments.filter((s) => (!onlyUnchecked || !s.reviewed) && (!q || s.text.toLowerCase().includes(q)));
  }, [convertedSegments, search, onlyUnchecked]);

  // Speakers: colour, lines, time (in the order they first speak)
  const whoOf = (text: string) => text.match(/^([^:\n]{1,40}): /)?.[1];
  const colors = useMemo(() => speakerColorMap(convertedSegments.map((s) => whoOf(s.text))), [convertedSegments]);
  const people: Person[] = useMemo(() => {
    const by = new Map<string, Person>();
    convertedSegments.forEach((s, i) => {
      const who = whoOf(s.text);
      if (!who || who === '…' || who === 'Репліки') return;
      const p = by.get(who) ?? { name: who, color: colors.get(who) ?? '#888', lines: 0, seconds: 0, unnamed: isUnnamed(who) };
      p.lines += 1;
      p.seconds += Math.max(0, (s.endTime ?? convertedSegments[i + 1]?.timestamp ?? s.timestamp) - s.timestamp);
      by.set(who, p);
    });
    return [...by.values()];
  }, [convertedSegments, colors]);

  // «Перевірте»: voice changes inside lines, voices without a name, unsure words
  const queue: QueueItem[] = useMemo(() => {
    const items: QueueItem[] = [];
    const words = (id: string, from: number, to: number) => (lineWords.get(id) ?? []).slice(from, to).map((w) => w.word).join(' ');
    for (const d of voiceDoubts) {
      items.push({ key: `v:${d.segment_id}:${d.from}`, kind: 'voice', segmentId: d.segment_id, seconds: d.seconds, title: 'Голос змінюється посеред рядка', description: `Схоже, ці слова сказав ${d.likely} (схожість ${Math.round(Math.max(0, d.similarity) * 100)}%)`, text: words(d.segment_id, d.from, d.to), primary: `Розрізати і віддати ${d.likely}` });
    }
    for (const p of people.filter((x) => x.unnamed)) {
      const seg = convertedSegments.find((s) => whoOf(s.text) === p.name);
      if (seg) items.push({ key: `n:${p.name}`, kind: 'name', segmentId: seg.id, seconds: seg.timestamp, title: `${p.name} без імені`, description: `${p.lines} рядк. Послухайте і назвіть людину — так буде в усіх її рядках.`, text: (seg.text.match(/^[^:\n]{1,40}: ([\s\S]*)$/)?.[1] ?? seg.text).slice(0, 140), primary: 'Назвати…' });
    }
    for (const d of doubts) {
      items.push({ key: `w:${d.segmentId}:${d.wordIndex}`, kind: 'word', segmentId: d.segmentId, seconds: d.seconds, title: `«${d.word}»`, description: d.heard ? `Почуто «${d.heard}», замінено словником` : 'Розпізнавання не певне — послухайте', text: (lineText(d.segmentId) ?? '').split(/\s+/).slice(Math.max(0, d.wordIndex - 6), d.wordIndex + 7).join(' '), primary: 'Виправити' });
    }
    return items.sort((x, y) => x.seconds - y.seconds);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [voiceDoubts, people, doubts, lineWords, convertedSegments]);
  const voiceMap = useMemo(() => new Map<string, VoiceDoubt>(voiceDoubts.map((d) => [d.segment_id, d])), [voiceDoubts]);
  const acceptVoice = async (d: VoiceDoubt) => {
    if (!meetingId) return;
    dropVoiceDoubt(d.segment_id, d.from);
    try {
      const line = await invoke<string>('assign_words_speaker', { meetingId, segmentId: d.segment_id, from: d.from, to: d.to, speaker: d.likely });
      await onRefetchTranscripts?.();
      markLineChanged(line);
      setActiveId(line);
      toast.success(`Розрізано: ці слова — ${d.likely}`);
    } catch (e) {
      toast.error(`${e}`);
    }
  };
  const goTo = (item: QueueItem) => {
    if (search || onlyUnchecked) {
      setSearch('');
      setOnlyUnchecked(false);
    }
    setActiveId(item.segmentId);
    if (item.kind === 'voice') {
      const d = voiceDoubts.find((v) => `v:${v.segment_id}:${v.from}` === item.key);
      const end = d ? lineWords.get(d.segment_id)?.[d.to - 1]?.end : undefined;
      playRange(item.seconds, end ?? item.seconds + 4);
    } else {
      playFromWord(Math.max(0, item.seconds - (item.kind === 'word' ? 1 : 0)));
    }
  };
  const onQueuePrimary = (item: QueueItem) => {
    if (item.kind === 'voice') {
      const d = voiceMap.get(item.segmentId);
      if (d) void acceptVoice(d);
    } else if (item.kind === 'name') {
      const el = document.querySelector(`[data-seg="${item.segmentId}"]`);
      setActiveId(item.segmentId);
      setPicker({ id: item.segmentId, current: item.title.replace(/ без імені$/, ''), anchor: (el ?? document.body).getBoundingClientRect() });
    } else {
      setActiveId(item.segmentId);
      setEditRequest((r) => ({ id: item.segmentId, n: (r?.n ?? 0) + 1 }));
    }
  };
  const onQueueDismiss = (item: QueueItem) => {
    if (item.kind === 'voice') {
      const d = voiceMap.get(item.segmentId);
      if (d) dropVoiceDoubt(d.segment_id, d.from);
    } else if (item.kind === 'word') {
      const d = doubts.find((x) => `w:${x.segmentId}:${x.wordIndex}` === item.key);
      if (d) void acceptDoubt(d);
    }
  };

  // The word being heard in the line being played (for X and the lit word)
  const playingIndex = player && (player.playing || player.current > 0) ? segmentIndexAt(convertedSegments, player.current) : -1;
  const playingId = playingIndex >= 0 ? convertedSegments[playingIndex]?.id : undefined;
  const currentWordOf = (id: string) => {
    const ws = lineWords.get(id);
    if (!ws || !player) return -1;
    let k = -1;
    ws.forEach((w, i) => {
      if (w.start <= player.current) k = i;
    });
    return k;
  };
  const [loopingId, setLoopingId] = useState<string | undefined>(undefined);
  useEffect(() => {
    if (!player?.looping) setLoopingId(undefined);
  }, [player?.looping]);

  // Keys while the transcript has focus (not while typing in a line):
  // ↑/↓ line, Enter text, S speaker, V checked and next, R repeat the line,
  // L hear the line, X cut at the word being heard, 1–9 a person, ⌘Z undo.
  // Space and ←/→ are the player's (see the page).
  const listRef = useRef<HTMLDivElement>(null);
  const onListKey = (e: React.KeyboardEvent) => {
    const t = e.target as HTMLElement;
    if (!tools || t.closest('input, textarea, select, [contenteditable="true"], [role="dialog"]')) return;
    if ((e.metaKey || e.ctrlKey) && e.code === 'KeyZ') {
      e.preventDefault();
      void undoLast();
      return;
    }
    if (e.metaKey || e.ctrlKey || e.altKey) return;
    if (e.key === 'Enter' && t.closest('button')) return;
    const list = visibleSegments;
    const i = activeId ? list.findIndex((s) => s.id === activeId) : -1;
    const go = (k: number) => {
      const seg = list[Math.max(0, Math.min(list.length - 1, k))];
      if (!seg) return;
      setActiveId(seg.id);
      if (player?.playing) playFromWord(seg.timestamp);
    };
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      go(i + 1);
      return;
    }
    if (e.key === 'ArrowUp') {
      e.preventDefault();
      go(i < 0 ? 0 : i - 1);
      return;
    }
    if (i < 0) return;
    const seg = list[i];
    const k = convertedSegments.findIndex((s) => s.id === seg.id);
    if (e.key === 'Enter') {
      e.preventDefault();
      setEditRequest((r) => ({ id: seg.id, n: (r?.n ?? 0) + 1 }));
    } else if (e.code === 'KeyS') {
      e.preventDefault();
      const el = document.querySelector(`[data-seg="${seg.id}"]`);
      if (el) setPicker({ id: seg.id, current: whoOf(seg.text), anchor: el.getBoundingClientRect() });
    } else if (e.code === 'KeyV') {
      e.preventDefault();
      void toggleReviewed(seg.id);
    } else if (e.code === 'KeyR') {
      e.preventDefault();
      if (loopingId === seg.id) {
        player?.onStopLoop?.();
        setLoopingId(undefined);
      } else {
        setLoopingId(seg.id);
        playRange(seg.timestamp, lineEnd(k), true);
      }
    } else if (e.code === 'KeyL') {
      e.preventDefault();
      playRange(seg.timestamp, lineEnd(k));
    } else if (e.code === 'KeyX') {
      e.preventDefault();
      const at = chosen && chosen.id === seg.id ? chosen.from : playingId === seg.id ? currentWordOf(seg.id) : -1;
      if (at > 0) void splitAt(seg.id, at);
      else toast('Увімкніть рядок і натисніть X на слові, з якого говорить інша людина (або виділіть слова)');
    } else if (/^Digit[1-9]$/.test(e.code)) {
      const p = people[Number(e.code.slice(5)) - 1];
      if (!p) return;
      e.preventDefault();
      void changeSpeakerFor(seg.id, whoOf(seg.text), p.name);
    }
  };

  const onListMouseUp = () => {
    window.setTimeout(() => {
      const sel = window.getSelection();
      if (!sel || sel.isCollapsed || !sel.anchorNode || !sel.focusNode) return setChosen(null);
      const el = (n: Node) => (n.nodeType === Node.ELEMENT_NODE ? (n as Element) : n.parentElement);
      const a = el(sel.anchorNode)?.closest('[data-w]');
      const b = el(sel.focusNode)?.closest('[data-w]');
      const seg = a?.closest('[data-seg]');
      if (!a || !b || !seg || b.closest('[data-seg]') !== seg) return setChosen(null);
      const id = seg.getAttribute('data-seg')!;
      const x = Number(a.getAttribute('data-w'));
      const y = Number(b.getAttribute('data-w'));
      setChosen({ id, from: Math.min(x, y), to: Math.max(x, y) + 1, count: lineWords.get(id)?.length ?? 0, rect: sel.getRangeAt(0).getBoundingClientRect() });
    }, 0);
  };
  useEffect(() => {
    if (!chosen) return;
    const away = (e: MouseEvent) => {
      if (!(e.target as HTMLElement).closest('.words-bar')) setChosen(null);
    };
    document.addEventListener('mousedown', away);
    return () => document.removeEventListener('mousedown', away);
  }, [chosen]);

  const undoLine = async (id: string) => {
    if (!meetingId) return;
    try {
      await invoke('api_undo_transcript_edit', { meetingId, segmentId: id });
      await onRefetchTranscripts?.();
    } catch (e) {
      toast.error(`${e}`);
    }
  };

  if (tools) {
    tools.colorOf = (who) => (who ? colors.get(who) : undefined) ?? 'var(--muted-text)';
    tools.voiceDoubts = voiceMap;
    tools.onVoiceAccept = (d) => void acceptVoice(d);
    tools.onVoiceReject = (d) => dropVoiceDoubt(d.segment_id, d.from);
    tools.playingId = playingId;
    tools.loopingId = player?.looping ? loopingId : undefined;
    const playLine = tools.onPlayLine;
    tools.onPlayLine = (id, loop) => {
      if (loop && loopingId === id && player?.looping) {
        player.onStopLoop?.();
        setLoopingId(undefined);
        return;
      }
      setLoopingId(loop ? id : undefined);
      playLine(id, loop);
    };
  }

  return (
    <div className="card flex h-full min-w-0 w-full flex-col relative overflow-hidden @container">
      {/* Toolbar: search, which lines, the keys */}
      <div className="flex items-center gap-2 px-4 pt-3 pb-2">
        <div className="flex h-8 min-w-[160px] max-w-[260px] flex-1 items-center gap-1.5 rounded-lg bg-[var(--selected)] px-2">
          <Search className="h-3.5 w-3.5 flex-none text-[color:var(--muted-text)]" />
          <input
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            placeholder="Пошук у стенограмі"
            aria-label="Пошук у стенограмі"
            className="min-w-0 flex-1 bg-transparent text-[13px] text-gray-900 outline-none placeholder:text-[color:var(--muted-text)]"
          />
        </div>
        <div className="step-switch !p-[2px]">
          <button type="button" className="!h-7 !px-3 !text-[12px]" data-on={!onlyUnchecked} onClick={() => setOnlyUnchecked(false)}>
            Усі рядки
          </button>
          <button type="button" className="!h-7 !px-3 !text-[12px]" data-on={onlyUnchecked} onClick={() => setOnlyUnchecked(true)}>
            Лише неперевірені
          </button>
        </div>
        <span className="hidden @[1100px]:flex min-w-0 flex-1 items-center gap-1 overflow-hidden whitespace-nowrap text-[11px] text-[color:var(--muted-text)]">
          <kbd>Пробіл</kbd> слухати <kbd>↑↓</kbd> рядок <kbd>V</kbd> перевірено <kbd>S</kbd> спікер <kbd>X</kbd> розрізати <kbd>1–9</kbd> людина
        </span>
        <div className="ml-auto flex flex-none items-center gap-1">
          <TranscriptButtonGroup
            transcriptCount={usePagination ? (totalCount ?? convertedSegments.length) : (transcripts?.length || 0)}
            onCopyTranscript={onCopyTranscript}
            onOpenMeetingFolder={onOpenMeetingFolder}
            meetingId={meetingId}
            meetingFolderPath={meetingFolderPath}
            onRefetchTranscripts={onRefetchTranscripts}
            onJumpTo={onJumpTo}
          />
        </div>
      </div>

      {/* One processing pipeline: text, then speakers, then ready */}
      {speakerProgress !== null && (
        <div className="mx-5 mb-3 rounded-[14px] border border-[var(--card-border)] bg-[var(--card-bg)] px-3 py-2.5 text-sm" role="status">
          <div className="flex items-center justify-between gap-2">
            <span className="font-semibold text-gray-900">Обробка запису</span>
            {onStopSpeakers && (
              <button type="button" onClick={onStopSpeakers} className="secondary-button h-8 px-3 text-xs font-semibold" title="Зупинити розпізнавання спікерів">
                Зупинити
              </button>
            )}
          </div>
          <div className="mt-1 flex flex-wrap items-center gap-x-2 text-[13px] text-gray-700">
            <span>1 Текст ✓</span>
            <span aria-hidden>→</span>
            <span className="font-semibold">
              2 Спікери {speakerProgress > 0 ? `${speakerProgress}%` : '…'}
              {speakerMinutesLeft ? ` (~${speakerMinutesLeft} хв)` : ''}
            </span>
            <span aria-hidden>→</span>
            <span className="text-[color:var(--muted-text)]">3 Готово</span>
          </div>
          <div className="mt-2 h-1 w-full overflow-hidden rounded-full bg-[var(--selected)]" aria-hidden>
            <div className="h-1 rounded-full bg-[var(--blue)] transition-all duration-500" style={{ width: `${Math.max(3, speakerProgress)}%` }} />
          </div>
          <div className="mt-1.5 text-xs text-[color:var(--muted-text)]">
            Розрізнення голосів і поділ тексту за спікерами. Можна працювати далі — текст оновиться сам.
          </div>
        </div>
      )}

      {/* Transcript content - use virtualized view for better performance */}
      <div className="flex flex-1 min-h-0">
      <div className="flex-1 min-w-0 min-h-0 overflow-hidden pb-2" ref={listRef} onKeyDown={onListKey} onMouseUp={onListMouseUp}>
        <VirtualizedTranscriptView
          tools={tools}
          segments={visibleSegments}
          isRecording={isRecording}
          isPaused={false}
          isProcessing={false}
          isStopping={false}
          enableStreaming={false}
          showConfidence={true}
          disableAutoScroll={disableAutoScroll}
          hasMore={hasMore}
          isLoadingMore={isLoadingMore}
          totalCount={totalCount}
          loadedCount={loadedCount}
          onLoadMore={onLoadMore}
          onEditLine={isRecording || speakerProgress !== null ? undefined : editLine}
          doubts={doubtMap}
          onUndoLine={undoLine}
          onChangeSpeaker={isRecording || speakerProgress !== null || !meetingId ? undefined : (id, current, anchor) => setPicker({ id, current, anchor })}
          onUndoSpeaker={undoSpeaker}
          playingSeconds={player && (player.playing || player.current > 0) ? player.current : undefined}
          followPlayback={!!player?.playing}
          compact
        />
      </div>
      {tools && !isRecording && (
        <ReviewInspector
          items={queue}
          people={people}
          onGo={goTo}
          onPrimary={onQueuePrimary}
          onDismiss={onQueueDismiss}
          onPlayPerson={(name) => {
            const seg = convertedSegments.find((s) => whoOf(s.text) === name);
            if (seg) {
              setActiveId(seg.id);
              playFromWord(seg.timestamp);
            }
          }}
          onNamePerson={(name, anchor) => {
            const seg = convertedSegments.find((s) => whoOf(s.text) === name);
            if (seg) setPicker({ id: seg.id, current: name, anchor });
          }}
          onOpenSpeakers={() => window.dispatchEvent(new CustomEvent(OPEN_SPEAKERS_EVENT))}
        />
      )}
      </div>

      {player && (
        <PlayerBar
          playing={player.playing}
          current={player.current}
          duration={player.duration}
          rate={player.rate}
          speaker={speakerAt}
          turns={turnTints}
          onRepeat={() => {
            const id = playingId ?? activeId;
            const k = convertedSegments.findIndex((s) => s.id === id);
            if (k < 0) return;
            setLoopingId(convertedSegments[k].id);
            playRange(convertedSegments[k].timestamp, lineEnd(k), true);
          }}
          line={playingIndex >= 0 ? ([convertedSegments[playingIndex].timestamp, lineEnd(playingIndex)] as [number, number]) : undefined}
          onToggle={player.onToggle}
          onSeek={player.onSeek}
          onSkip={player.onSkip}
          onRate={player.onRate}
          looping={player.looping}
          onStopLoop={player.onStopLoop}
        />
      )}

      {picker && meetingId && (
        <SpeakerPicker meetingId={meetingId} current={picker.current} anchor={picker.anchor} onPick={changeSpeaker} onClose={() => setPicker(null)} wordsOnly={!!picker.words} />
      )}

      {chosen && tools && (
        <div
          className="words-bar fixed z-40 flex flex-wrap items-center gap-1 rounded-xl border border-[var(--card-border)] bg-[var(--card-bg)] p-1 text-[12px] shadow-lg"
          style={{ top: Math.min(chosen.rect.bottom + 6, window.innerHeight - 48), left: Math.max(8, Math.min(chosen.rect.left, window.innerWidth - 560)), maxWidth: 552 }}
        >
          {chosen.likely && (
            <>
              <span className="px-1.5 text-[color:var(--muted-text)]">Схоже, це {chosen.likely}</span>
              <button
                type="button"
                className="line-tool line-tool-on"
                onClick={async () => {
                  const c = chosen;
                  setChosen(null);
                  dropVoiceDoubt(c.id, c.from);
                  try {
                    const line = await invoke<string>('assign_words_speaker', { meetingId, segmentId: c.id, from: c.from, to: c.to, speaker: c.likely });
                    await onRefetchTranscripts?.();
                    markLineChanged(line);
                  } catch (e) {
                    toast.error(`${e}`);
                  }
                }}
              >
                Так
              </button>
              <button
                type="button"
                className="line-tool"
                onClick={() => {
                  dropVoiceDoubt(chosen.id, chosen.from);
                  setChosen(null);
                }}
              >
                Ні, все вірно
              </button>
            </>
          )}
          <button
            type="button"
            className="line-tool"
            onClick={() => {
              if (chosen.likely) dropVoiceDoubt(chosen.id, chosen.from);
              const seg = segById(chosen.id);
              setPicker({ id: chosen.id, current: seg?.text.match(/^([^:\n]{1,40}): /)?.[1], anchor: chosen.rect, words: { from: chosen.from, to: chosen.to } });
              setChosen(null);
            }}
          >
            Це сказав…
          </button>
          {chosen.from > 0 && (
            <button type="button" className="line-tool" onClick={() => { void splitAt(chosen.id, chosen.from); setChosen(null); }}>
              Розрізати перед виділеним
            </button>
          )}
          {chosen.to < chosen.count && (
            <button type="button" className="line-tool" onClick={() => { void splitAt(chosen.id, chosen.to); setChosen(null); }}>
              Розрізати після
            </button>
          )}
        </div>
      )}

      {reviewOpen && (
        <DoubtReview doubts={doubts} lineText={lineText} onAccept={acceptDoubt} onCorrect={correctDoubt} onClose={() => setReviewOpen(false)} />
      )}
    </div>
  );
}
