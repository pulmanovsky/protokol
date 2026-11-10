import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import {
  DocumentTemplate,
  DocumentVariant,
  TEMPLATES_CHANGED_EVENT,
  mainTemplate,
  meetingHour,
  rememberTemplateForHour,
  templateForHour,
} from '@/lib/documentTemplates';

/** Kept for callers that still name the old default */
export const DEFAULT_MEETING_TYPE_ID = 'meeting_materials_uk';

export type MeetingType = DocumentTemplate;

const storageKey = (meetingId: string) => `meetingType:${meetingId}`;

/**
 * Document templates, the one chosen for this meeting and the documents
 * already made for it (one per template). The template preselected is the
 * one of the document shown; otherwise the one chosen for this meeting,
 * the one last used at this hour, one whose hints fit the title, or the
 * main template.
 */
export function useTemplates(meeting?: { id: string; title?: string; created_at?: string }) {
  const [templates, setTemplates] = useState<DocumentTemplate[]>([]);
  const [selectedTemplate, setSelected] = useState<string>(mainTemplate());
  const [variants, setVariants] = useState<DocumentVariant[]>([]);
  /** The template of the document shown (null: none, or made before 0.3.8) */
  const [shownTemplate, setShownTemplate] = useState<string | null>(null);
  // The document's own template wins over a suggestion that answers later
  const shownRef = useRef<string | null>(null);

  const loadTemplates = useCallback(() => {
    invoke<DocumentTemplate[]>('api_list_templates')
      .then(setTemplates)
      .catch(() => setTemplates([]));
  }, []);

  useEffect(() => {
    loadTemplates();
    window.addEventListener(TEMPLATES_CHANGED_EVENT, loadTemplates);
    window.addEventListener('focus', loadTemplates);
    return () => {
      window.removeEventListener(TEMPLATES_CHANGED_EVENT, loadTemplates);
      window.removeEventListener('focus', loadTemplates);
    };
  }, [loadTemplates]);

  const refreshVariants = useCallback(async () => {
    if (!meeting?.id) return;
    const id = meeting.id;
    try {
      const [list, shown] = await Promise.all([
        invoke<DocumentVariant[]>('api_document_variants', { meetingId: id }),
        invoke<{ templateId: string | null; transcriptChanged: boolean }>('api_shown_document', { meetingId: id, text: null }),
      ]);
      setVariants(list);
      if (id !== meeting?.id) return;
      shownRef.current = shown.templateId;
      setShownTemplate(shown.templateId);
      if (shown.templateId) setSelected(shown.templateId);
    } catch {
      setVariants([]);
    }
  }, [meeting?.id]);

  useEffect(() => {
    if (!meeting?.id) return;
    setVariants([]);
    setShownTemplate(null);
    shownRef.current = null;
    let saved: string | null = null;
    try {
      saved = localStorage.getItem(storageKey(meeting.id));
    } catch {
      saved = null;
    }
    const remembered = saved ?? templateForHour(meetingHour(meeting.title));
    if (remembered) {
      setSelected(remembered);
    } else {
      const hour = meetingHour(meeting.title) ?? 99;
      invoke<string | null>('api_suggest_template', { title: meeting.title ?? '', hour })
        .then((id) => !shownRef.current && setSelected(id ?? mainTemplate()))
        .catch(() => !shownRef.current && setSelected(mainTemplate()));
    }
    void refreshVariants();
  }, [meeting?.id, meeting?.title, meeting?.created_at, refreshVariants]);

  // A template that no longer exists (deleted in Settings): the main one
  useEffect(() => {
    if (templates.length && !templates.some((t) => t.id === selectedTemplate)) {
      const main = mainTemplate();
      setSelected(templates.some((t) => t.id === main) ? main : templates[0].id);
    }
  }, [templates, selectedTemplate]);

  const setSelectedTemplate = (id: string) => {
    setSelected(id);
    if (meeting?.id) {
      try {
        localStorage.setItem(storageKey(meeting.id), id);
      } catch {
        // per-viewer convenience only
      }
      rememberTemplateForHour(meetingHour(meeting.title), id);
    }
  };

  return { templates, selectedTemplate, setSelectedTemplate, variants, shownTemplate, refreshVariants };
}
