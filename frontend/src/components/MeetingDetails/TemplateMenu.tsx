'use client';

import { useState } from 'react';
import { useRouter } from 'next/navigation';
import { Check, ChevronDown, Plus } from 'lucide-react';
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover';
import { DocumentTemplate, DocumentVariant, estimateLabel, variantTime } from '@/lib/documentTemplates';

interface TemplateMenuProps {
  templates: DocumentTemplate[];
  variants: DocumentVariant[];
  /** The template of the document shown */
  shown: string | null;
  /** Shown when the document has no template on record (made before 0.3.8) */
  fallback: string;
  minutes: number | null;
  disabled?: boolean;
  /** Show the document already made with this template */
  onShow: (templateId: string) => void;
  /** Make a document with this template */
  onCreate: (templateId: string) => void;
}

/** The toolbar's template button: the documents made for this meeting, and other templates to make one with */
export function TemplateMenu({ templates, variants, shown, fallback, minutes, disabled, onShow, onCreate }: TemplateMenuProps) {
  const [open, setOpen] = useState(false);
  const router = useRouter();
  const name = (id: string | null) => templates.find((t) => t.id === id)?.name;
  const current = shown ?? fallback;
  const label = name(current) ?? 'Шаблон';
  const made = variants.filter((v) => templates.some((t) => t.id === v.templateId));
  const others = templates.filter((t) => !made.some((v) => v.templateId === t.id));

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <button type="button" className="tb-btn tb-side" title="Шаблон документа" aria-label={`Шаблон документа: ${label}`} disabled={disabled}>
          <svg width="15" height="15" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.4" strokeLinejoin="round" aria-hidden="true">
            <rect x="2.5" y="2.5" width="11" height="11" rx="2" />
            <path d="M2.5 6h11M6 6v7.5" />
          </svg>
          <span className="tb-label max-w-[180px] truncate">{label}</span>
          {made.length > 1 && <span className="tpl-count">{made.length}</span>}
          <ChevronDown className="tb-chevron h-3.5 w-3.5" aria-hidden="true" />
        </button>
      </PopoverTrigger>
      <PopoverContent align="start" className="tpl-menu w-[300px] p-1.5">
        <div role="menu" aria-label="Шаблон документа">
          {made.length > 0 && (
            <>
              <div className="tpl-menu-head">Створено для цього запису</div>
              {made.map((v) => {
                const on = v.templateId === current;
                return (
                  <button
                    key={v.templateId}
                    type="button"
                    role="menuitemradio"
                    aria-checked={on}
                    className="tpl-menu-item"
                    data-on={on}
                    onClick={() => {
                      setOpen(false);
                      if (!on) onShow(v.templateId);
                    }}
                  >
                    <span className="tpl-menu-icon">{on && <Check className="h-3.5 w-3.5" strokeWidth={2.5} />}</span>
                    <span className="flex-1 truncate">{name(v.templateId)}</span>
                    <span className="tpl-menu-note">{variantTime(v.createdAt)}</span>
                  </button>
                );
              })}
              <div className="tpl-menu-sep" />
            </>
          )}
          {others.length > 0 && (
            <>
              <div className="tpl-menu-head">{made.length > 0 ? 'Створити за іншим шаблоном' : 'Створити за шаблоном'}</div>
              {others.map((t) => (
                <button
                  key={t.id}
                  type="button"
                  role="menuitem"
                  className="tpl-menu-item"
                  onClick={() => {
                    setOpen(false);
                    onCreate(t.id);
                  }}
                  title={t.description}
                >
                  <span className="tpl-menu-icon tpl-menu-muted">
                    <Plus className="h-3.5 w-3.5" />
                  </span>
                  <span className="flex-1 truncate">{t.name}</span>
                  <span className="tpl-menu-note">{estimateLabel(t, minutes)}</span>
                </button>
              ))}
              <div className="tpl-menu-sep" />
            </>
          )}
          <button
            type="button"
            role="menuitem"
            className="tpl-menu-item"
            onClick={() => {
              setOpen(false);
              router.push('/settings?section=types');
            }}
          >
            <span className="tpl-menu-icon" />
            Налаштувати шаблони…
          </button>
        </div>
      </PopoverContent>
    </Popover>
  );
}
