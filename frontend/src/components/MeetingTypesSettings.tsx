'use client';
import { confirmNative } from '@/lib/confirm';

import { useCallback, useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { toast } from '@/lib/notice';
import { ChevronRight, FileDown, Plus, Trash2, X } from 'lucide-react';
import { plural } from '@/lib/plural';

interface MeetingType {
  id: string;
  name: string;
  description: string;
}

interface MeetingPart {
  title: string;
  cues?: string[];
  role?: string | null;
}

interface MeetingTypeForm {
  id: string | null;
  name: string;
  description: string;
  reportFields: string[];
  parts: MeetingPart[];
  blocks: string[];
  titleHints: string[];
  hours: number[];
  controls: string | null;
  /** «Групувати за спікером»: one block per person in «Хід наради» */
  groupBySpeaker: boolean;
  builtIn: boolean;
}

/** Document blocks with a short explanation, in document order */
const BLOCKS: { value: string; help: string }[] = [
  { value: 'Контроль', help: 'стан завдань попередньої наради' },
  { value: 'Хід наради', help: 'хто що сказав, коротко' },
  { value: 'Рішення', help: 'що вирішили' },
  { value: 'Завдання', help: 'хто, що, до коли' },
  { value: 'Відкриті питання', help: 'що лишилося невирішеним' },
  { value: 'Не доповідали', help: 'постійні учасники, яких не було чути' },
  { value: 'Стенограма', help: 'дослівний текст у кінці документа' },
];

const DEFAULT_TYPE = 'meeting_materials_uk';

const lines = (s: string) => s.split('\n');
const commaList = (s: string) => s.split(',').map((x) => x.trim()).filter(Boolean);

/**
 * Meeting types in Settings: any team describes its own meetings (what to
 * note about each speaker, the parts, which sections the document has).
 * Built-in types stay as they are; a change is saved as the user's copy.
 */
export function MeetingTypesSettings() {
  const [types, setTypes] = useState<MeetingType[]>([]);
  const [form, setForm] = useState<MeetingTypeForm | null>(null);
  const [saving, setSaving] = useState(false);
  const [knownRoles, setKnownRoles] = useState<string[]>([]);

  const load = useCallback(async () => {
    setTypes(await invoke<MeetingType[]>('api_list_templates').catch(() => []));
  }, []);

  useEffect(() => {
    load();
    invoke<string[]>('get_speaker_roles').then(setKnownRoles).catch(() => setKnownRoles([]));
  }, [load]);

  const open = async (id: string) => {
    try {
      setForm(await invoke<MeetingTypeForm>('api_get_meeting_type', { id }));
    } catch (e) {
      toast.error(`Не вдалося відкрити тип: ${e}`);
    }
  };

  const createNew = async () => {
    try {
      const base = await invoke<MeetingTypeForm>('api_get_meeting_type', { id: DEFAULT_TYPE });
      setForm({ ...base, id: null, builtIn: false, name: 'Новий тип', description: '', titleHints: [], hours: [] });
    } catch (e) {
      toast.error(`${e}`);
    }
  };

  const save = async () => {
    if (!form) return;
    setSaving(true);
    try {
      const copy = form.builtIn;
      const toSave = {
        ...form,
        // A copy of a built-in type gets its own name
        name: copy && types.some((t) => t.name === form.name.trim()) ? `${form.name.trim()} (копія)` : form.name,
        id: copy ? null : form.id,
        reportFields: form.reportFields.map((f) => f.trim()).filter(Boolean),
      };
      const id = await invoke<string>('api_save_meeting_type', { form: toSave });
      await load();
      setForm(await invoke<MeetingTypeForm>('api_get_meeting_type', { id }));
      toast.success(copy ? `Збережено як ваш тип «${toSave.name}»` : 'Збережено');
    } catch (e) {
      toast.error(`${e}`);
    } finally {
      setSaving(false);
    }
  };

  const remove = async () => {
    if (!form?.id || form.builtIn) return;
    if (!(await confirmNative(`Видалити тип «${form.name}»?`, 'Записи, вже оформлені цим типом, не зміняться.', 'Видалити'))) return;
    try {
      await invoke('api_delete_meeting_type', { id: form.id });
      setForm(null);
      await load();
      toast.success('Тип видалено');
    } catch (e) {
      toast.error(`${e}`);
    }
  };

  const importPack = async () => {
    try {
      const done = await invoke<[number, number] | null>('api_import_pack', { path: null });
      if (!done) return;
      const [types, terms] = done;
      await load();
      toast.success(
        `Імпортовано: ${types} ${plural(types, 'тип', 'типи', 'типів')}, ${terms} ${plural(terms, 'новий термін', 'нові терміни', 'нових термінів')}`,
      );
    } catch (e) {
      toast.error(`${e}`);
    }
  };
  const set = (patch: Partial<MeetingTypeForm>) => setForm((f) => (f ? { ...f, ...patch } : f));
  const setPart = (i: number, patch: Partial<MeetingPart>) =>
    setForm((f) => (f ? { ...f, parts: f.parts.map((p, k) => (k === i ? { ...p, ...patch } : p)) } : f));

  return (
    <div className="flex flex-col gap-2">
      <p className="settings-help">
        Тип визначає, як із запису складається документ: що коротко записати про кожного, хто говорить, з яких частин
        складається розмова та які розділи будуть у документі. Тип можна вибрати над документом.
      </p>

      <div className="settings-list">
        {types.map((t) => (
          <button
            key={t.id}
            type="button"
            onClick={() => open(t.id)}
            data-active={form?.id === t.id}
            className="settings-row w-full text-left hover:bg-[var(--selected)] data-[active=true]:bg-[var(--selected)]"
          >
            <span className="min-w-0">
              <span className="block truncate">{t.name}</span>
              <span className="block text-xs text-[var(--muted-text)] truncate">{t.description}</span>
            </span>
            <span className="flex items-center gap-2 flex-none text-xs text-[var(--muted-text)]">
              {t.id.startsWith('custom:') ? 'ваш' : 'вбудований'}
              <ChevronRight className="w-4 h-4" />
            </span>
          </button>
        ))}
      </div>
      <div className="flex gap-2">
        <button type="button" className="settings-button" onClick={createNew}>
          <Plus className="w-3.5 h-3.5" /> Новий тип
        </button>
        <button
          type="button"
          className="settings-button"
          title="Файл .json із типами записів, словником та формулюваннями вашої організації"
          onClick={importPack}
        >
          <FileDown className="w-3.5 h-3.5" /> Імпорт шаблону
        </button>
      </div>

      {form && (
        <>
          <h3 className="settings-group-title">{form.builtIn ? `${form.name} — вбудований тип` : form.id ? 'Ваш тип' : 'Новий тип'}</h3>
          {form.builtIn && (
            <p className="settings-help">Вбудований тип не змінюється: ваші зміни збережуться як новий тип, а цей лишиться як був.</p>
          )}
          <div className="settings-list">
            <label className="settings-block flex flex-col gap-1 text-sm">
              Назва
              <input type="text" className="h-8 px-2.5" value={form.name} onChange={(e) => set({ name: e.target.value })} />
            </label>
            <label className="settings-block flex flex-col gap-1 text-sm">
              Опис (для кого і коли)
              <input type="text" className="h-8 px-2.5" value={form.description} onChange={(e) => set({ description: e.target.value })} />
            </label>
            <label className="settings-block flex flex-col gap-1 text-sm">
              Що записати про кожного, хто виступає — по рядку
              <textarea
                rows={4}
                className="px-2.5 py-1.5 rounded-[10px] border border-[var(--card-border)] bg-[var(--selected)] text-sm"
                placeholder={'Наприклад:\nЗроблено\nПлан\nПроблеми'}
                value={form.reportFields.join('\n')}
                onChange={(e) => set({ reportFields: lines(e.target.value) })}
              />
              <span className="text-xs text-[var(--muted-text)]">
                Порожньо — 1–2 речення своїми словами. Пункт, що починається з «План», потрапляє в «Завдання» з іменем доповідача.
              </span>
            </label>
          </div>

          <h3 className="settings-group-title">Частини розмови</h3>
          <p className="settings-help">
            Необов’язково. Частина починається, коли хтось каже одну з фраз-підказок, або з першого виступу людини з цією роллю (роль
            ставиться у «Спікери»).
          </p>
          <div className="settings-list">
            {form.parts.length === 0 && <div className="settings-row text-[var(--muted-text)]">Без частин — одним списком</div>}
            {form.parts.map((p, i) => (
              <div key={i} className="settings-block grid grid-cols-[1fr_auto] gap-2 text-sm">
                <input
                  type="text"
                  className="h-8 px-2.5"
                  aria-label="Назва частини"
                  placeholder="Назва частини"
                  value={p.title}
                  onChange={(e) => setPart(i, { title: e.target.value })}
                />
                <button
                  type="button"
                  aria-label="Прибрати частину"
                  className="settings-button"
                  onClick={() => set({ parts: form.parts.filter((_, k) => k !== i) })}
                >
                  <X className="w-3.5 h-3.5" />
                </button>
                <input
                  type="text"
                  className="h-8 px-2.5"
                  aria-label="Фрази-підказки"
                  placeholder="Фрази, з яких починається, через кому"
                  value={(p.cues ?? []).join(', ')}
                  onChange={(e) => setPart(i, { cues: commaList(e.target.value) })}
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
          {knownRoles.length > 0 && (
            <p className="settings-help">Ролі зі «Спікерів»: {knownRoles.join(', ')}. Частина почнеться з першого виступу людини з будь-якою з указаних ролей.</p>
          )}
          <div className="flex">
            <button type="button" className="settings-button" onClick={() => set({ parts: [...form.parts, { title: '', cues: [], role: null }] })}>
              <Plus className="w-3.5 h-3.5" /> Додати частину
            </button>
          </div>

          <h3 className="settings-group-title">Розділи документа</h3>
          <div className="settings-list">
            {BLOCKS.map((b) => (
              <label key={b.value} className="settings-row cursor-pointer">
                <span>
                  {b.value} <span className="text-xs text-[var(--muted-text)]">— {b.help}</span>
                </span>
                <input
                  type="checkbox"
                  checked={form.blocks.includes(b.value)}
                  onChange={(e) =>
                    set({ blocks: e.target.checked ? [...form.blocks, b.value] : form.blocks.filter((x) => x !== b.value) })
                  }
                />
              </label>
            ))}
            {form.blocks.includes('Хід наради') && (
              <label className="settings-row cursor-pointer">
                <span>
                  Групувати за спікером{' '}
                  <span className="text-xs text-[var(--muted-text)]">— усе, що людина сказала за розмову, одним блоком, без повторів</span>
                </span>
                <input type="checkbox" checked={form.groupBySpeaker} onChange={(e) => set({ groupBySpeaker: e.target.checked })} />
              </label>
            )}
            {form.blocks.includes('Контроль') && (
              <label className="settings-row">
                <span>Перевіряти завдання наради типу</span>
                <select className="h-8 px-2" value={form.controls ?? ''} onChange={(e) => set({ controls: e.target.value || null })}>
                  <option value="">— будь-якої попередньої —</option>
                  {types.map((t) => (
                    <option key={t.id} value={t.id}>
                      {t.name}
                    </option>
                  ))}
                </select>
              </label>
            )}
          </div>

          <h3 className="settings-group-title">Автовибір типу</h3>
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
                onBlur={(e) =>
                  set({ hours: commaList(e.target.value).map(Number).filter((h) => Number.isInteger(h) && h >= 0 && h < 24) })
                }
              />
            </label>
          </div>

          <div className="flex items-center gap-2 pt-1">
            <button type="button" className="settings-button !bg-[var(--blue)] !text-white" disabled={saving} onClick={save}>
              {form.builtIn ? 'Зберегти як новий тип' : 'Зберегти'}
            </button>
            <button type="button" className="settings-button" onClick={() => setForm(null)}>
              Скасувати
            </button>
            {!form.builtIn && form.id && (
              <button type="button" className="settings-button ml-auto" onClick={remove}>
                <Trash2 className="w-3.5 h-3.5" /> Видалити
              </button>
            )}
          </div>
        </>
      )}
    </div>
  );
}
