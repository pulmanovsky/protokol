'use client';

import { useCallback, useMemo, useRef, useReducer, startTransition, useEffect, useLayoutEffect, useState, memo } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { segmentIndexAt, JUMP_TO_TIME_EVENT, TIME_CLICKED_EVENT, LINE_CHANGED_EVENT } from "@/lib/timeline";
import { useAutoScroll } from "@/hooks/useAutoScroll";
import { useTranscriptStreaming } from "@/hooks/useTranscriptStreaming";
import { ConfidenceIndicator } from "./ConfidenceIndicator";
import { Tooltip, TooltipContent, TooltipTrigger } from "./ui/tooltip";
import { RecordingStatusBar } from "./RecordingStatusBar";
import { motion, AnimatePresence } from "framer-motion";
import { TranscriptSegmentData } from "@/types";
import { ArrowDown, Check, ChevronDown, Circle, Merge, Pencil, Play, Repeat, Scissors, UserRound } from "lucide-react";
import { isUnnamed } from "@/lib/speakerColors";

export interface VirtualizedTranscriptViewProps {
    /** Transcript segments to display */
    segments: TranscriptSegmentData[];
    /** Whether recording is in progress */
    isRecording?: boolean;
    /** Whether recording is paused */
    isPaused?: boolean;
    /** Whether processing/finalizing transcription */
    isProcessing?: boolean;
    /** Whether stopping */
    isStopping?: boolean;
    /** Enable streaming effect for latest segment */
    enableStreaming?: boolean;
    /** Show confidence indicators */
    showConfidence?: boolean;
    /** Completely disable auto-scroll behavior (for meeting details page) */
    disableAutoScroll?: boolean;
    /** Show the sticky recording status bar (the recording screen has its own top bar) */
    showStatusBar?: boolean;
    /** Smaller rows for the side panel next to the protocol */
    compact?: boolean;

    // Pagination props (infinite scroll)
    hasMore?: boolean;
    isLoadingMore?: boolean;
    totalCount?: number;
    loadedCount?: number;
    onLoadMore?: () => void;

    /** Saves a line corrected by hand (text without the speaker) */
    onEditLine?: (id: string, text: string) => Promise<void>;
    /** Puts back the recognised text of a corrected line */
    onUndoLine?: (id: string) => Promise<void>;
    /** Doubtful words (their places in each line), underlined */
    doubts?: Map<string, Set<number>>;
    /** Where the recording is being played: that line is highlighted */
    playingSeconds?: number;
    /** Keep the playing line in view (until the user scrolls away) */
    followPlayback?: boolean;
    /** «Змінити спікера» of a line: opens the list of people at `anchor` */
    onChangeSpeaker?: (id: string, current: string | undefined, anchor: DOMRect) => void;
    /** Puts back the recognised speaker of a line changed by hand */
    onUndoSpeaker?: (id: string) => void;
    /** Working on lines: words with times, the line in focus, checks */
    tools?: LineTools;
}

/** A word of a line with its moment in the recording */
export interface WordTime {
    word: string;
    start: number;
    end: number;
    prob: number;
}

/** What the secretary does with lines (see TranscriptPanel) */
export interface LineTools {
    /** Words of each line with times (when known) */
    words?: Map<string, WordTime[]>;
    /** The line in focus (keys ↑/↓, Enter, S, R, L) */
    activeId?: string | null;
    onActivate: (id: string) => void;
    /** A click on a word: play from it */
    onWord: (id: string, index: number) => void;
    onPlayLine: (id: string, loop: boolean) => void;
    onToggleReviewed: (id: string) => void;
    onMerge: (id: string) => void;
    /** Asks the line `id` to open its text for editing (n changes each time) */
    editRequest?: { id: string; n: number };
    /** The colour of each speaker */
    colorOf?: (speaker: string | undefined) => string;
    /** Lines where the voice seems to change: the words and the likely person */
    voiceDoubts?: Map<string, VoiceDoubt>;
    onVoiceAccept?: (d: VoiceDoubt) => void;
    onVoiceReject?: (d: VoiceDoubt) => void;
    /** The line being played */
    playingId?: string;
    /** A line played over and over */
    loopingId?: string;
}

export interface VoiceDoubt {
    segment_id: string;
    from: number;
    to: number;
    seconds: number;
    likely: string;
    similarity: number;
}

// Threshold for enabling virtualization (below this, use simple rendering)
const VIRTUALIZATION_THRESHOLD = 10;

// Helper function to format seconds as recording-relative time [MM:SS]
function formatRecordingTime(seconds: number | undefined): string {
    if (seconds === undefined) return '--:--:--';

    const totalSeconds = Math.floor(seconds);
    const hours = Math.floor(totalSeconds / 3600);
    const minutes = Math.floor((totalSeconds % 3600) / 60);
    const secs = totalSeconds % 60;

    return [hours, minutes, secs].map((v) => v.toString().padStart(2, '0')).join(':');
}

// Helper function to remove filler words and repetitions
function cleanStopWords(text: string): string {
    const stopWords = ['uh', 'um', 'er', 'ah', 'hmm', 'hm', 'eh', 'oh'];

    let cleanedText = text;
    stopWords.forEach(word => {
        const pattern = new RegExp(`\\b${word}\\b[,\\s]*`, 'gi');
        cleanedText = cleanedText.replace(pattern, ' ');
    });

    return cleanedText.replace(/\s+/g, ' ').trim();
}

// Memoized transcript segment component
const TranscriptSegment = memo(function TranscriptSegment({
    id,
    timestamp,
    text,
    confidence,
    isStreaming,
    showConfidence,
    compact,
    highlighted = false,
    playing = false,
    speaker,
    edited = false,
    onSave,
    onUndo,
    doubtful,
    who,
    speakerManual = false,
    onChangeSpeaker,
    onUndoSpeaker,
    words,
    currentWord,
    reviewed = false,
    active = false,
    tools,
    isLast = false,
}: {
    id: string;
    timestamp: number;
    text: string;
    confidence?: number;
    isStreaming: boolean;
    showConfidence: boolean;
    compact: boolean;
    highlighted?: boolean;
    playing?: boolean;
    /** Shown when the speaker changes (not on every line) */
    speaker?: string;
    edited?: boolean;
    onSave?: (id: string, text: string) => Promise<void>;
    onUndo?: (id: string) => Promise<void>;
    doubtful?: Set<number>;
    /** The line's speaker (shown or not) */
    who?: string;
    speakerManual?: boolean;
    onChangeSpeaker?: (id: string, current: string | undefined, anchor: DOMRect) => void;
    onUndoSpeaker?: (id: string) => void;
    words?: WordTime[];
    /** The word being played */
    currentWord?: number;
    reviewed?: boolean;
    active?: boolean;
    tools?: LineTools;
    isLast?: boolean;
}) {
    const label = speaker ?? (speakerManual ? who : undefined);
    const openPicker = (e: React.MouseEvent<HTMLElement>) => onChangeSpeaker?.(id, who, e.currentTarget.getBoundingClientRect());
    const displayText = cleanStopWords(text) || (text.trim() === '' ? '[тиша]' : text);
    const [editing, setEditing] = useState(false);
    const [draft, setDraft] = useState(text);
    const [saving, setSaving] = useState(false);
    const start = () => {
        if (!onSave || isStreaming) return;
        setDraft(text);
        setEditing(true);
    };
    // Enter on the line in focus opens its text
    const editN = tools?.editRequest?.id === id ? tools.editRequest.n : 0;
    useEffect(() => {
        if (editN > 0) start();
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [editN]);
    // Words with times: each can be clicked (play from it) and is lit while heard
    const plainWords = text.split(/\s+/).filter(Boolean);
    const timed = words && words.length === plainWords.length ? words : undefined;
    const voice = tools?.voiceDoubts?.get(id);
    const save = async () => {
        if (!onSave) return;
        const value = draft.replace(/\s+/g, ' ').trim();
        if (!value || value === text.trim()) {
            setEditing(false);
            return;
        }
        setSaving(true);
        try {
            await onSave(id, value);
            setEditing(false);
        } finally {
            setSaving(false);
        }
    };

    // The secretary's workplace: time · status · the line in its speaker's colour
    if (tools) {
        const color = tools.colorOf?.(who) ?? 'var(--muted-text)';
        const unnamed = isUnnamed(who);
        const looping = tools.loopingId === id;
        const showHeader = !!label || active || !!voice;
        const editor = (
            <div className="flex flex-col gap-1.5">
                <textarea
                    autoFocus
                    aria-label="Виправити текст рядка"
                    value={draft}
                    rows={Math.min(8, Math.max(2, Math.ceil(draft.length / 60)))}
                    onChange={(e) => setDraft(e.target.value)}
                    onKeyDown={(e) => {
                        if (e.key === 'Enter' && !e.shiftKey) {
                            e.preventDefault();
                            void save();
                        } else if (e.key === 'Escape') {
                            e.preventDefault();
                            setEditing(false);
                        }
                    }}
                    className="w-full rounded-lg border border-[var(--blue)] bg-[var(--card-bg)] px-2 py-1.5 text-[15px] leading-[1.55] text-gray-900 outline-none"
                />
                <div className="flex items-center gap-2 text-xs">
                    <button type="button" disabled={saving} onClick={() => void save()} className="settings-button !h-7 !bg-[var(--blue)] !text-white">Зберегти</button>
                    <button type="button" onClick={() => setEditing(false)} className="settings-button !h-7">Скасувати</button>
                    {edited && onUndo && (
                        <button type="button" onClick={() => void onUndo(id).then(() => setEditing(false))} className="settings-button !h-7">Повернути розпізнане</button>
                    )}
                    <span className="ml-auto text-[color:var(--muted-text)]">Enter — зберегти · Esc — скасувати</span>
                </div>
            </div>
        );
        return (
            <div
                id={`segment-${id}`}
                data-start={timestamp}
                data-seg={id}
                onMouseDown={() => tools.onActivate(id)}
                className={`work-row ${active ? 'work-row-active' : ''} ${playing ? 'work-row-playing' : ''} ${voice ? 'work-row-voice' : ''} ${highlighted ? 'segment-highlight' : ''}`}
            >
                <button
                    type="button"
                    aria-label={`Слухати з ${formatRecordingTime(timestamp)}`}
                    onClick={() => window.dispatchEvent(new CustomEvent(TIME_CLICKED_EVENT, { detail: { seconds: timestamp, id } }))}
                    className="work-time tabular-nums"
                >
                    {formatRecordingTime(timestamp)}
                </button>
                <span className="work-status" aria-label={playing ? 'грає' : reviewed ? 'перевірено' : 'не перевірено'}>
                    {playing ? <Play className="h-3 w-3 text-[var(--blue)]" fill="currentColor" /> : reviewed ? <Check className="h-3.5 w-3.5 text-[#2fa66a]" strokeWidth={3} /> : <Circle className="h-3 w-3 text-[color:var(--hairline-strong,rgba(127,127,127,0.45))]" />}
                </span>
                <div className="work-body" style={{ borderLeftColor: color }}>
                    {showHeader && (
                        <div className="mb-0.5 flex min-h-[22px] flex-wrap items-center gap-x-2 gap-y-1">
                            {label && unnamed && onChangeSpeaker ? (
                                <button type="button" onClick={openPicker} className="name-pill" title="Назвати людину (S)">
                                    {label} · Назвати…
                                </button>
                            ) : label ? (
                                <button type="button" onClick={openPicker} className="speaker-name inline-flex items-center gap-0.5 text-[13px] font-semibold" style={{ color }} title="Змінити спікера (S)">
                                    {label}
                                    <ChevronDown className="h-3 w-3 opacity-70" />
                                </button>
                            ) : null}
                            {speakerManual && (
                                <span className="whitespace-nowrap text-[11px] font-medium text-[color:var(--muted-text)]">
                                    змінено вручну
                                    {onUndoSpeaker && (
                                        <button type="button" onClick={() => onUndoSpeaker(id)} className="ml-1 text-[var(--blue)] hover:underline">· повернути</button>
                                    )}
                                </span>
                            )}
                            {voice && <span className="voice-chip">Схоже, голос змінюється посеред рядка</span>}
                            {active && !isStreaming && !editing && (
                                <span className="ml-auto flex items-center gap-1" onMouseDown={(e) => e.stopPropagation()}>
                                    <button type="button" className="row-icon" onClick={() => tools.onPlayLine(id, false)} title="Слухати рядок · L"><Play className="h-3.5 w-3.5" fill="currentColor" /></button>
                                    <button type="button" className={`row-icon ${looping ? 'row-icon-on' : ''}`} onClick={() => tools.onPlayLine(id, true)} title="Повтор рядка · R"><Repeat className="h-3.5 w-3.5" /></button>
                                    {onChangeSpeaker && <button type="button" className="row-icon" onClick={openPicker} title="Спікер · S"><UserRound className="h-3.5 w-3.5" /></button>}
                                    {onSave && <button type="button" className="row-icon" onClick={start} title="Правити · Enter"><Pencil className="h-3.5 w-3.5" /></button>}
                                    {!isLast && <button type="button" className="row-icon" onClick={() => tools.onMerge(id)} title="Об'єднати з наступним"><Merge className="h-3.5 w-3.5" /></button>}
                                    <button type="button" className={`reviewed-button ${reviewed ? 'reviewed-on' : ''}`} onClick={() => tools.onToggleReviewed(id)} title="Перевірено і далі · V">
                                        <Check className="h-3.5 w-3.5" strokeWidth={3} /> {reviewed ? 'Перевірено' : 'Перевірено'} <kbd>V</kbd>
                                    </button>
                                </span>
                            )}
                        </div>
                    )}
                    {editing ? editor : (
                        <p
                            onDoubleClick={start}
                            onClick={(e) => {
                                if (!timed) return;
                                const sel = window.getSelection();
                                if (sel && !sel.isCollapsed) return;
                                const w = (e.target as HTMLElement).closest('[data-w]');
                                if (w) tools.onWord(id, Number(w.getAttribute('data-w')));
                            }}
                            className="work-text"
                        >
                            {timed
                                ? timed.map((w, i) => (
                                    <span key={i}>
                                        {voice && voice.from > 0 && i === voice.from && (
                                            <span className="cut-mark" title={`Розрізати тут: далі говорить ${voice.likely}`}><Scissors className="inline h-3 w-3" /> {voice.likely} — розрізати тут</span>
                                        )}
                                        <span
                                            data-w={i}
                                            className={`line-word ${i === currentWord ? 'word-current' : ''} ${doubtful?.has(i) ? 'word-doubt' : ''} ${voice && i >= voice.from && i < voice.to ? 'word-voice' : ''}`}
                                            title={doubtful?.has(i) ? 'Розпізнавання не певне — перевірте' : undefined}
                                        >
                                            {w.word}
                                        </span>
                                        {voice && voice.to < timed.length && i === voice.to - 1 && (
                                            <span className="cut-mark" title="Розрізати тут"> <Scissors className="inline h-3 w-3" /> розрізати тут</span>
                                        )}
                                        {i < timed.length - 1 ? ' ' : ''}
                                    </span>
                                ))
                                : displayText}
                            {edited && <span className="ml-1.5 whitespace-nowrap align-middle text-[11px] font-medium text-[color:var(--muted-text)]">· виправлено</span>}
                        </p>
                    )}
                    {voice && !editing && (
                        <div className="mt-1.5 flex flex-wrap items-center gap-1.5" onMouseDown={(e) => e.stopPropagation()}>
                            <button type="button" className="voice-accept" onClick={() => tools.onVoiceAccept?.(voice)}>
                                <Scissors className="h-3.5 w-3.5" /> Розрізати і віддати {voice.likely}
                            </button>
                            <button type="button" className="line-tool" onClick={() => tools.onVoiceReject?.(voice)}>Ні, одна людина</button>
                            <span className="text-[11px] text-[color:var(--muted-text)]">схожість голосу {Math.round(Math.max(0, voice.similarity) * 100)}%</span>
                        </div>
                    )}
                </div>
            </div>
        );
    }

    return (
        <div
            id={`segment-${id}`}
            data-start={timestamp}
            data-seg={id}
            className={`transcript-segment ${compact ? 'mb-2' : 'mb-4'} ${highlighted ? 'segment-highlight' : ''} ${playing && !highlighted ? 'segment-playing' : ''} ${active ? 'segment-active' : ''}`}
        >
            <div className={`grid items-start ${compact ? 'grid-cols-[64px_minmax(0,1fr)] gap-3' : 'grid-cols-[72px_minmax(0,1fr)] gap-4'}`}>
                <button
                    type="button"
                    aria-label={`Слухати з ${formatRecordingTime(timestamp)}`}
                    onClick={() => window.dispatchEvent(new CustomEvent(TIME_CLICKED_EVENT, { detail: { seconds: timestamp, id } }))}
                    className={`time-mark ${compact ? 'text-xs pt-[2px]' : 'text-[13px] pt-[3px]'} flex items-center gap-1 text-left text-[color:var(--muted-text)] tabular-nums cursor-pointer`}
                >
                    <Play className="time-play h-2.5 w-2.5 flex-none text-[var(--blue)]" fill="currentColor" aria-hidden="true" />
                    <span className="time-text">{formatRecordingTime(timestamp)}</span>
                    {reviewed && <Check className="h-3 w-3 flex-none text-[#34a853]" aria-label="перевірено" />}
                </button>
                <div>
                    {label && (
                        <div className={`${compact ? 'text-xs' : 'text-[13px]'} mb-0.5 flex flex-wrap items-baseline gap-x-1.5`}>
                            {onChangeSpeaker ? (
                                <button type="button" onClick={openPicker} title="Змінити спікера" className="speaker-name font-semibold text-gray-900">
                                    {label}
                                </button>
                            ) : (
                                <span className="font-semibold text-gray-900">{label}</span>
                            )}
                            {speakerManual && (
                                <span className="whitespace-nowrap text-[11px] font-medium text-[color:var(--muted-text)]">
                                    · змінено вручну
                                    {onUndoSpeaker && (
                                        <button type="button" onClick={() => onUndoSpeaker(id)} className="ml-1 text-[var(--blue)] hover:underline">
                                            повернути
                                        </button>
                                    )}
                                </span>
                            )}
                        </div>
                    )}
                    {editing ? (
                        <div className="flex flex-col gap-1.5">
                            <textarea
                                autoFocus
                                aria-label="Виправити текст рядка"
                                value={draft}
                                rows={Math.min(8, Math.max(2, Math.ceil(draft.length / 48)))}
                                onChange={(e) => setDraft(e.target.value)}
                                onKeyDown={(e) => {
                                    if (e.key === 'Enter' && !e.shiftKey) {
                                        e.preventDefault();
                                        void save();
                                    } else if (e.key === 'Escape') {
                                        e.preventDefault();
                                        setEditing(false);
                                    }
                                }}
                                className={`w-full rounded-lg border border-[var(--blue)] bg-[var(--card-bg)] px-2 py-1.5 ${compact ? 'text-sm' : 'text-[17px]'} leading-[1.5] text-gray-900 outline-none`}
                            />
                            <div className="flex items-center gap-2 text-xs">
                                <button type="button" disabled={saving} onClick={() => void save()} className="settings-button !h-7 !bg-[var(--blue)] !text-white">
                                    Зберегти
                                </button>
                                <button type="button" onClick={() => setEditing(false)} className="settings-button !h-7">
                                    Скасувати
                                </button>
                                {edited && onUndo && (
                                    <button type="button" onClick={() => void onUndo(id).then(() => setEditing(false))} className="settings-button !h-7">
                                        Повернути розпізнане
                                    </button>
                                )}
                                <span className="ml-auto text-[color:var(--muted-text)]">Enter — зберегти, Esc — скасувати</span>
                            </div>
                        </div>
                    ) : (
                        <div className="group/line relative">
                            <p
                                onDoubleClick={start}
                                className={`${compact ? 'text-sm leading-[1.5]' : 'text-[17px] leading-[1.55]'} ${isStreaming ? 'text-[color:var(--muted-text)]' : 'text-gray-900'} ${onSave ? 'cursor-text' : ''}`}
                            >
                                {timed
                                    ? timed.map((w, i) => (
                                        <span key={i}>
                                            <span
                                                data-w={i}
                                                className={`line-word ${i === currentWord ? 'word-current' : ''} ${doubtful?.has(i) ? 'underline decoration-dotted decoration-[var(--blue)] underline-offset-4' : ''}`}
                                                title={doubtful?.has(i) ? 'Розпізнавання не певне — перевірте' : undefined}
                                            >
                                                {w.word}
                                            </span>
                                            {i < timed.length - 1 ? ' ' : ''}
                                        </span>
                                    ))
                                    : doubtful && doubtful.size > 0
                                    ? text.split(/(\s+)/).map((part, i) => {
                                        const index = Math.floor(i / 2);
                                        return i % 2 === 0 && doubtful.has(index) ? (
                                            <span key={i} title="Розпізнавання не певне — перевірте" className="underline decoration-dotted decoration-[var(--blue)] underline-offset-4">{part}</span>
                                        ) : (
                                            <span key={i}>{part}</span>
                                        );
                                    })
                                    : displayText}
                                {edited && <span className="ml-1.5 whitespace-nowrap align-middle text-[11px] font-medium text-[color:var(--muted-text)]">· виправлено</span>}
                            </p>
                            {(onSave || onChangeSpeaker) && !isStreaming && (
                                <div className="absolute -right-1 -top-1 hidden group-hover/line:flex gap-0.5 rounded-md bg-[var(--card-bg)] shadow-sm">
                                    {onChangeSpeaker && (
                                        <button
                                            type="button"
                                            title="Змінити спікера цього рядка"
                                            aria-label="Змінити спікера"
                                            onClick={openPicker}
                                            className="flex h-6 w-6 items-center justify-center rounded-md text-[color:var(--muted-text)] hover:text-gray-900"
                                        >
                                            <UserRound className="h-3.5 w-3.5" />
                                        </button>
                                    )}
                                    {onSave && (
                                        <button
                                            type="button"
                                            title="Виправити текст (або двічі клацніть по ньому)"
                                            aria-label="Виправити текст"
                                            onClick={start}
                                            className="flex h-6 w-6 items-center justify-center rounded-md text-[color:var(--muted-text)] hover:text-gray-900"
                                        >
                                            <Pencil className="h-3.5 w-3.5" />
                                        </button>
                                    )}
                                </div>
                            )}
                        </div>
                    )}
                </div>
            </div>
        </div>
    );
});

/** «Спікер 2: text» → speaker and text */
function splitSpeaker(text: string): [string | undefined, string] {
    const m = text.match(/^([^:\n]{1,40}): ([\s\S]*)$/);
    return m ? [m[1], m[2]] : [undefined, text];
}

/** The speaker label for a line: only when it differs from the line before */
function speakerLabel(segments: TranscriptSegmentData[], index: number): string | undefined {
    const [current] = splitSpeaker(segments[index]?.text ?? '');
    if (!current) return undefined;
    const [previous] = splitSpeaker(segments[index - 1]?.text ?? '');
    return current === previous ? undefined : current;
}

/** The word heard at `seconds` in a line */
function wordAt(words: WordTime[] | undefined, seconds: number): number | undefined {
    if (!words || words.length === 0) return undefined;
    let k = -1;
    for (let i = 0; i < words.length; i++) {
        if (words[i].start <= seconds) k = i;
        else break;
    }
    return k >= 0 ? k : undefined;
}

/** The first of the lines whose speaker was set by hand in a row: marked «змінено вручну» */
function manualStart(segments: TranscriptSegmentData[], index: number): boolean {
    const seg = segments[index];
    if (!seg?.speakerManual) return false;
    const prev = segments[index - 1];
    return !(prev?.speakerManual && splitSpeaker(prev.text)[0] === splitSpeaker(seg.text)[0]);
}

/** Mute markers written by older versions are not text: hidden */
const MIC_MARKER = /^\s*—\s*Мікрофон (вимкнено|увімкнено)\s*—\s*$/;

export const VirtualizedTranscriptView: React.FC<VirtualizedTranscriptViewProps> = ({
    segments: allSegments,
    isRecording = false,
    isPaused = false,
    isProcessing = false,
    isStopping = false,
    enableStreaming = false,
    showConfidence = true,
    disableAutoScroll = false,
    showStatusBar = true,
    compact = false,
    hasMore = false,
    isLoadingMore = false,
    totalCount = 0,
    loadedCount = 0,
    onLoadMore,
    onEditLine,
    onUndoLine,
    onChangeSpeaker,
    onUndoSpeaker,
    tools,
    doubts,
    playingSeconds,
    followPlayback = false,
}) => {
    const segments = useMemo(() => allSegments.filter((seg) => !MIC_MARKER.test(seg.text ?? '')), [allSegments]);
    // Create scroll ref first - shared between virtualizer and auto-scroll hook
    const scrollRef = useRef<HTMLDivElement>(null);
    // Ref for infinite scroll trigger element
    const loadMoreTriggerRef = useRef<HTMLDivElement>(null);

    // Force re-render without flushSync (avoids React warning)
    const [, rerender] = useReducer((x: number) => x + 1, 0);

    // Setup virtualizer for efficient rendering of large lists
    const virtualizer = useVirtualizer({
        count: segments.length,
        getScrollElement: () => scrollRef.current,
        estimateSize: () => 60, // Estimated height per segment
        overscan: 10, // Render extra items above/below viewport
        onChange: () => {
            startTransition(() => {
                rerender();
            });
        },
    });

    // «Show this moment»: scroll the list itself to the line that holds it
    // (only nearby lines exist on the page, so searching the page is not enough)
    const [highlightId, setHighlightId] = useState<string | null>(null);
    useEffect(() => {
        const onJump = (e: Event) => {
            const seconds = (e as CustomEvent<{ seconds: number }>).detail?.seconds;
            if (typeof seconds !== 'number') return;
            const index = Math.max(0, segmentIndexAt(segments, seconds));
            const segment = segments[index];
            if (!segment) return;
            setHighlightId(segment.id);
            if (segments.length >= VIRTUALIZATION_THRESHOLD) {
                virtualizer.scrollToIndex(index, { align: 'center' });
            } else {
                document.getElementById(`segment-${segment.id}`)?.scrollIntoView({ behavior: 'smooth', block: 'center' });
            }
            window.setTimeout(() => setHighlightId((h) => (h === segment.id ? null : h)), 4000);
            // Play from where this line really starts (the document gives whole seconds)
            if (Math.abs(segment.timestamp - seconds) < 1) {
                window.dispatchEvent(new CustomEvent(TIME_CLICKED_EVENT, { detail: { seconds: segment.timestamp } }));
            }
        };
        // A click on a line's own time mark highlights exactly that line
        const onClicked = (e: Event) => {
            const id = (e as CustomEvent<{ id?: string }>).detail?.id;
            if (!id) return;
            setHighlightId(id);
            window.setTimeout(() => setHighlightId((h) => (h === id ? null : h)), 4000);
        };
        window.addEventListener(JUMP_TO_TIME_EVENT, onJump);
        window.addEventListener(TIME_CLICKED_EVENT, onClicked);
        window.addEventListener(LINE_CHANGED_EVENT, onClicked);
        return () => {
            window.removeEventListener(JUMP_TO_TIME_EVENT, onJump);
            window.removeEventListener(TIME_CLICKED_EVENT, onClicked);
            window.removeEventListener(LINE_CHANGED_EVENT, onClicked);
        };
    }, [segments, virtualizer]);

    // The list keeps its place when the lines are read again (after an edit,
    // a speaker change, a new recognition): the first line in view is
    // remembered by id and time; if it is gone, the line at that time is
    // brought back to the top
    const anchorRef = useRef<{ id: string; seconds: number; offset: number } | null>(null);
    useEffect(() => {
        const el = scrollRef.current;
        if (!el) return;
        const remember = () => {
            const top = el.scrollTop;
            const first = virtualizer.getVirtualItems().find((v) => v.end > top);
            const seg = first ? segments[first.index] : undefined;
            anchorRef.current = seg && first ? { id: seg.id, seconds: seg.timestamp, offset: top - first.start } : null;
        };
        el.addEventListener('scroll', remember, { passive: true });
        return () => el.removeEventListener('scroll', remember);
    }, [segments, virtualizer]);
    useLayoutEffect(() => {
        const anchor = anchorRef.current;
        if (!anchor || segments.length < VIRTUALIZATION_THRESHOLD) return;
        if (segments.some((s) => s.id === anchor.id)) return;
        const index = Math.max(0, segmentIndexAt(segments, anchor.seconds));
        virtualizer.scrollToIndex(index, { align: 'start' });
    }, [segments, virtualizer]);

    // The line in focus (keys ↑/↓) stays in view
    const activeId = tools?.activeId;
    useEffect(() => {
        if (!activeId) return;
        const index = segments.findIndex((s) => s.id === activeId);
        if (index < 0) return;
        if (segments.length >= VIRTUALIZATION_THRESHOLD) virtualizer.scrollToIndex(index, { align: 'auto' });
        else document.getElementById(`segment-${activeId}`)?.scrollIntoView({ block: 'nearest' });
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [activeId]);

    // Streaming text effect hook (typewriter animation for new transcripts)
    const { streamingSegmentId, getDisplayText } = useTranscriptStreaming(
        segments,
        isRecording,
        enableStreaming
    );

    // The newest line grows while it is being recognised: follow it too
    const lastSegment = segments[segments.length - 1];
    const followKey = lastSegment ? `${lastSegment.id}:${getDisplayText(lastSegment).length}` : '';

    // Auto-follow the newest line (virtualised and plain lists)
    const { autoScroll, scrollToBottom, newCount } = useAutoScroll({
        scrollRef,
        segments,
        isRecording,
        isPaused,
        virtualizer,
        virtualizationThreshold: VIRTUALIZATION_THRESHOLD,
        disableAutoScroll,
        contentKey: followKey,
    });
    const showJumpToLatest = !disableAutoScroll && !autoScroll && segments.length > 0 && (isRecording || newCount > 0);

    // Infinite scroll: IntersectionObserver to trigger loading more
    useEffect(() => {
        if (!onLoadMore || !hasMore || isLoadingMore || isRecording || segments.length === 0) {
            return;
        }

        const triggerElement = loadMoreTriggerRef.current;
        if (!triggerElement) return;

        const observer = new IntersectionObserver(
            (entries) => {
                if (entries[0].isIntersecting && hasMore && !isLoadingMore) {
                    onLoadMore();
                }
            },
            {
                root: null,
                rootMargin: '100px',
                threshold: 0,
            }
        );

        observer.observe(triggerElement);

        return () => observer.disconnect();
    }, [hasMore, isLoadingMore, onLoadMore, isRecording, segments.length]);

    // Scroll-based fallback for fast scrolling
    useEffect(() => {
        if (!onLoadMore || !hasMore || isLoadingMore || isRecording) return;

        const scrollElement = scrollRef.current;
        if (!scrollElement) return;

        let ticking = false;

        const handleScroll = () => {
            if (ticking || isLoadingMore || !hasMore) return;

            ticking = true;
            requestAnimationFrame(() => {
                const { scrollTop, scrollHeight, clientHeight } = scrollElement;
                const scrollBottom = scrollHeight - scrollTop - clientHeight;

                // Trigger load when within 200px of bottom
                if (scrollBottom < 200 && hasMore && !isLoadingMore) {
                    onLoadMore();
                }
                ticking = false;
            });
        };

        scrollElement.addEventListener('scroll', handleScroll, { passive: true });
        return () => scrollElement.removeEventListener('scroll', handleScroll);
    }, [onLoadMore, hasMore, isLoadingMore, isRecording]);

    // Use simple rendering for small lists, virtualization for large lists
    const useVirtualization = segments.length >= VIRTUALIZATION_THRESHOLD;

    // The line being played; the list follows it unless the user scrolled away
    const playingIndex = playingSeconds === undefined ? -1 : segmentIndexAt(segments, playingSeconds);
    const [scrolledAway, setScrolledAway] = useState(false);
    const followingRef = useRef(false);
    const scrollToPlaying = useCallback(() => {
        if (playingIndex < 0) return;
        followingRef.current = true;
        if (useVirtualization) {
            virtualizer.scrollToIndex(playingIndex, { align: 'center' });
        } else {
            document.getElementById(`segment-${segments[playingIndex]?.id}`)?.scrollIntoView({ behavior: 'smooth', block: 'center' });
        }
        window.setTimeout(() => {
            followingRef.current = false;
        }, 400);
    }, [playingIndex, useVirtualization, virtualizer, segments]);
    useEffect(() => {
        if (followPlayback && !scrolledAway) scrollToPlaying();
    }, [playingIndex, followPlayback, scrolledAway, scrollToPlaying]);
    useEffect(() => {
        if (!followPlayback) setScrolledAway(false);
    }, [followPlayback]);
    useEffect(() => {
        const el = scrollRef.current;
        if (!el) return;
        // Only the user's own scrolling (wheel, touch, keys) stops following
        const away = () => {
            if (followPlayback && !followingRef.current) setScrolledAway(true);
        };
        el.addEventListener('wheel', away, { passive: true });
        el.addEventListener('touchmove', away, { passive: true });
        return () => {
            el.removeEventListener('wheel', away);
            el.removeEventListener('touchmove', away);
        };
    }, [followPlayback]);

    return (
        <div className="relative h-full min-h-0">
            {followPlayback && scrolledAway && playingIndex >= 0 && (
                <button
                    type="button"
                    onClick={() => {
                        setScrolledAway(false);
                        scrollToPlaying();
                    }}
                    className="absolute bottom-2 right-3 z-20 flex h-7 items-center gap-1 rounded-full bg-[var(--card-bg)] px-2.5 text-xs font-semibold text-[var(--blue)] shadow-md border border-[var(--card-border)]"
                >
                    <ArrowDown className="h-3.5 w-3.5" /> До поточного
                </button>
            )}
        <div ref={scrollRef} tabIndex={-1} className="flex flex-col h-full overflow-y-auto custom-scrollbar px-4 py-2 outline-none">
            {/* Recording Status Bar - Sticky at top, always visible when recording */}
            <AnimatePresence>
                {isRecording && showStatusBar && (
                    <div className="sticky top-0 z-10 bg-white pb-2">
                        <RecordingStatusBar isPaused={isPaused} />
                    </div>
                )}
            </AnimatePresence>

            {/* Content - add padding when recording to prevent overlap */}
            <div className={isRecording ? 'pt-2' : ''}>
            {segments.length === 0 ? (
                // Empty state
                <motion.div
                    initial={{ opacity: 0 }}
                    animate={{ opacity: 1 }}
                    className="text-center text-gray-500 mt-8"
                >
                    {isRecording ? (
                        <>
                            <div className="flex items-center justify-center mb-3">
                                <div className={`w-3 h-3 rounded-full ${isPaused ? 'bg-gray-400' : 'record-dot animate-pulse'}`}></div>
                            </div>
                            <p className="text-sm text-gray-600">
                                {isPaused ? 'Запис на паузі' : 'Іде запис…'}
                            </p>
                            <p className="text-xs mt-1 text-gray-400">
                                {isPaused ? 'Натисніть «Продовжити», щоб записувати далі' : 'Текст з’явиться, щойно хтось заговорить'}
                            </p>
                        </>
                    ) : (
                        <>
                            <p className="text-base font-semibold">Стенограма порожня</p>
                        </>
                    )}
                </motion.div>
            ) : useVirtualization ? (
                // Virtualized rendering for large lists
                <>
                    <div
                        style={{
                            height: virtualizer.getTotalSize(),
                            width: "100%",
                            position: "relative",
                        }}
                    >
                        {virtualizer.getVirtualItems().map((virtualRow) => {
                            const segment = segments[virtualRow.index];
                            const isStreaming = streamingSegmentId === segment.id;

                            return (
                                <div
                                    key={segment.id}
                                    data-index={virtualRow.index}
                                    ref={virtualizer.measureElement}
                                    style={{
                                        position: "absolute",
                                        top: 0,
                                        left: 0,
                                        width: "100%",
                                        transform: `translateY(${virtualRow.start}px)`,
                                    }}
                                >
                                    <TranscriptSegment
                                        id={segment.id}
                                        timestamp={segment.timestamp}
                                        text={splitSpeaker(getDisplayText(segment))[1]}
                                        confidence={segment.confidence}
                                        isStreaming={isStreaming}
                                        showConfidence={showConfidence}
                                        compact={compact}
                                        highlighted={highlightId === segment.id}
                                        playing={playingIndex >= 0 && segments[playingIndex]?.id === segment.id}
                                        speaker={speakerLabel(segments, virtualRow.index)}
                                        who={splitSpeaker(segment.text)[0]}
                                        speakerManual={manualStart(segments, virtualRow.index)}
                                        onChangeSpeaker={onChangeSpeaker}
                                        onUndoSpeaker={onUndoSpeaker}
                                        words={tools?.words?.get(segment.id)}
                                        currentWord={playingIndex >= 0 && segments[playingIndex]?.id === segment.id && playingSeconds !== undefined ? wordAt(tools?.words?.get(segment.id), playingSeconds) : undefined}
                                        reviewed={segment.reviewed}
                                        active={tools?.activeId === segment.id}
                                        tools={tools}
                                        isLast={virtualRow.index === segments.length - 1 && !hasMore}
                                        edited={segment.edited}
                                        onSave={onEditLine}
                                        onUndo={onUndoLine}
                                        doubtful={doubts?.get(segment.id)}
                                    />
                                </div>
                            );
                        })}
                    </div>

                    {/* Infinite scroll trigger and loading indicator */}
                    {(hasMore || isLoadingMore) && !isRecording && segments.length > 0 && (
                        <div ref={loadMoreTriggerRef} className="flex justify-center items-center py-4 mt-2">
                            {isLoadingMore ? (
                                <div className="flex items-center gap-2 text-gray-500">
                                    <div className="w-4 h-4 border-2 border-gray-300 border-t-gray-600 rounded-full animate-spin" />
                                    <span className="text-sm">Завантаження…</span>
                                </div>
                            ) : hasMore && totalCount > 0 ? (
                                <span className="text-sm text-gray-400">
                                    Показано {loadedCount} з {totalCount}
                                </span>
                            ) : null}
                        </div>
                    )}

                    {/* Listening indicator when recording */}
                    {!isStopping && isRecording && !isPaused && !isProcessing && segments.length > 0 && (
                        <motion.div
                            initial={{ opacity: 0 }}
                            animate={{ opacity: 1 }}
                            exit={{ opacity: 0 }}
                            className="flex items-center gap-2 mt-4 text-gray-500"
                        >
                            <div className="w-2 h-2 record-dot rounded-full animate-pulse"></div>
                            <span className="text-sm">Іде запис…</span>
                        </motion.div>
                    )}
                </>
            ) : (
                // Simple rendering for small lists (better animations)
                <>
                    <div className="space-y-1">
                        {segments.map((segment, index) => {
                            const isStreaming = streamingSegmentId === segment.id;

                            return (
                                <motion.div
                                    key={segment.id}
                                    initial={{ opacity: 0, y: 5 }}
                                    animate={{ opacity: 1, y: 0 }}
                                    transition={{ duration: 0.15 }}
                                >
                                    <TranscriptSegment
                                        id={segment.id}
                                        timestamp={segment.timestamp}
                                        text={splitSpeaker(getDisplayText(segment))[1]}
                                        confidence={segment.confidence}
                                        isStreaming={isStreaming}
                                        showConfidence={showConfidence}
                                        compact={compact}
                                        highlighted={highlightId === segment.id}
                                        playing={playingIndex >= 0 && segments[playingIndex]?.id === segment.id}
                                        speaker={speakerLabel(segments, index)}
                                        who={splitSpeaker(segment.text)[0]}
                                        speakerManual={manualStart(segments, index)}
                                        onChangeSpeaker={onChangeSpeaker}
                                        onUndoSpeaker={onUndoSpeaker}
                                        words={tools?.words?.get(segment.id)}
                                        currentWord={playingIndex >= 0 && segments[playingIndex]?.id === segment.id && playingSeconds !== undefined ? wordAt(tools?.words?.get(segment.id), playingSeconds) : undefined}
                                        reviewed={segment.reviewed}
                                        active={tools?.activeId === segment.id}
                                        tools={tools}
                                        isLast={index === segments.length - 1 && !hasMore}
                                        edited={segment.edited}
                                        onSave={onEditLine}
                                        onUndo={onUndoLine}
                                        doubtful={doubts?.get(segment.id)}
                                    />
                                </motion.div>
                            );
                        })}
                    </div>

                    {/* Infinite scroll trigger (for small lists that grow) */}
                    {(hasMore || isLoadingMore) && !isRecording && segments.length > 0 && (
                        <div ref={loadMoreTriggerRef} className="flex justify-center items-center py-4 mt-2">
                            {isLoadingMore ? (
                                <div className="flex items-center gap-2 text-gray-500">
                                    <div className="w-4 h-4 border-2 border-gray-300 border-t-gray-600 rounded-full animate-spin" />
                                    <span className="text-sm">Завантаження…</span>
                                </div>
                            ) : hasMore && totalCount > 0 ? (
                                <span className="text-sm text-gray-400">
                                    Показано {loadedCount} з {totalCount}
                                </span>
                            ) : null}
                        </div>
                    )}

                    {/* Listening indicator when recording */}
                    {!isStopping && isRecording && !isPaused && !isProcessing && segments.length > 0 && (
                        <motion.div
                            initial={{ opacity: 0 }}
                            animate={{ opacity: 1 }}
                            exit={{ opacity: 0 }}
                            className="flex items-center gap-2 mt-4 text-gray-500"
                        >
                            <div className="w-2 h-2 record-dot rounded-full animate-pulse"></div>
                            <span className="text-sm">Іде запис…</span>
                        </motion.div>
                    )}
                </>
            )}
            </div>
        </div>
        {showJumpToLatest && (
            <button
                type="button"
                onClick={scrollToBottom}
                className="absolute left-1/2 -translate-x-1/2 bottom-3 z-20 inline-flex items-center gap-1.5 h-8 px-3.5 rounded-full text-[13px] font-semibold bg-gray-900 text-[hsl(var(--primary-foreground))] shadow-lg"
            >
                ↓ До останнього{newCount > 0 ? ` · ${newCount} ${newLinesWord(newCount)}` : ''}
            </button>
        )}
        </div>
    );
};

function newLinesWord(n: number): string {
    const mod10 = n % 10;
    const mod100 = n % 100;
    if (mod10 === 1 && mod100 !== 11) return 'новий рядок';
    if (mod10 >= 2 && mod10 <= 4 && (mod100 < 12 || mod100 > 14)) return 'нові рядки';
    return 'нових рядків';
}
