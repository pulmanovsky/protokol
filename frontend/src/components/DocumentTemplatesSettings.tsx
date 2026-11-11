'use client';
import { confirmNative } from '@/lib/confirm';

import { useCallback, useEffect, useRef, useState } from 'react';
import { useSearchParams } from 'next/navigation';
import { invoke } from '@tauri-apps/api/core';
import { toast } from '@/lib/notice';
import { ChevronDown, ChevronRight, FileDown, GripVertical, Plus, X } from 'lucide-react';
import { plural } from '@/lib/plural';
import { Switch } from '@/components/ui/switch';
import { Dialog, DialogContent, DialogTitle } from '@/components/ui/dialog';
import { SettingsGroup, SettingsRow } from '@/components/settings/SettingsGroup';
import { useConfig } from '@/contexts/ConfigContext';
import {
  DocumentTemplate,
  MAIN_TEMPLATE_DEFAULT,
  OVERALL_BLOCK,
  TEMPLATES_CHANGED_EVENT,
  mainTemplate,
  remembersByHour,
  setMainTemplate,
  setRemembersByHour,
} from '@/lib/documentTemplates';

interface MeetingPart {
  title: string;
  cues?: string[];
  role?: string | null;
}

interface TemplateForm {
  id: string | null;
  name: string;
  description: string;
  reportFields: string[];
  parts: MeetingPart[];
  /** What the document has, in order */
  blocks: string[];
  titleHints: string[];
  hours: number[];
  controls: string | null;
  groupBySpeaker: boolean;
  /** 'points' («Короткими пунктами») or 'fields' («За полями») */
  style: string;
  detailed: boolean;
  wishes: string;
  builtIn: boolean;
}

/** The blocks in the main list, with what each one is */
const MAIN_BLOCKS: { value: string; label: string; help?: string }[] = [
  { value: 'Хід наради', label: 'Підсумок кожного доповідача' },
  { value: 'Завдання', label: 'Завдання', help: 'хто, що, до коли' },
  { value: 'Відкриті питання', label: 'Відкриті питання' },
  { value: OVERALL_BLOCK, label: 'Загальний підсумок' },
  { value: 'Рішення', label: 'Рішення' },
  { value: 'Стенограма', label: 'Повна стенограма в кінці' },
];
const MAIN_VALUES = MAIN_BLOCKS.map((b) => b.value);

const commaList = (s: string) => s.split(',').map((x) => x.trim()).filter(Boolean);

/**
 * Settings → «Шаблони документа»: the list, the main template, and an
 * editor sheet with a sample of the document. Built-in templates stay as
 * they are; a change is saved as the user's own template.
 */
export function DocumentTemplatesSettings() {
  const [templates, setTemplates] = useState<DocumentTemplate[]>([]);
  const [form, setForm] = useState<TemplateForm | null>(null);
  const [main, setMain] = useState(MAIN_TEMPLATE_DEFAULT);
  const [byHour, setByHour] = useState(true);
  // One switch for the automatic document (the old «підсумок одразу після запису» switch is folded into it)
  const { toggleIsAutoSummary } = useConfig();
  const [autoDocument, setAutoDocument] = useState(false);
  useEffect(() => {
    invoke<boolean>('get_auto_document').then(setAutoDocument).catch(() => {});
  }, []);
  const changeAutoDocument = async (enabled: boolean) => {
    setAutoDocument(enabled);
    try {
      await invoke('set_auto_document', { enabled });
      toggleIsAutoSummary(false);
    } catch {
      setAutoDocument(!enabled);
      toast.error('Не вдалося зберегти налаштування');
    }
  };
  const searchParams = useSearchParams();

  const load = useCallback(async () => {
    setTemplates(await invoke<DocumentTemplate[]>('api_list_templates').catch(() => []));
  }, []);

  useEffect(() => {
    void load();
    setMain(mainTemplate());
    setByHour(remembersByHour());
  }, [load]);

  const open = async (id: string) => {
    try {
      setForm(await invoke<TemplateForm>('api_get_meeting_type', { id }));
    } catch (e) {
      toast.error(`Не вдалося відкрити шаблон: ${e}`);
    }
  };

  const createNew = useCallback(async () => {
    try {
      const base = await invoke<TemplateForm>('api_get_meeting_type', { id: MAIN_TEMPLATE_DEFAULT });
      setForm({ ...base, id: null, builtIn: false, name: 'Новий шаблон', description: '', titleHints: [], hours: [], wishes: '' });
    } catch (e) {
      toast.error(`${e}`);
    }
  }, []);

  // «Новий шаблон» on the document tab opens the editor here
  const askedNew = searchParams.get('new') === '1';
  useEffect(() => {
    if (askedNew) void createNew();
  }, [askedNew, createNew]);

  const importFile = async () => {
    try {
      const done = await invoke<[number, number] | null>('api_import_pack', { path: null });
      if (!done) return;
      const [count, terms] = done;
      await load();
      window.dispatchEvent(new Event(TEMPLATES_CHANGED_EVENT));
      toast.success(
        terms > 0
          ? `Імпортовано: ${count} ${plural(count, 'шаблон', 'шаблони', 'шаблонів')}, ${terms} ${plural(terms, 'новий термін', 'нові терміни', 'нових термінів')}`
          : `Імпортовано: ${count} ${plural(count, 'шаблон', 'шаблони', 'шаблонів')}`,
      );
    } catch (e) {
      toast.error(`${e}`);
    }
  };

  return (
    <div className="flex flex-col gap-5">
      <SettingsGroup
        title="Створення"
        help="Документ створюється на цьому Mac за основним шаблоном. Для ранкового запису пропонується шаблон попереднього ранкового."
      >
        <SettingsRow
          label={
            <span className="flex flex-col min-w-0">
              <span>Створювати документ автоматично</span>
              <span className="text-xs text-[color:var(--muted-text)]">Після запису та розпізнавання спікерів</span>
            </span>
          }
        >
          <Switch checked={autoDocument} onCheckedChange={(on) => void changeAutoDocument(on)} aria-label="Створювати документ автоматично" />
        </SettingsRow>
        <SettingsRow label="Основний шаблон" htmlFor="main-template">
          <select
            id="main-template"
            className="h-7 px-2 text-[13px] max-w-[240px]"
            value={templates.some((t) => t.id === main) ? main : MAIN_TEMPLATE_DEFAULT}
            onChange={(e) => {
              setMain(e.target.value);
              setMainTemplate(e.target.value);
            }}
          >
            {templates.map((t) => (
              <option key={t.id} value={t.id}>
                {t.name}
              </option>
            ))}
          </select>
        </SettingsRow>
        <SettingsRow label="Пропонувати шаблон за часом дня">
          <Switch
            checked={byHour}
            onCheckedChange={(on) => {
              setByHour(on);
              setRemembersByHour(on);
            }}
          />
        </SettingsRow>
      </SettingsGroup>

      <section className="flex flex-col gap-1.5">
        <h3 className="settings-group-title">Шаблони</h3>
        <p className="settings-help !mt-0 !mb-1">
          Шаблон визначає, що буде в документі. Вибрати його можна у вкладці «Документ» кожного запису.
        </p>
        <div className="settings-list">
          {templates.map((t) => (
            <button
              key={t.id}
              type="button"
              onClick={() => void open(t.id)}
              className="settings-row w-full text-left hover:bg-[var(--hover)]"
            >
              <span className="min-w-0">
                <span className="block truncate">{t.name}</span>
                <span className="block text-xs text-[var(--muted-text)] truncate">{t.description}</span>
              </span>
              <span className="flex items-center gap-2 flex-none text-xs text-[var(--muted-text)]">
                {t.builtIn ? 'вбудований' : 'ваш'}
                <ChevronRight className="w-4 h-4" />
              </span>
            </button>
          ))}
        </div>
        <div className="flex gap-2 pt-1">
          <button type="button" className="settings-button" onClick={() => void createNew()}>
            <Plus className="w-3.5 h-3.5" /> Новий шаблон
          </button>
          <button type="button" className="settings-button" title="Файл .json із шаблонами (а також словником вашої організації)" onClick={() => void importFile()}>
            <FileDown className="w-3.5 h-3.5" /> Імпортувати…
          </button>
        </div>
      </section>


      {form && (
        <TemplateEditor
          form={form}
          templates={templates}
          onChange={setForm}
          onClose={() => setForm(null)}
          onSaved={async (id) => {
            await load();
            window.dispatchEvent(new Event(TEMPLATES_CHANGED_EVENT));
            setForm(await invoke<TemplateForm>('api_get_meeting_type', { id }).catch(() => null));
          }}
          onDeleted={async () => {
            setForm(null);
            await load();
            window.dispatchEvent(new Event(TEMPLATES_CHANGED_EVENT));
          }}
        />
      )}
    </div>
  );
}

function TemplateEditor({
  form,
  templates,
  onChange,
  onClose,
  onSaved,
  onDeleted,
}: {
  form: TemplateForm;
  templates: DocumentTemplate[];
  onChange: (f: TemplateForm) => void;
  onClose: () => void;
  onSaved: (id: string) => Promise<void>;
  onDeleted: () => Promise<void>;
}) {
  const [saving, setSaving] = useState(false);
  const [more, setMore] = useState(false);
  const [newField, setNewField] = useState('');
  const [knownRoles, setKnownRoles] = useState<string[]>([]);
  useEffect(() => {
    invoke<string[]>('get_speaker_roles').then(setKnownRoles).catch(() => setKnownRoles([]));
  }, []);

  const set = (patch: Partial<TemplateForm>) => onChange({ ...form, ...patch });
  const setPart = (i: number, patch: Partial<MeetingPart>) => set({ parts: form.parts.map((p, k) => (k === i ? { ...p, ...patch } : p)) });
  const byFields = form.style === 'fields';
  const on = (b: string) => form.blocks.includes(b);
  // The main list: blocks that are on, in the document's order, then the rest
  const ordered = form.blocks.filter((b) => MAIN_VALUES.includes(b));
  const rest = MAIN_VALUES.filter((b) => !ordered.includes(b));

  const toggle = (b: string, value: boolean) => {
    if (!value) return set({ blocks: form.blocks.filter((x) => x !== b) });
    if (b === 'Контроль') return set({ blocks: ['Контроль', ...form.blocks] });
    if (b === 'Не доповідали') {
      const at = form.blocks.indexOf('Хід наради');
      const blocks = [...form.blocks];
      blocks.splice(at >= 0 ? at + 1 : blocks.length, 0, b);
      return set({ blocks });
    }
    // The transcript stays last; anything else goes before it
    const blocks = form.blocks.filter((x) => x !== 'Стенограма');
    blocks.push(b);
    if (b !== 'Стенограма' && on('Стенограма')) blocks.push('Стенограма');
    set({ blocks });
  };

  /** Moves a block that is on to another place among those that are on */
  const move = (from: string, to: string) => {
    if (from === to) return;
    const blocks = [...form.blocks];
    blocks.splice(blocks.indexOf(from), 1);
    blocks.splice(blocks.indexOf(to) + (form.blocks.indexOf(from) < form.blocks.indexOf(to) ? 1 : 0), 0, from);
    set({ blocks });
  };

  // Drag by the handle: the row under the pointer takes the dragged block's place
  const dragging = useRef<string | null>(null);
  const [dragged, setDragged] = useState<string | null>(null);
  const rowsRef = useRef<Map<string, HTMLDivElement>>(new Map());
  const onPointerMove = (e: React.PointerEvent) => {
    const from = dragging.current;
    if (!from) return;
    for (const [b, el] of rowsRef.current) {
      if (b === from || !ordered.includes(b)) continue;
      const r = el.getBoundingClientRect();
      if (e.clientY > r.top + r.height * 0.25 && e.clientY < r.bottom - r.height * 0.25) {
        move(from, b);
        break;
      }
    }
  };

  const save = async () => {
    setSaving(true);
    try {
      const copy = form.builtIn;
      const name = form.name.trim();
      const toSave = {
        ...form,
        // A copy of a built-in template gets its own name
        name: copy && templates.some((t) => t.name === name) ? `${name} (копія)` : name,
        id: copy ? null : form.id,
        description: '',
        reportFields: form.reportFields.map((f) => f.trim()).filter(Boolean),
      };
      const id = await invoke<string>('api_save_meeting_type', { form: toSave });
      toast.success(copy ? `Збережено як ваш шаблон «${toSave.name}»` : 'Шаблон збережено');
      await onSaved(id);
    } catch (e) {
      toast.error(`${e}`);
    } finally {
      setSaving(false);
    }
  };

  const remove = async () => {
    if (!form.id || form.builtIn) return;
    if (!(await confirmNative(`Видалити шаблон «${form.name}»?`, 'Документи, уже створені за ним, не зміняться.', 'Видалити'))) return;
    try {
      await invoke('api_delete_meeting_type', { id: form.id });
      toast.success('Шаблон видалено');
      await onDeleted();
    } catch (e) {
      toast.error(`${e}`);
    }
  };

  const exportFile = async () => {
    if (!form.id) return;
    try {
      const path = await invoke<string | null>('api_export_meeting_type', { id: form.id });
      if (path) toast.success('Шаблон збережено у файл', { description: path });
    } catch (e) {
      toast.error(`${e}`);
    }
  };

  const row = (b: { value: string; label: string; help?: string }, draggable: boolean) => (
    <div
      key={b.value}
      ref={(el) => {
        if (el) rowsRef.current.set(b.value, el);
        else rowsRef.current.delete(b.value);
      }}
      className="tpl-ed-row"
      data-dragged={dragged === b.value}
      data-off={!on(b.value)}
    >
      <div className="tpl-ed-row-main">
        {draggable ? (
          <span
            className="tpl-ed-handle"
            aria-label="Перетягнути"
            title="Перетягніть, щоб змінити порядок"
            onPointerDown={(e) => {
              (e.target as HTMLElement).setPointerCapture(e.pointerId);
              dragging.current = b.value;
              setDragged(b.value);
            }}
            onPointerMove={onPointerMove}
            onPointerUp={() => {
              dragging.current = null;
              setDragged(null);
            }}
            onPointerCancel={() => {
              dragging.current = null;
              setDragged(null);
            }}
          >
            <GripVertical className="h-4 w-4" />
          </span>
        ) : (
          <span className="tpl-ed-handle" aria-hidden="true" />
        )}
        <span className="flex-1 min-w-0">
          {b.label}
          {b.help && <span className="text-[var(--muted-text)]"> · {b.help}</span>}
        </span>
        <Switch checked={on(b.value)} onCheckedChange={(v) => toggle(b.value, v)} aria-label={b.label} />
      </div>
      {b.value === 'Хід наради' && on(b.value) && (
        <div className="tpl-ed-sub">
          <div className="segmented inline-flex text-[12.5px]">
            {[
              ['points', 'Короткими пунктами'],
              ['fields', 'За полями'],
            ].map(([value, label]) => (
              <button key={value} type="button" className="segmented-item px-3 h-[26px]" data-active={form.style === value} onClick={() => set({ style: value })}>
                {label}
              </button>
            ))}
          </div>
          {byFields && (
            <div className="tpl-ed-chips">
              {form.reportFields.map((f, i) => (
                <span key={`${f}-${i}`} className="tpl-ed-chip">
                  {f}
                  <button type="button" aria-label={`Прибрати поле ${f}`} onClick={() => set({ reportFields: form.reportFields.filter((_, k) => k !== i) })}>
                    <X className="h-3 w-3" />
                  </button>
                </span>
              ))}
              <input
                className="tpl-ed-chip-input"
                placeholder="+ Поле"
                aria-label="Нове поле"
                value={newField}
                onChange={(e) => setNewField(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === 'Enter' && newField.trim()) {
                    set({ reportFields: [...form.reportFields, newField.trim()] });
                    setNewField('');
                  }
                }}
                onBlur={() => {
                  if (newField.trim()) {
                    set({ reportFields: [...form.reportFields, newField.trim()] });
                    setNewField('');
                  }
                }}
              />
            </div>
          )}
        </div>
      )}
    </div>
  );

  return (
    <Dialog open onOpenChange={(o) => !o && onClose()}>
      <DialogContent className="tpl-editor max-w-[920px] w-[calc(100vw-48px)] p-0 gap-0" aria-describedby={undefined}>
        <DialogTitle className="sr-only">{form.name || 'Шаблон'}</DialogTitle>
        <div className="tpl-ed-body">
          <div className="tpl-ed-form custom-scrollbar">
            <label className="tpl-ed-label" htmlFor="tpl-name">
              Назва
            </label>
            <div className="flex items-center gap-2">
              <input id="tpl-name" type="text" className="h-8 px-2.5 flex-1 min-w-0" value={form.name} onChange={(e) => set({ name: e.target.value })} />
              <span className="tpl-tag">{form.builtIn ? 'вбудований' : 'ваш шаблон'}</span>
            </div>
            {form.builtIn && <p className="settings-help !m-0">Вбудований шаблон не змінюється: зміни збережуться як ваш шаблон.</p>}

            <div className="tpl-ed-label">Що буде в документі · порядок змінюється перетягуванням</div>
            <div className="settings-list" onPointerMove={onPointerMove}>
              {ordered.map((v) => row(MAIN_BLOCKS.find((b) => b.value === v)!, ordered.length > 1))}
              {rest.map((v) => row(MAIN_BLOCKS.find((b) => b.value === v)!, false))}
            </div>

            <div className="tpl-ed-label">Обсяг</div>
            <div className="segmented inline-flex self-start text-[12.5px]">
              {[
                [false, 'Коротко'],
                [true, 'Докладніше'],
              ].map(([value, label]) => (
                <button key={String(value)} type="button" className="segmented-item px-3 h-[26px]" data-active={form.detailed === value} onClick={() => set({ detailed: value as boolean })}>
                  {label as string}
                </button>
              ))}
            </div>

            <label className="tpl-ed-label" htmlFor="tpl-wishes">
              Побажання до тексту
            </label>
            <textarea
              id="tpl-wishes"
              rows={2}
              className="px-2.5 py-1.5 rounded-[10px] border border-[var(--card-border)] bg-[var(--card-bg)] text-sm"
              placeholder="Наприклад: посади писати повністю, суми вказувати в гривнях."
              value={form.wishes}
              onChange={(e) => set({ wishes: e.target.value })}
            />

            <button type="button" className="tpl-ed-more" onClick={() => setMore(!more)} aria-expanded={more}>
              <ChevronDown className="h-3.5 w-3.5" style={{ transform: more ? 'none' : 'rotate(-90deg)' }} />
              Додатково: частини наради, контроль попередніх завдань
            </button>
            {more && (
              <div className="flex flex-col gap-3">
                <div className="settings-list">
                  <div className="settings-row">
                    <span>
                      Контроль попередніх завдань <span className="text-xs text-[var(--muted-text)]">— на початку документа</span>
                    </span>
                    <Switch checked={on('Контроль')} onCheckedChange={(v) => toggle('Контроль', v)} />
                  </div>
                  {on('Контроль') && (
                    <label className="settings-row">
                      <span>Перевіряти завдання наради за шаблоном</span>
                      <select className="h-7 px-2 text-[13px] max-w-[220px]" value={form.controls ?? ''} onChange={(e) => set({ controls: e.target.value || null })}>
                        <option value="">— будь-якої попередньої —</option>
                        {templates.map((t) => (
                          <option key={t.id} value={t.id}>
                            {t.name}
                          </option>
                        ))}
                      </select>
                    </label>
                  )}
                  <div className="settings-row">
                    <span>
                      Хто не доповідав <span className="text-xs text-[var(--muted-text)]">— постійні учасники, яких не було чути</span>
                    </span>
                    <Switch checked={on('Не доповідали')} onCheckedChange={(v) => toggle('Не доповідали', v)} />
                  </div>
                  {byFields && on('Хід наради') && (
                    <div className="settings-row">
                      <span>
                        Групувати за спікером <span className="text-xs text-[var(--muted-text)]">— усе, що людина сказала, одним блоком</span>
                      </span>
                      <Switch checked={form.groupBySpeaker} onCheckedChange={(v) => set({ groupBySpeaker: v })} />
                    </div>
                  )}
                </div>

                <div className="tpl-ed-label !mt-0">Частини наради</div>
                <p className="settings-help !m-0">
                  Частина починається, коли хтось каже одну з фраз-підказок, або з першого виступу людини з цією роллю (роль ставиться у «Спікери»).
                </p>
                <div className="settings-list">
                  {form.parts.length === 0 && <div className="settings-row text-[var(--muted-text)]">Без частин, одним списком</div>}
                  {form.parts.map((p, i) => (
                    <div key={i} className="settings-block grid grid-cols-[1fr_auto] gap-2 text-sm">
                      <input type="text" className="h-8 px-2.5" aria-label="Назва частини" placeholder="Назва частини" value={p.title} onChange={(e) => setPart(i, { title: e.target.value })} />
                      <button type="button" aria-label="Прибрати частину" className="settings-button" onClick={() => set({ parts: form.parts.filter((_, k) => k !== i) })}>
                        <X className="w-3.5 h-3.5" />
                      </button>
                      <input
                        type="text"
                        className="h-8 px-2.5"
                        aria-label="Фрази-підказки"
                        placeholder="Фрази, з яких починається, через кому"
                        defaultValue={(p.cues ?? []).join(', ')}
                        onBlur={(e) => setPart(i, { cues: commaList(e.target.value) })}
                      />
                      <span />
                      <input
                        type="text"
                        className="h-8 px-2.5"
                        aria-label="Ролі"
                        list="known-speaker-roles"
                        placeholder={knownRoles.length ? `Ролі через кому, напр. «${knownRoles.slice(0, 3).join(', ')}»` : 'Ролі через кому (необов’язково)'}
                        value={p.role ?? ''}
                        onChange={(e) => setPart(i, { role: e.target.value })}
                      />
                    </div>
                  ))}
                </div>
                <datalist id="known-speaker-roles">
                  {knownRoles.map((r) => (
                    <option key={r} value={r} />
                  ))}
                </datalist>
                <div className="flex">
                  <button type="button" className="settings-button" onClick={() => set({ parts: [...form.parts, { title: '', cues: [], role: null }] })}>
                    <Plus className="w-3.5 h-3.5" /> Додати частину
                  </button>
                </div>

                <div className="tpl-ed-label !mt-0">Пропонувати цей шаблон</div>
                <div className="settings-list">
                  <label className="settings-block flex flex-col gap-1 text-sm">
                    Слова в назві запису, через кому
                    <input
                      type="text"
                      className="h-8 px-2.5"
                      placeholder="наприклад: планерка, щоденна"
                      defaultValue={form.titleHints.join(', ')}
                      key={`hints-${form.id}`}
                      onBlur={(e) => set({ titleHints: commaList(e.target.value) })}
                    />
                  </label>
                  <label className="settings-block flex flex-col gap-1 text-sm">
                    Години початку, через кому
                    <input
                      type="text"
                      className="h-8 px-2.5"
                      placeholder="наприклад: 8, 9"
                      defaultValue={form.hours.join(', ')}
                      key={`hours-${form.id}`}
                      onBlur={(e) => set({ hours: commaList(e.target.value).map(Number).filter((h) => Number.isInteger(h) && h >= 0 && h < 24) })}
                    />
                  </label>
                </div>
              </div>
            )}
          </div>
          <div className="tpl-ed-preview custom-scrollbar" aria-label="Зразок">
            <div className="tpl-ed-label !mt-0">Зразок</div>
            <TemplateSample form={form} />
          </div>
        </div>
        <div className="tpl-ed-footer">
          {!form.builtIn && form.id && (
            <button type="button" className="settings-button !text-[#c4291c]" onClick={() => void remove()}>
              Видалити
            </button>
          )}
          {form.id && (
            <button type="button" className="settings-button" onClick={() => void exportFile()}>
              Зберегти у файл
            </button>
          )}
          <span className="flex-1" />
          <button type="button" className="settings-button" onClick={onClose}>
            Скасувати
          </button>
          <button type="button" className="settings-button !bg-[var(--blue)] !text-white" disabled={saving || !form.name.trim()} onClick={() => void save()}>
            {form.builtIn ? 'Зберегти як новий' : 'Зберегти'}
          </button>
        </div>
      </DialogContent>
    </Dialog>
  );
}

/** What a document made with the form looks like, with made-up content */
function TemplateSample({ form }: { form: TemplateForm }) {
  const byFields = form.style === 'fields';
  const fields = form.reportFields.length ? form.reportFields : ['Зроблено', 'План', 'Проблеми'];
  const speakers: { name: string; time: string; points: string[]; more: string[]; values: string[] }[] = [
    {
      name: 'Олена К.',
      time: '00:05',
      points: ['Склад перевірено, 20 ящиків прийнято', 'Наступна перевірка в четвер'],
      more: ['Документи передано до штабу'],
      values: ['склад перевірено', 'перевірка в четвер', 'бракує пального'],
    },
    {
      name: 'Андрій М.',
      time: '03:40',
      points: ['Навчання з водіями проведено', 'Потрібні 2 нові радіостанції'],
      more: ['Графік на тиждень погоджено'],
      values: ['навчання проведено', 'графік на тиждень', 'немає радіостанцій'],
    },
  ];
  const head = (text: string) => <div className="tpl-sample-h">{text}</div>;
  return (
    <div className="tpl-sample">
      <div className="tpl-sample-title">{form.name.trim() || 'Документ'}</div>
      <div className="tpl-sample-meta">Тривалість: 29 хв · Учасники: Олена К., Андрій М.</div>
      {form.blocks.map((b) => {
        switch (b) {
          case 'Контроль':
            return (
              <div key={b}>
                {head('Контроль виконання')}
                <div className="tpl-sample-line">Перевірити зв’язок · виконано</div>
              </div>
            );
          case 'Хід наради':
            return (
              <div key={b}>
                {head('Хід наради')}
                {speakers.map((s) => (
                  <div key={s.name} className="tpl-sample-speaker">
                    <div className="tpl-sample-who">
                      {s.name} <span>{s.time}</span>
                    </div>
                    {byFields
                      ? fields.slice(0, 4).map((f, i) => (
                          <div key={f} className="tpl-sample-line">
                            <b>{f}:</b> {s.values[i % s.values.length]}
                          </div>
                        ))
                      : [...s.points, ...(form.detailed ? s.more : [])].map((p) => (
                          <div key={p} className="tpl-sample-line">
                            • {p}
                          </div>
                        ))}
                  </div>
                ))}
              </div>
            );
          case 'Не доповідали':
            return (
              <div key={b}>
                {head('Не доповідали')}
                <div className="tpl-sample-line">Ігор Д.</div>
              </div>
            );
          case 'Рішення':
            return (
              <div key={b}>
                {head('Рішення')}
                <div className="tpl-sample-line">• Перенести навчання на четвер</div>
              </div>
            );
          case 'Завдання':
            return (
              <div key={b}>
                {head('Завдання')}
                <div className="tpl-sample-line">Олена К. · перевірити склад · до п’ятниці</div>
                <div className="tpl-sample-line">Андрій М. · подати заявку на радіостанції · до …</div>
              </div>
            );
          case 'Відкриті питання':
            return (
              <div key={b}>
                {head('Відкриті питання')}
                <div className="tpl-sample-line">• Хто відповідає за пальне?</div>
              </div>
            );
          case OVERALL_BLOCK:
            return (
              <div key={b}>
                {head('Загальний підсумок')}
                <div className="tpl-sample-line">• Склад готовий, перевірка в четвер</div>
                <div className="tpl-sample-line">• Потрібні 2 радіостанції</div>
                {form.detailed && <div className="tpl-sample-line">• Графік на тиждень погоджено</div>}
              </div>
            );
          case 'Стенограма':
            return (
              <div key={b}>
                {head('Стенограма по доповідачах')}
                <div className="tpl-sample-line">
                  <b>Олена К. · 00:05</b> Доброго ранку. Склад перевірили, двадцять ящиків прийняли…
                </div>
              </div>
            );
          default:
            return null;
        }
      })}
    </div>
  );
}
