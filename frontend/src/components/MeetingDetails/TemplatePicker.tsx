'use client';

import { Check, Plus } from 'lucide-react';
import { useRouter } from 'next/navigation';
import { plural } from '@/lib/plural';
import { DocumentTemplate, MAIN_TEMPLATE_DEFAULT, OVERALL_BLOCK, estimateLabel, mainTemplate } from '@/lib/documentTemplates';

/** A tiny drawing of what the document looks like, from its blocks */
export function TemplateSketch({ blocks, byFields }: { blocks: string[]; byFields: boolean }) {
  const rows: React.ReactNode[] = [];
  const bar = (key: string, width: string, tone: 'green' | 'orange' | 'purple' | 'dark' | 'plain' = 'plain') => (
    <span key={key} className={`tpl-sketch-bar tpl-tone-${tone}`} style={{ width }} />
  );
  for (const b of blocks) {
    if (rows.length >= 3) break;
    if (b === 'Хід наради') {
      rows.push(
        <span key={b} className="tpl-sketch-row">
          {bar('a', '20px', 'green')}
          {bar('b', byFields ? '52%' : '68%')}
        </span>,
      );
      if (rows.length < 3) {
        rows.push(
          <span key={`${b}2`} className="tpl-sketch-row">
            {bar('a', byFields ? '16px' : '20px', byFields ? 'dark' : 'orange')}
            {bar('b', '55%')}
          </span>,
        );
      }
    } else if (b === 'Рішення' || b === 'Завдання' || b === 'Відкриті питання' || b === 'Контроль') {
      rows.push(
        <span key={b} className="tpl-sketch-row">
          <span className="tpl-sketch-check" />
          {bar('b', b === 'Завдання' ? '52%' : '64%')}
        </span>,
      );
    } else if (b === OVERALL_BLOCK) {
      rows.push(<span key={b}>{bar('a', '34px', 'dark')}</span>);
    } else if (b === 'Стенограма') {
      rows.push(
        <span key={b} className="tpl-sketch-text">
          {bar('a', '24px', 'purple')}
          {bar('b', '100%')}
          {bar('c', '86%')}
        </span>,
      );
    }
  }
  return <span className="tpl-sketch" aria-hidden="true">{rows}</span>;
}

interface TemplatePickerProps {
  templates: DocumentTemplate[];
  selected: string;
  onSelect: (id: string) => void;
  /** How it was started, for the log */
  onCreate: (how: 'picker-button' | 'picker-double-click') => void;
  /** Length of the meeting, for «≈ N хв» */
  minutes: number | null;
  /** Lines of the transcript not yet checked */
  unchecked: number;
  onCheck: () => void;
  error?: string | null;
  hasModel: boolean;
}

/** The document tab before a document exists: which template to make it with */
export function TemplatePicker({ templates, selected, onSelect, onCreate, minutes, unchecked, onCheck, error, hasModel }: TemplatePickerProps) {
  const router = useRouter();
  const main = mainTemplate();
  return (
    <div className="tpl-picker custom-scrollbar">
      <div className="tpl-picker-inner">
        <div>
          <h2 className="tpl-picker-title">Шаблон документа</h2>
          <p className="tpl-picker-sub">Документ складається з виправленої стенограми. Кожен створений варіант зберігається.</p>
        </div>
        <div className="tpl-grid" role="radiogroup" aria-label="Шаблон документа">
          {templates.map((t) => {
            const on = t.id === selected;
            const isMain = t.id === main || (!templates.some((x) => x.id === main) && t.id === MAIN_TEMPLATE_DEFAULT);
            return (
              <button
                key={t.id}
                type="button"
                role="radio"
                aria-checked={on}
                className="tpl-card"
                data-on={on}
                onClick={() => onSelect(t.id)}
                onDoubleClick={() => {
                  onSelect(t.id);
                  if (hasModel) onCreate('picker-double-click');
                }}
                title={t.description}
              >
                <TemplateSketch blocks={t.blocks} byFields={t.byFields} />
                <span className="tpl-card-name">
                  <span>{t.name}</span>
                  {isMain ? <span className="tpl-tag tpl-tag-main">основний</span> : !t.builtIn ? <span className="tpl-tag">ваш</span> : null}
                </span>
                <span className="tpl-card-desc">{t.description}</span>
                <span className="tpl-card-time">{estimateLabel(t, minutes)}</span>
                {on && (
                  <span className="tpl-card-check" aria-hidden="true">
                    <Check className="h-2.5 w-2.5" strokeWidth={3} />
                  </span>
                )}
              </button>
            );
          })}
          <button type="button" className="tpl-card tpl-card-new" onClick={() => router.push('/settings?section=types&new=1')}>
            <span className="tpl-card-plus" aria-hidden="true">
              <Plus className="h-3.5 w-3.5" />
            </span>
            <span className="tpl-card-name">Новий шаблон</span>
            <span className="tpl-card-desc">Свій набір розділів</span>
          </button>
        </div>
        {error && (
          <p role="alert" className="tpl-error">
            {error}
          </p>
        )}
        <div className="tpl-footer">
          {unchecked > 0 && (
            <>
              <span className="tpl-dot" aria-hidden="true" />
              <span className="tpl-footer-note">
                У стенограмі {unchecked} {plural(unchecked, 'рядок', 'рядки', 'рядків')} ще не перевірено.
              </span>
              <button type="button" className="tpl-link" onClick={onCheck}>
                Перевірити
              </button>
            </>
          )}
          <span className="flex-1" />
          {!hasModel && <span className="tpl-footer-note">Спершу оберіть модель у Налаштуваннях</span>}
          <button type="button" className="accent-button h-[34px] rounded-full px-[18px] text-[13.5px] font-semibold disabled:opacity-50" onClick={() => onCreate('picker-button')} disabled={!hasModel || !selected}>
            {error ? 'Спробувати ще раз' : 'Створити документ'}
          </button>
        </div>
      </div>
    </div>
  );
}
