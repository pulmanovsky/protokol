"use client";

import { useState, useEffect, useCallback, useRef, forwardRef, useImperativeHandle } from 'react';
import dynamic from 'next/dynamic';
import { Summary, SummaryDataResponse, SummaryFormat, BlockNoteBlock } from '@/types';
import { AISummary } from './index';
import { Block } from '@blocknote/core';
import { useCreateBlockNote } from '@blocknote/react';
import { BlockNoteView } from '@blocknote/shadcn';
import { blocksToMarkdownSafely } from '@/lib/blocknote-markdown';
import { registerDocLayout } from '@/lib/docLayout';
import "@blocknote/shadcn/style.css";

// Dynamically import BlockNote Editor to avoid SSR issues
const Editor = dynamic(() => import('../BlockNoteEditor/Editor'), { ssr: false });

interface BlockNoteSummaryViewProps {
  summaryData: SummaryDataResponse | Summary | null;
  onSave?: (data: { markdown?: string; summary_json?: BlockNoteBlock[] }) => Promise<void>;
  onSummaryChange?: (summary: Summary) => void;
  status?: 'idle' | 'processing' | 'summarizing' | 'regenerating' | 'completed' | 'error';
  error?: string | null;
  onRegenerateSummary?: () => void;
  meeting?: {
    id: string;
    title: string;
    created_at: string;
  };
  onDirtyChange?: (isDirty: boolean) => void;
  /** Speakers' colours as in the transcript */
  speakerColors?: Map<string, string>;
}

export interface BlockNoteSummaryViewRef {
  saveSummary: () => Promise<void>;
  getMarkdown: () => Promise<string>;
  isDirty: boolean;
  /** Adds a bullet after the block whose text holds `containing` */
  insertBulletAfter?: (containing: string, text: string) => boolean;
  /** Takes `part` out of the block whose text holds it (the block goes when only its label is left) */
  removeFromBlock?: (part: string) => boolean;
  /** In the block with this text, `from` becomes `to` (its formatting kept) */
  replaceInBlock?: (blockText: string, from: string, to: string) => boolean;
  undo?: () => void;
  redo?: () => void;
}

/** The plain text of a block */
function blockText(block: any): string {
  return Array.isArray(block?.content) ? block.content.map((c: any) => (typeof c?.text === 'string' ? c.text : '')).join('') : '';
}

/** Every block, nested ones too */
function allBlocks(blocks: any[]): any[] {
  return blocks.flatMap((b) => [b, ...allBlocks(b?.children ?? [])]);
}

// Format detection helper
function detectSummaryFormat(data: any): { format: SummaryFormat; data: any } {
  if (!data) {
    return { format: 'legacy', data: null };
  }

  // Priority 1: BlockNote format (has summary_json)
  if (data.summary_json && Array.isArray(data.summary_json)) {
    console.log('✅ FORMAT: BLOCKNOTE (summary_json exists)');
    return { format: 'blocknote', data };
  }

  // Priority 2: Markdown format
  if (data.markdown && typeof data.markdown === 'string') {
    console.log('✅ FORMAT: MARKDOWN (will parse to BlockNote)');
    return { format: 'markdown', data };
  }

  // Priority 3: Legacy JSON
  const hasLegacyStructure = data.MeetingName || Object.keys(data).some(key =>
    typeof data[key] === 'object' && data[key]?.title && data[key]?.blocks
  );

  if (hasLegacyStructure) {
    console.log('✅ FORMAT: LEGACY (custom JSON)');
    return { format: 'legacy', data };
  }

  return { format: 'legacy', data: null };
}

export const BlockNoteSummaryView = forwardRef<BlockNoteSummaryViewRef, BlockNoteSummaryViewProps>(({
  summaryData,
  onSave,
  onSummaryChange,
  status = 'idle',
  error = null,
  onRegenerateSummary,
  meeting,
  onDirtyChange,
  speakerColors,
}, ref) => {
  const { format, data } = detectSummaryFormat(summaryData);
  const [isDirty, setIsDirty] = useState(false);
  const [currentBlocks, setCurrentBlocks] = useState<Block[]>([]);
  const [isSaving, setIsSaving] = useState(false);
  const isContentLoaded = useRef(false);

  // Create BlockNote editor for markdown parsing
  const editor = useCreateBlockNote({
    initialContent: undefined
  });

  // The transcript's layout: names on the left, time marks that play
  const colorsRef = useRef(speakerColors);
  colorsRef.current = speakerColors;
  const colorOf = useCallback((name: string) => colorsRef.current?.get(name), []);
  const layouts = useRef<{ refresh: () => void }[]>([]);
  const innerEditor = useRef<any>(null);
  const withLayout = useCallback(
    (target: any) => {
      const layout = registerDocLayout(target, colorOf);
      layouts.current.push(layout);
      return () => {
        layout.dispose();
        layouts.current = layouts.current.filter((l) => l !== layout);
      };
    },
    [colorOf],
  );
  useEffect(() => (editor ? withLayout(editor) : undefined), [editor, withLayout]);
  useEffect(() => {
    layouts.current.forEach((l) => l.refresh());
  }, [speakerColors]);
  const onInnerEditor = useCallback(
    (inner: any) => {
      innerEditor.current = inner;
      const dispose = withLayout(inner);
      return () => {
        dispose();
        if (innerEditor.current === inner) innerEditor.current = null;
      };
    },
    [withLayout],
  );

  // Parse markdown to blocks when format is markdown
  useEffect(() => {
    if (format === 'markdown' && data?.markdown && editor) {
      const loadMarkdown = async () => {
        try {
          console.log('📝 Parsing markdown to BlockNote blocks...');
          const blocks = await editor.tryParseMarkdownToBlocks(data.markdown);
          editor.replaceBlocks(editor.document, blocks);
          console.log('✅ Markdown parsed successfully');

          // Delay to ensure editor has finished rendering before allowing onChange
          setTimeout(() => {
            isContentLoaded.current = true;
          }, 100);
        } catch (err) {
          console.error('❌ Failed to parse markdown:', err);
        }
      };
      loadMarkdown();
    }
  }, [format, data?.markdown, editor]);

  // Set content loaded flag for blocknote format
  useEffect(() => {
    if (format === 'blocknote' && data?.summary_json) {
      // Delay to ensure editor has finished rendering
      setTimeout(() => {
        isContentLoaded.current = true;
      }, 100);
    }
  }, [format, data?.summary_json]);

  const handleEditorChange = useCallback((blocks: Block[]) => {
    // Only set dirty flag if content has finished loading
    if (isContentLoaded.current) {
      setCurrentBlocks(blocks);
      setIsDirty(true);
    }
  }, []);

  // Notify parent of dirty state changes
  useEffect(() => {
    if (onDirtyChange) {
      onDirtyChange(isDirty);
    }
  }, [isDirty, onDirtyChange]);

  const handleSave = useCallback(async () => {
    if (!onSave || !isDirty) return;

    setIsSaving(true);
    try {
      console.log('💾 Saving BlockNote content...');

      // Generate markdown from current blocks; preserve BlockNote JSON even if markdown conversion fails.
      const markdownResult = await blocksToMarkdownSafely(editor, currentBlocks, {
        source: 'BlockNoteSummaryView.handleSave',
      });

      const saveData: { markdown?: string; summary_json?: BlockNoteBlock[] } = {
        summary_json: currentBlocks as unknown as BlockNoteBlock[]
      };

      if (markdownResult.markdown !== undefined) {
        saveData.markdown = markdownResult.markdown;
      }

      await onSave(saveData);
      setIsDirty(false);
      console.log('✅ Save successful');
    } catch (error) {
      console.error('❌ Save failed:', error);
      throw error;
    } finally {
      setIsSaving(false);
    }
  }, [onSave, isDirty, currentBlocks, editor]);

  // Expose methods to parent via ref
  useImperativeHandle(ref, () => ({
    undo: () => (format === 'blocknote' ? innerEditor.current : editor)?.undo?.(),
    redo: () => (format === 'blocknote' ? innerEditor.current : editor)?.redo?.(),
    saveSummary: handleSave,
    getMarkdown: async () => {
      try {
        console.log('🔍 getMarkdown called, format:', format);
        console.log('🔍 currentBlocks length:', currentBlocks.length);
        console.log('🔍 data:', data);

        // For markdown format - use the main editor
        if (format === 'markdown' && editor) {
          console.log('📝 Using markdown editor, blocks:', editor.document.length);
          const markdownResult = await blocksToMarkdownSafely(editor, editor.document, {
            source: 'BlockNoteSummaryView.getMarkdown.markdown',
            fallbackMarkdown: data?.markdown,
          });
          console.log('📝 Generated markdown length:', markdownResult.markdown?.length || 0);
          return markdownResult.markdown || '';
        }

        // For blocknote format - use currentBlocks state
        if (format === 'blocknote') {
          console.log('📝 BlockNote format, currentBlocks:', currentBlocks.length);
          const blocks = currentBlocks.length > 0
            ? currentBlocks
            : (data?.summary_json as unknown as Block[] | undefined) || [];

          if (blocks.length > 0 && editor) {
            const markdownResult = await blocksToMarkdownSafely(editor, blocks, {
              source: 'BlockNoteSummaryView.getMarkdown.blocknote',
              fallbackMarkdown: data?.markdown,
            });
            console.log('📝 Generated markdown from blocks, length:', markdownResult.markdown?.length || 0);
            return markdownResult.markdown || '';
          }
          // Fallback: if we have the original data with markdown
          if (data?.markdown) {
            console.log('📝 Using fallback markdown from data');
            return data.markdown;
          }
        }

        // For legacy format - return empty (handled by parent)
        console.warn('⚠️ Cannot generate markdown for legacy format, returning empty');
        return '';
      } catch (err) {
        console.error('❌ Failed to generate markdown:', err);
        return '';
      }
    },
    isDirty,
    insertBulletAfter: (containing: string, text: string) => {
      if (!editor) return false;
      const target = allBlocks(editor.document as any[]).find((b) => blockText(b).includes(containing));
      if (!target) return false;
      editor.insertBlocks([{ type: 'bulletListItem', content: text } as any], target, 'before');
      return true;
    },
    replaceInBlock: (text: string, from: string, to: string) => {
      if (!editor) return false;
      const blocks = allBlocks(editor.document as any[]);
      const target = blocks.find((b) => blockText(b) === text) ?? blocks.find((b) => blockText(b).includes(from) && text.includes(blockText(b).slice(0, 20)));
      if (!target || !Array.isArray((target as any).content)) return false;
      const content = (target as any).content as any[];
      const i = content.findIndex((c) => typeof c?.text === 'string' && c.text.includes(from));
      if (i < 0) return false;
      const next = content.map((c, k) => (k === i ? { ...c, text: c.text.replace(from, to) } : c));
      editor.updateBlock(target, { content: next } as any);
      return true;
    },
    removeFromBlock: (part: string) => {
      if (!editor) return false;
      const target = allBlocks(editor.document as any[]).find((b) => blockText(b).includes(part));
      if (!target) return false;
      const rest = blockText(target).replace(part, '').replace(/\s{2,}/g, ' ').trim();
      if (/^[^:]{1,40}:\s*$/.test(rest)) editor.removeBlocks([target]);
      else editor.updateBlock(target, { content: rest } as any);
      return true;
    },
  }), [handleSave, isDirty, editor, format, currentBlocks, data]);

  // Render legacy format
  if (format === 'legacy') {
    console.log('🎨 Rendering LEGACY format');
    return (
      <AISummary
        summary={summaryData as Summary}
        status={status}
        error={error}
        onSummaryChange={onSummaryChange || (() => { })}
        onRegenerateSummary={onRegenerateSummary || (() => { })}
        meeting={meeting}
      />
    );
  }

  // Render BlockNote format (has summary_json)
  if (format === 'blocknote') {
    console.log('🎨 Rendering BLOCKNOTE format (direct)');
    return (
      <div className="flex flex-col w-full">
        <div className="w-full">
          <Editor
            initialContent={data.summary_json}
            onChange={(blocks) => {
              console.log('📝 Editor blocks changed:', blocks.length);
              handleEditorChange(blocks);
            }}
            editable={true}
            onEditor={onInnerEditor}
          />
        </div>
      </div>
    );
  }

  // Render Markdown format (parse and display in BlockNote)
  if (format === 'markdown') {
    console.log('🎨 Rendering MARKDOWN format (parsed to BlockNote)');
    return (
      <div className="flex flex-col w-full">
        <div className="w-full">
          <BlockNoteView
            editor={editor}
            editable={true}
            sideMenu={false}
            onChange={() => {
              if (isContentLoaded.current) {
                handleEditorChange(editor.document);
              }
            }}
            theme="light"
          />
        </div>
      </div>
    );
  }

  return null;
});

BlockNoteSummaryView.displayName = 'BlockNoteSummaryView';
