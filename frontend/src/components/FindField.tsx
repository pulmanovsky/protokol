'use client';

import { RefObject, useCallback, useEffect, useRef, useState } from 'react';
import { ChevronDown, ChevronUp, Search, X } from 'lucide-react';

/**
 * Пошук у тексті, як ⌘F у браузері: підсвічує всі збіги (слова, частини слів,
 * кілька слів поспіль, імена спікерів), показує «3 з 12», Enter і ⇧Enter
 * (або ⌘G і ⇧⌘G) переходять між збігами. Текст на сторінці не змінюється:
 * підсвічування малюється через CSS Custom Highlight API.
 */

const ALL = 'find-all';
const CURRENT = 'find-current';

type Highlights = { set: (name: string, h: unknown) => void; delete: (name: string) => void };
const registry = (): Highlights | null => {
  const css = (globalThis as unknown as { CSS?: { highlights?: Highlights } }).CSS;
  return css?.highlights && typeof (globalThis as unknown as { Highlight?: unknown }).Highlight === 'function' ? css.highlights : null;
};
const makeHighlight = (ranges: Range[]) => new (globalThis as unknown as { Highlight: new (...r: Range[]) => unknown }).Highlight(...ranges);

/** Letters compared without case; the apostrophes ’ ʼ ' count as one */
const fold = (s: string) => s.toLowerCase().replace(/[’ʼ`]/g, "'");

/** Text pieces of one line or paragraph are searched together, never across two of them */
const BLOCK = '.tv-text, .tv-name, .tv-name-pill, .tv-time, .tv-note, p, li, h1, h2, h3, h4, h5, h6, td, th';

function findRanges(root: HTMLElement, query: string): Range[] {
  const q = fold(query.trim().replace(/\s+/g, ' '));
  if (!q) return [];
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT, {
    acceptNode: (n) => {
      const el = n.parentElement;
      if (!el || !n.nodeValue) return NodeFilter.FILTER_REJECT;
      if (el.closest('[aria-hidden="true"], script, style, input, textarea, [data-find-skip]')) return NodeFilter.FILTER_REJECT;
      return NodeFilter.FILTER_ACCEPT;
    },
  });
  const ranges: Range[] = [];
  let nodes: Text[] = [];
  let block: Element | null = null;
  const flush = () => {
    if (!nodes.length) return;
    // One string per block; spaces collapsed as on screen, with each character's place kept
    let text = '';
    const where: { node: Text; offset: number }[] = [];
    for (const node of nodes) {
      const v = node.nodeValue ?? '';
      for (let i = 0; i < v.length; i++) {
        const ch = /\s/.test(v[i]) ? ' ' : v[i];
        if (ch === ' ' && (text.endsWith(' ') || text === '')) continue;
        text += ch;
        where.push({ node, offset: i });
      }
    }
    const hay = fold(text);
    let from = 0;
    for (;;) {
      const at = hay.indexOf(q, from);
      if (at < 0) break;
      const start = where[at];
      const end = where[at + q.length - 1];
      const r = document.createRange();
      r.setStart(start.node, start.offset);
      r.setEnd(end.node, end.offset + 1);
      ranges.push(r);
      from = at + q.length;
    }
    nodes = [];
  };
  for (let n = walker.nextNode() as Text | null; n; n = walker.nextNode() as Text | null) {
    const b = n.parentElement?.closest(BLOCK) ?? null;
    if (b !== block) {
      flush();
      block = b;
    }
    nodes.push(n);
  }
  flush();
  return ranges;
}

/**
 * WebKit keeps painting the ranges of the previous query after the registry
 * changes; a one-frame opacity change makes it repaint the text
 */
function repaint(root: HTMLElement | null) {
  const el = (root?.closest('.tv-surface, .doc-surface') as HTMLElement | null) ?? root;
  if (!el) return;
  el.style.opacity = '0.99';
  requestAnimationFrame(() =>
    requestAnimationFrame(() => {
      el.style.opacity = '';
    }),
  );
}

function scrollToRange(range: Range) {
  const el = range.startContainer.parentElement;
  if (!el) return;
  const rect = range.getBoundingClientRect();
  const view = el.closest('.tv-surface, .doc-surface') as HTMLElement | null;
  if (view) {
    const box = view.getBoundingClientRect();
    if (rect.top < box.top + 40 || rect.bottom > box.bottom - 40) {
      view.scrollTo({ top: view.scrollTop + rect.top - box.top - box.height / 2 });
    }
  } else {
    el.scrollIntoView({ block: 'center' });
  }
}

interface FindFieldProps {
  /** Where to search */
  target: RefObject<HTMLElement>;
  /** «Пошук у стенограмі», «Пошук у документі» */
  label: string;
  disabled?: boolean;
}

export function FindField({ target, label, disabled }: FindFieldProps) {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState('');
  const [ranges, setRanges] = useState<Range[]>([]);
  const [index, setIndex] = useState(0);
  const input = useRef<HTMLInputElement>(null);
  const indexRef = useRef(0);
  indexRef.current = index;

  const paint = useCallback((all: Range[], current: number) => {
    const reg = registry();
    if (!reg) return;
    if (!all.length) {
      reg.delete(ALL);
      reg.delete(CURRENT);
    } else {
      reg.set(ALL, makeHighlight(all));
      reg.set(CURRENT, makeHighlight([all[current]]));
    }
    repaint(target.current);
  }, [target]);

  const clear = useCallback(() => {
    const reg = registry();
    reg?.delete(ALL);
    reg?.delete(CURRENT);
    repaint(target.current);
  }, [target]);

  /** Looks again: after typing, after an edit, after more lines load */
  const refresh = useCallback(
    (jump: boolean) => {
      const root = target.current;
      if (!root || !open) return;
      const found = findRanges(root, query);
      let at = 0;
      if (found.length) {
        if (jump) {
          // The first match from the top of what is on screen, as in a browser
          const view = root.closest('.tv-surface, .doc-surface') ?? root;
          const top = (view as HTMLElement).getBoundingClientRect().top;
          at = Math.max(0, found.findIndex((r) => r.getBoundingClientRect().bottom >= top));
        } else {
          at = Math.min(indexRef.current, found.length - 1);
        }
      }
      setRanges(found);
      setIndex(at);
      paint(found, at);
      if (jump && found.length) scrollToRange(found[at]);
      if (!registry() && jump && found.length) {
        // Without highlights (an old WebKit) at least the current match is selected
        const sel = window.getSelection();
        sel?.removeAllRanges();
        sel?.addRange(found[at]);
        input.current?.focus();
      }
    },
    [target, open, query, paint],
  );

  useEffect(() => {
    if (!open) return;
    const t = window.setTimeout(() => refresh(true), 120);
    return () => window.clearTimeout(t);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [query, open]);

  // The text changes under the search (an edit, the rest of the lines): the matches follow it
  useEffect(() => {
    const root = target.current;
    if (!open || !root || !query.trim()) return;
    let t = 0;
    const obs = new MutationObserver(() => {
      window.clearTimeout(t);
      t = window.setTimeout(() => refresh(false), 250);
    });
    obs.observe(root, { subtree: true, childList: true, characterData: true });
    return () => {
      obs.disconnect();
      window.clearTimeout(t);
    };
  }, [open, query, target, refresh]);

  useEffect(() => () => clear(), [clear]);

  const go = (step: number) => {
    if (!ranges.length) return;
    const at = (index + step + ranges.length) % ranges.length;
    setIndex(at);
    paint(ranges, at);
    scrollToRange(ranges[at]);
  };

  const close = () => {
    setOpen(false);
    setQuery('');
    setRanges([]);
    clear();
  };

  // ⌘F from the menu (MenuBridge) clicks the button or focuses the open field
  if (!open) {
    return (
      <button
        type="button"
        className="tv-icon tv-find"
        aria-label={label}
        title={`${label} (⌘F)`}
        disabled={disabled}
        onClick={() => setOpen(true)}
      >
        <Search className="h-4 w-4" />
      </button>
    );
  }

  const q = query.trim();
  return (
    <div className="find-field" role="search">
      <Search className="find-field-icon h-3.5 w-3.5" aria-hidden="true" />
      <input
        ref={input}
        autoFocus
        className="tv-search find-input"
        placeholder="Пошук"
        aria-label={label}
        value={query}
        onChange={(e) => setQuery(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === 'Enter') {
            e.preventDefault();
            go(e.shiftKey ? -1 : 1);
          } else if (e.key === 'g' && e.metaKey) {
            e.preventDefault();
            go(e.shiftKey ? -1 : 1);
          } else if (e.key === 'Escape') {
            e.preventDefault();
            close();
          }
        }}
      />
      {q && (
        <span className="find-count" aria-live="polite">
          {ranges.length ? `${index + 1} з ${ranges.length}` : 'Не знайдено'}
        </span>
      )}
      <button type="button" className="find-step" aria-label="Попередній збіг" title="Попередній (⇧Enter)" disabled={!ranges.length} onClick={() => go(-1)}>
        <ChevronUp className="h-3.5 w-3.5" />
      </button>
      <button type="button" className="find-step" aria-label="Наступний збіг" title="Наступний (Enter)" disabled={!ranges.length} onClick={() => go(1)}>
        <ChevronDown className="h-3.5 w-3.5" />
      </button>
      <button type="button" className="find-step" aria-label="Закрити пошук" title="Закрити (Esc)" onClick={close}>
        <X className="h-3.5 w-3.5" />
      </button>
    </div>
  );
}
