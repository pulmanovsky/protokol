import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';

/** «Загальна зустріч»: short protocol, the default when nothing else fits */
export const DEFAULT_MEETING_TYPE_ID = 'meeting_materials_uk';

export interface MeetingType {
  id: string;
  name: string;
  description: string;
}

const storageKey = (meetingId: string) => `meetingType:${meetingId}`;

/**
 * Meeting types and the one chosen for this meeting. A choice is remembered
 * per meeting; otherwise the app suggests a type from the title and start
 * hour, and falls back to «Загальна зустріч».
 */
export function useTemplates(meeting?: { id: string; title?: string; created_at?: string }) {
  const [templates, setTemplates] = useState<MeetingType[]>([]);
  const [selectedTemplate, setSelected] = useState<string>(DEFAULT_MEETING_TYPE_ID);

  useEffect(() => {
    invoke<MeetingType[]>('api_list_templates').then(setTemplates).catch(() => setTemplates([]));
  }, []);

  useEffect(() => {
    if (!meeting?.id) return;
    let saved: string | null = null;
    try {
      saved = localStorage.getItem(storageKey(meeting.id));
    } catch {
      saved = null;
    }
    if (saved) {
      setSelected(saved);
      return;
    }
    // The hour from the title («Запис 06.11.2026 08:30»); an import has
    // none, and its import time says nothing about the meeting
    const match = (meeting.title ?? '').match(/\b(\d{1,2}):\d{2}\b/);
    const hour = match ? Number(match[1]) : 99;
    invoke<string | null>('api_suggest_template', { title: meeting.title ?? '', hour })
      .then((id) => setSelected(id ?? DEFAULT_MEETING_TYPE_ID))
      .catch(() => setSelected(DEFAULT_MEETING_TYPE_ID));
  }, [meeting?.id, meeting?.title, meeting?.created_at]);

  const setSelectedTemplate = (id: string) => {
    setSelected(id);
    if (meeting?.id) {
      try {
        localStorage.setItem(storageKey(meeting.id), id);
      } catch {
        // per-viewer convenience only
      }
    }
  };

  return { templates, selectedTemplate, setSelectedTemplate };
}
