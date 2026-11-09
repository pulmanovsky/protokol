'use client';

import { RefObject, useEffect, useRef, useState } from 'react';
import { Copy } from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import { toast } from '@/lib/notice';
import type { BlockNoteSummaryViewRef } from '@/components/AISummary/BlockNoteSummaryView';
import { documentSections, sectionPlain, type DocSection } from '@/lib/docSections';

interface Item {
  kind: DocSection['kind'];
  ordinal: number;
  name: string;
  top: number;
}

/** A small copy button in the paper's right margin for each person's block and the
 * overall summary; seen while the pointer is over that part, or on focus */
export function DocSectionCopy({ summaryRef }: { summaryRef: RefObject<BlockNoteSummaryViewRef> }) {
  const anchor = useRef<HTMLSpanElement>(null);
  const [items, setItems] = useState<Item[]>([]);
  const [hover, setHover] = useState(-1);
  const itemsRef = useRef(items);
  itemsRef.current = items;

  useEffect(() => {
    const paper = anchor.current?.parentElement;
    if (!paper) return;
    let frame = 0;
    const measure = () => {
      frame = 0;
      const box = paper.getBoundingClientRect();
      const found: Item[] = [];
      let speakers = 0;
      paper.querySelectorAll('.bn-block-outer').forEach((outer) => {
        const content = outer.querySelector('.bn-block-content');
        const text = (content?.textContent ?? '').trim();
        const top = Math.round((content ?? outer).getBoundingClientRect().top - box.top);
        const person = text.match(/^ВИСТУПИЛИ:\s*(.+?)(?:\s*\[.*)?$/);
        if (person) found.push({ kind: 'speaker', ordinal: speakers++, name: person[1], top });
        else if (text === 'Загальний підсумок') found.push({ kind: 'overall', ordinal: 0, name: text, top });
      });
      setItems((was) => (JSON.stringify(was) === JSON.stringify(found) ? was : found));
    };
    const later = () => {
      if (!frame) frame = window.requestAnimationFrame(measure);
    };
    later();
    const changes = new MutationObserver(later);
    changes.observe(paper, { childList: true, subtree: true, characterData: true });
    const sizes = new ResizeObserver(later);
    sizes.observe(paper);
    const move = (e: MouseEvent) => {
      const y = e.clientY - paper.getBoundingClientRect().top;
      let at = -1;
      itemsRef.current.forEach((item, i) => {
        if (item.top - 6 <= y) at = i;
      });
      setHover(at);
    };
    const leave = () => setHover(-1);
    paper.addEventListener('mousemove', move);
    paper.addEventListener('mouseleave', leave);
    return () => {
      if (frame) window.cancelAnimationFrame(frame);
      changes.disconnect();
      sizes.disconnect();
      paper.removeEventListener('mousemove', move);
      paper.removeEventListener('mouseleave', leave);
    };
  }, []);

  const copy = async (item: Item, withTimes: boolean) => {
    try {
      const markdown = (await summaryRef.current?.getMarkdown()) ?? '';
      const section = documentSections(markdown).filter((s) => s.kind === item.kind)[item.ordinal];
      if (!section) throw new Error('немає тексту');
      await invoke('api_copy_text', { text: sectionPlain(section, withTimes) });
      toast(`Скопійовано: ${section.title}`);
    } catch (e) {
      toast.error(`Не вдалося скопіювати: ${e}`);
    }
  };

  return (
    <>
      <span ref={anchor} hidden />
      {items.map((item, i) => (
        <button
          key={`${item.kind}-${item.ordinal}`}
          type="button"
          className={`doc-copy${hover === i ? ' doc-copy-on' : ''}`}
          style={{ top: item.top }}
          aria-label={`Копіювати: ${item.name}`}
          title={`Копіювати: ${item.name} · з ⌥ — із часом`}
          onClick={(e) => void copy(item, e.altKey)}
        >
          <Copy className="h-3.5 w-3.5" aria-hidden="true" />
        </button>
      ))}
    </>
  );
}
