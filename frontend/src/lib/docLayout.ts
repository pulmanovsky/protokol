import { Plugin, PluginKey, type EditorState } from '@tiptap/pm/state';
import { Decoration, DecorationSet } from '@tiptap/pm/view';
import type { Node as PMNode } from '@tiptap/pm/model';
import { NO_SOURCE } from './sourceMarks';

/**
 * The document laid out like the transcript: who spoke in the left column
 * («Олена Коваль», under it «00:10 · 2 хв»), what they said on the right,
 * and the time of each point as a small mark that plays the recording.
 *
 * Only the look changes (ProseMirror decorations): the text, the saved
 * markdown and the exported files keep «ВИСТУПИЛИ: Ім'я [00:10] · 2 хв».
 */

const SPEAKER = /^ВИСТУПИЛИ:\s*/;
const TIME = /\[(\d{1,2}:\d{2}(?::\d{2})?)\]/g;

/** Text of an inline node with the document position of every character */
function textWithPositions(content: PMNode, start: number): { text: string; at: number[] } {
  let text = '';
  const at: number[] = [];
  content.descendants((node, pos) => {
    if (node.isText && node.text) {
      for (let i = 0; i < node.text.length; i++) at.push(start + pos + i);
      text += node.text;
    }
    return true;
  });
  return { text, at };
}

function inline(at: number[], from: number, to: number, attrs: Record<string, string>): Decoration | null {
  if (to <= from || from < 0 || to > at.length) return null;
  return Decoration.inline(at[from], at[to - 1] + 1, attrs);
}

/** «[00:14]» → a time mark (the brackets hidden) */
function timeMarks(content: PMNode, start: number, out: Decoration[]) {
  const { text, at } = textWithPositions(content, start);
  for (const m of text.matchAll(TIME)) {
    const i = m.index ?? 0;
    const time = m[1];
    for (const d of [
      inline(at, i, i + 1, { class: 'doc-hide' }),
      inline(at, i + 1, i + 1 + time.length, { class: 'doc-time', title: 'Слухати з цього місця' }),
      inline(at, i + 1 + time.length, i + 2 + time.length, { class: 'doc-hide' }),
    ])
      if (d) out.push(d);
  }
}

function build(doc: PMNode, colorOf: (name: string) => string | undefined): DecorationSet {
  const top = doc.firstChild;
  if (!top) return DecorationSet.empty;
  const out: Decoration[] = [];
  const blocks: { pos: number; node: PMNode }[] = [];
  top.forEach((node, offset) => blocks.push({ pos: 1 + offset, node }));

  // Labels: a speaker («ВИСТУПИЛИ: …») or a section heading; each keeps the
  // rows of what follows it until the next label
  const kind = blocks.map(({ node }, i) => {
    const content = node.firstChild;
    if (!content) return 'text';
    const text = content.textContent.trim();
    if (content.type.name === 'heading') {
      if (i === 0 && content.attrs.level === 1) return 'hidden'; // the title is in the header
      if (text === 'Хід наради') return 'hidden';
      return 'label';
    }
    if (content.type.name === 'paragraph' && SPEAKER.test(text)) return 'speaker';
    if (content.type.name === 'paragraph' && /^(Дата|Тривалість|Учасники)[\s(:]/.test(text) && i < 3) return 'facts';
    return 'text';
  });

  blocks.forEach(({ pos, node }, i) => {
    const k = kind[i];
    const content = node.firstChild;
    if (k === 'facts') {
      out.push(Decoration.node(pos, pos + node.nodeSize, { class: 'doc-facts' }));
      return;
    }
    if (k === 'hidden') {
      out.push(Decoration.node(pos, pos + node.nodeSize, { class: 'doc-hidden' }));
      return;
    }
    if (k === 'label' || k === 'speaker') {
      let span = 0;
      for (let j = i + 1; j < blocks.length && kind[j] === 'text'; j++) span++;
      out.push(Decoration.node(pos, pos + node.nodeSize, { class: `doc-who${k === 'label' ? ' doc-who-heading' : ''}`, style: span > 1 ? `grid-row: span ${span}` : '' }));
      if (k === 'speaker' && content) {
        // «ВИСТУПИЛИ: Олена Коваль [00:10] · 2 хв»
        const { text, at } = textWithPositions(content, pos + 2);
        const label = text.match(SPEAKER)?.[0].length ?? 0;
        const bracket = text.indexOf('[', label);
        const nameEnd = bracket >= 0 ? bracket : text.length;
        const name = text.slice(label, nameEnd).trim();
        const color = colorOf(name);
        for (const d of [
          inline(at, 0, label, { class: 'doc-hide' }),
          inline(at, label, nameEnd, { class: 'doc-name', ...(color ? { style: `color: ${color}` } : {}) }),
          bracket >= 0 ? inline(at, bracket, text.length, { class: 'doc-meta' }) : null,
        ])
          if (d) out.push(d);
        if (bracket >= 0) {
          const close = text.indexOf(']', bracket);
          if (close > bracket) {
            for (const d of [inline(at, bracket, bracket + 1, { class: 'doc-hide' }), inline(at, close, close + 1, { class: 'doc-hide' })]) if (d) out.push(d);
          }
        }
      }
      return;
    }
    // What was said: the time marks, nested points too; a point whose source
    // was not found (⟨?⟩) tinted amber. «Не враховано» lists remarks left out
    // on purpose: an unpinned quote there is not a point without a source.
    node.descendants((child, p) => {
      if (child.isTextblock) {
        const at = pos + 1 + p;
        timeMarks(child, at + 1, out);
        if (child.textContent.split('Не враховано:')[0].includes(NO_SOURCE)) {
          out.push(Decoration.node(at, at + child.nodeSize, { class: 'no-source', title: 'Без джерела: у словах доповідача цього не знайдено — перевірте' }));
        }
        return false;
      }
      return true;
    });
  });
  return DecorationSet.create(doc, out);
}

/** Registers the layout on a BlockNote editor; returns how to take it off */
export function registerDocLayout(editor: any, colorOf: (name: string) => string | undefined): { refresh: () => void; dispose: () => void } {
  const tiptap = editor?._tiptapEditor;
  if (!tiptap?.registerPlugin) return { refresh: () => {}, dispose: () => {} };
  const key = new PluginKey<DecorationSet>('docLayout');
  const plugin = new Plugin<DecorationSet>({
    key,
    state: {
      init: (_, state: EditorState) => build(state.doc, colorOf),
      apply: (tr, old) => (tr.docChanged || tr.getMeta(key) ? build(tr.doc, colorOf) : old),
    },
    props: {
      decorations(state) {
        return key.getState(state);
      },
    },
  });
  tiptap.registerPlugin(plugin);
  return {
    refresh: () => {
      const view = tiptap.view;
      if (view && !view.isDestroyed) view.dispatch(view.state.tr.setMeta(key, true));
    },
    dispose: () => {
      try {
        tiptap.unregisterPlugin(key);
      } catch {
        /* the editor is gone */
      }
    },
  };
}
