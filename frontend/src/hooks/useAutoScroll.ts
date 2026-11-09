import { useRef, useState, useEffect, useCallback, useLayoutEffect, RefObject } from "react";
import { Virtualizer } from "@tanstack/react-virtual";

interface UseAutoScrollProps {
    scrollRef: RefObject<HTMLDivElement | null>;
    segments: { id: string }[];
    isRecording: boolean;
    isPaused: boolean;
    activeSegmentId?: string;
    virtualizer?: Virtualizer<HTMLDivElement, Element>;
    virtualizationThreshold?: number;
    /** Completely disable auto-scroll (meeting details page) */
    disableAutoScroll?: boolean;
    /** Changes whenever the newest line grows (streaming text), so following keeps up */
    contentKey?: string | number;
}

interface UseAutoScrollReturn {
    /** Following the newest line */
    autoScroll: boolean;
    setAutoScroll: (value: boolean) => void;
    /** Jump back to the newest line and resume following */
    scrollToBottom: () => void;
    /** Lines that arrived while the user was reading above */
    newCount: number;
}

/** Within this many pixels of the end counts as "at the bottom" */
const BOTTOM_THRESHOLD = 48;
/** Input events within this window mean the scroll came from the user */
const USER_INTENT_MS = 600;

function prefersReducedMotion(): boolean {
    return typeof window !== "undefined" && !!window.matchMedia?.("(prefers-reduced-motion: reduce)").matches;
}

/**
 * Keeps the live transcript on its newest line.
 *
 * - Follows smoothly as lines arrive and as the current line grows
 *   (jumps instead when the system asks to reduce motion).
 * - Scrolling up by hand pauses following and counts new lines; scrolling
 *   back to the bottom (or `scrollToBottom`) resumes it.
 * - Works with the virtual list: when the newest line is not rendered, it is
 *   brought in with `scrollToIndex` first.
 */
export function useAutoScroll({
    scrollRef,
    segments,
    isRecording,
    isPaused,
    activeSegmentId,
    virtualizer,
    virtualizationThreshold = 10,
    disableAutoScroll = false,
    contentKey,
}: UseAutoScrollProps): UseAutoScrollReturn {
    const useVirtualization = !!virtualizer && segments.length >= virtualizationThreshold;
    const [autoScroll, setAutoScrollState] = useState(true);
    const [newCount, setNewCount] = useState(0);
    const followRef = useRef(true);
    const lastUserInputRef = useRef(0);
    const prevCountRef = useRef(segments.length);

    const setAutoScroll = useCallback((value: boolean) => {
        followRef.current = value;
        setAutoScrollState(value);
        if (value) setNewCount(0);
    }, []);

    const isNearBottom = useCallback(() => {
        const el = scrollRef.current;
        if (!el) return true;
        return el.scrollHeight - el.scrollTop - el.clientHeight <= BOTTOM_THRESHOLD;
    }, [scrollRef]);

    /** Scrolls to the end of the list; `smooth` is ignored with Reduce Motion */
    const scrollToEnd = useCallback(
        (smooth: boolean) => {
            const el = scrollRef.current;
            if (!el) return;
            const behavior: ScrollBehavior = smooth && !prefersReducedMotion() ? "smooth" : "auto";
            const lastIndex = segments.length - 1;
            if (useVirtualization && virtualizer && lastIndex >= 0) {
                const rendered = virtualizer.getVirtualItems();
                const lastRendered = rendered.length ? rendered[rendered.length - 1].index : -1;
                if (lastRendered < lastIndex) {
                    // Far away: bring the newest line into the virtual window first
                    virtualizer.scrollToIndex(lastIndex, { align: "end" });
                }
            }
            // Then settle at the very end (includes the "Іде запис…" line below the list)
            requestAnimationFrame(() => {
                el.scrollTo({ top: el.scrollHeight, behavior });
            });
        },
        [scrollRef, segments.length, useVirtualization, virtualizer],
    );

    const scrollToBottom = useCallback(() => {
        setAutoScroll(true);
        scrollToEnd(true);
    }, [scrollToEnd, setAutoScroll]);

    // Tell user scrolling apart from our own: only scrolls right after user input pause following
    useEffect(() => {
        const el = scrollRef.current;
        if (!el) return;
        const markUserInput = () => {
            lastUserInputRef.current = Date.now();
        };
        const onKey = (event: KeyboardEvent) => {
            if (["ArrowUp", "ArrowDown", "PageUp", "PageDown", "Home", "End", " "].includes(event.key)) markUserInput();
        };
        const onScroll = () => {
            if (isNearBottom()) {
                if (!followRef.current) setAutoScroll(true);
                return;
            }
            if (Date.now() - lastUserInputRef.current < USER_INTENT_MS && followRef.current) {
                setAutoScroll(false);
            }
        };
        el.addEventListener("wheel", markUserInput, { passive: true });
        el.addEventListener("touchmove", markUserInput, { passive: true });
        el.addEventListener("pointerdown", markUserInput, { passive: true });
        el.addEventListener("keydown", onKey);
        el.addEventListener("scroll", onScroll, { passive: true });
        return () => {
            el.removeEventListener("wheel", markUserInput);
            el.removeEventListener("touchmove", markUserInput);
            el.removeEventListener("pointerdown", markUserInput);
            el.removeEventListener("keydown", onKey);
            el.removeEventListener("scroll", onScroll);
        };
    }, [scrollRef, isNearBottom, setAutoScroll]);

    // Count lines that arrive while the user reads above
    useEffect(() => {
        const added = segments.length - prevCountRef.current;
        prevCountRef.current = segments.length;
        if (added > 0 && !followRef.current) setNewCount((n) => n + added);
        if (segments.length === 0) setNewCount(0);
    }, [segments.length]);

    // Follow new lines and the growing current line
    useLayoutEffect(() => {
        if (disableAutoScroll || !isRecording || isPaused || segments.length === 0) return;
        if (!followRef.current) return;
        scrollToEnd(true);
    }, [segments.length, contentKey, isRecording, isPaused, disableAutoScroll, scrollToEnd]);

    // Scroll to a chosen segment (search results etc.)
    useEffect(() => {
        if (!activeSegmentId) return;
        const behavior: ScrollBehavior = prefersReducedMotion() ? "auto" : "smooth";
        if (useVirtualization && virtualizer) {
            const index = segments.findIndex((s) => s.id === activeSegmentId);
            if (index >= 0) virtualizer.scrollToIndex(index, { align: "center" });
        } else {
            document.getElementById(`segment-${activeSegmentId}`)?.scrollIntoView({ behavior, block: "center" });
        }
    }, [activeSegmentId, useVirtualization, virtualizer, segments]);

    return { autoScroll, setAutoScroll, scrollToBottom, newCount };
}
