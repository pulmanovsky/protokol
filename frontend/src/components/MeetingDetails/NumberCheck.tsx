'use client';

import { useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { invoke } from '@tauri-apps/api/core';

/** A number of the document not found in the transcript (places in the bullet's text) */
interface Issue {
  start: number;
  end: number;
  text: string;
  fix?: string | null;
  /** The transcript itself has it both ways near this point */
  mixed?: boolean;
}

interface Marked {
  range: Range;
  issue: Issue;
  blockText: string;
}

const HIGHLIGHT = 'num-missing';

/** The moment a bullet points to: its own ⟨mm:ss⟩, else its «ВИСТУПИЛИ» block's [mm:ss] */
function secondsOf(block: Element): number | null {
  const own = (block.textContent ?? '').match(/⟨(\d{1,2}):(\d{2})⟩/);
  if (own) return Number(own[1]) * 60 + Number(own[2]);
  let prev = block.closest('.bn-block-outer')?.previousElementSibling as Element | null;
  for (let k = 0; prev && k < 40; k++, prev = prev.previousElementSibling) {
    const m = (prev.textContent ?? '').match(/ВИСТУПИЛИ:.*?\[(\d{1,2}):(\d{2})/);
    if (m) return Number(m[1]) * 60 + Number(m[2]);
  }
  return null;
}

/** A Range over chars `start..end` of an element's text */
function rangeOf(el: Element, start: number, end: number): Range | null {
  const walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
  let seen = 0;
  let from: [Node, number] | null = null;
  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    // Offsets come in chars (code points), DOM offsets in UTF-16 units
    const text = node.textContent ?? '';
    const chars = Array.from(text);
    const units = (n: number) => chars.slice(0, n).join('').length;
    if (!from && start < seen + chars.length) from = [node, units(start - seen)];
    if (from && end <= seen + chars.length) {
      const r = document.createRange();
      r.setStart(from[0], from[1]);
      r.setEnd(node, units(end - seen));
      return r;
    }
    seen += chars.length;
  }
  return null;
}

/**
 * Numbers of the document checked against the transcript: one not said is
 * marked amber («Немає в стенограмі»), with «Замінити на …» when the
 * transcript has a likely original. Nothing is changed on its own.
 */
export function useNumberCheck(
  docRef: React.RefObject<HTMLDivElement | null>,
  meetingId: string,
  active: boolean,
  replace: (blockText: string, from: string, to: string) => boolean,
) {
  const [count, setCount] = useState(0);
  const [hover, setHover] = useState<{ x: number; y: number; m: Marked } | null>(null);
  const marked = useRef<Marked[]>([]);
  const overPopover = useRef(false);
  const onNumber = useRef(false);

  useEffect(() => {
    if (!active) return;
    let alive = true;
    let timer: number | undefined;
    const highlights = (CSS as unknown as { highlights?: Map<string, unknown> }).highlights;
    const HighlightCtor = (window as unknown as { Highlight?: new (...r: Range[]) => unknown }).Highlight;
    const run = async () => {
      const root = docRef.current;
      if (!root) return;
      const blocks = Array.from(root.querySelectorAll('.bn-block-content[data-content-type="bulletListItem"], .bn-block-content[data-content-type="numberedListItem"]'));
      const bullets = blocks.map((b) => ({ text: b.textContent ?? '', seconds: secondsOf(b) }));
      let found: Issue[][] = [];
      try {
        found = await invoke<Issue[][]>('api_check_document_numbers', { meetingId, bullets });
      } catch {
        return;
      }
      if (!alive) return;
      const next: Marked[] = [];
      found.forEach((issues, i) => {
        const block = blocks[i];
        if (!block || (block.textContent ?? '') !== bullets[i].text) return;
        for (const issue of issues) {
          const range = rangeOf(block, issue.start, issue.end);
          if (range) next.push({ range, issue, blockText: bullets[i].text });
        }
        // Without the highlight API (older macOS): the bullet is tinted instead
        if (!highlights) block.classList.toggle('num-missing-block', issues.length > 0);
      });
      marked.current = next;
      setCount(next.length);
      if (highlights && HighlightCtor) {
        if (next.length) highlights.set(HIGHLIGHT, new HighlightCtor(...next.map((m) => m.range)));
        else highlights.delete(HIGHLIGHT);
      }
    };
    const later = () => {
      window.clearTimeout(timer);
      timer = window.setTimeout(() => void run(), 700);
    };
    later();
    const observer = new MutationObserver(later);
    if (docRef.current) observer.observe(docRef.current, { childList: true, subtree: true, characterData: true });
    // Hovering a marked number: what is wrong, and the fix when there is one
    const onMove = (e: MouseEvent) => {
      if (overPopover.current) return;
      const hit = marked.current.find((m) => {
        for (const r of Array.from(m.range.getClientRects())) {
          if (e.clientX >= r.left && e.clientX <= r.right && e.clientY >= r.top - 2 && e.clientY <= r.bottom + 2) return true;
        }
        return false;
      });
      onNumber.current = !!hit;
      if (hit) {
        const r = hit.range.getBoundingClientRect();
        setHover((h) => (h?.m === hit ? h : { x: r.left, y: r.bottom + 6, m: hit }));
      } else {
        // A moment to reach the popover
        window.setTimeout(() => {
          if (!overPopover.current && !onNumber.current) setHover(null);
        }, 350);
      }
    };
    const root = docRef.current;
    root?.addEventListener('mousemove', onMove);
    return () => {
      alive = false;
      window.clearTimeout(timer);
      observer.disconnect();
      root?.removeEventListener('mousemove', onMove);
      highlights?.delete(HIGHLIGHT);
      marked.current = [];
      setHover(null);
    };
  }, [active, meetingId, docRef]);

  const popover =
    hover &&
    createPortal(
      <div
        className="num-popover"
        role="tooltip"
        style={{ position: 'fixed', left: Math.max(8, Math.min(hover.x, window.innerWidth - 240)), top: Math.min(hover.y, window.innerHeight - 70) }}
        onMouseEnter={() => (overPopover.current = true)}
        onMouseLeave={() => {
          overPopover.current = false;
          setHover(null);
        }}
      >
        <span>{hover.m.issue.mixed ? 'У стенограмі записано по-різному' : 'Немає в стенограмі'}</span>
        {hover.m.issue.fix && (
          <button
            type="button"
            onClick={() => {
              const fix = hover.m.issue.fix!;
              if (replace(hover.m.blockText, hover.m.issue.text, fix)) setHover(null);
            }}
          >
            Замінити на {hover.m.issue.fix}
          </button>
        )}
      </div>,
      document.body,
    );

  return { count, popover };
}
