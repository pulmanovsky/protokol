'use client';

import { useEffect, useMemo, useState } from 'react';
import { useRouter } from 'next/navigation';
import { MoreHorizontal, Pencil, SearchIcon, Trash2, Upload, X } from 'lucide-react';
import { useSidebar, type CurrentMeeting } from '@/components/Sidebar/SidebarProvider';
import { useConfig } from '@/contexts/ConfigContext';
import { useImportDialog } from '@/contexts/ImportDialogContext';
import { formatTime } from '@/components/CalendarReminder';
import { RENAME_MEETING_EVENT, DELETE_MEETING_EVENT } from '@/lib/meetingEvents';
import { SourcePicker } from '@/components/audio/SourcePicker';
import { popupNativeMenu } from '@/lib/nativeMenu';

type Filter = 'all' | 'draft' | 'approved';

const FILTERS: { value: Filter; label: string }[] = [
  { value: 'all', label: 'Усі' },
  { value: 'draft', label: 'Чернетки' },
  { value: 'approved', label: 'Затверджені' },
];

const GROUPS = ['Сьогодні', 'Цього тижня', 'Раніше'] as const;

function startOfDay(d: Date): Date {
  const c = new Date(d);
  c.setHours(0, 0, 0, 0);
  return c;
}

function groupOf(iso: string | undefined, now: Date): (typeof GROUPS)[number] {
  if (!iso) return 'Раніше';
  const date = new Date(iso);
  const today = startOfDay(now);
  if (date >= today) return 'Сьогодні';
  // Week starts on Monday
  const weekStart = new Date(today);
  weekStart.setDate(today.getDate() - ((today.getDay() + 6) % 7));
  return date >= weekStart ? 'Цього тижня' : 'Раніше';
}

function formatWhen(iso: string | undefined, now: Date): string {
  if (!iso) return '';
  const date = new Date(iso);
  const sameYear = date.getFullYear() === now.getFullYear();
  const day = date.toLocaleDateString('uk-UA', sameYear ? { day: 'numeric', month: 'long' } : { day: 'numeric', month: 'long', year: 'numeric' });
  return `${day}, ${formatTime(iso)}`;
}

function formatDuration(seconds?: number | null): string {
  if (!seconds || seconds <= 0) return '';
  const minutes = Math.max(1, Math.round(seconds / 60));
  return minutes >= 60 ? `${Math.floor(minutes / 60)} год ${minutes % 60} хв` : `${minutes} хв`;
}

interface MenuState {
  meeting: CurrentMeeting;
  x: number;
  y: number;
}

export default function ProtocolsPage() {
  const router = useRouter();
  const { meetings, setCurrentMeeting, handleRecordingToggle } = useSidebar();
  const { betaFeatures } = useConfig();
  const { openImportDialog } = useImportDialog();
  const [query, setQuery] = useState('');
  const [filter, setFilter] = useState<Filter>('all');
  const [menu, setMenu] = useState<MenuState | null>(null);
  const now = useMemo(() => new Date(), []);

  useEffect(() => {
    if (!menu) return;
    const close = () => setMenu(null);
    const onKey = (e: KeyboardEvent) => e.key === 'Escape' && close();
    window.addEventListener('click', close);
    window.addEventListener('keydown', onKey);
    window.addEventListener('resize', close);
    return () => {
      window.removeEventListener('click', close);
      window.removeEventListener('keydown', onKey);
      window.removeEventListener('resize', close);
    };
  }, [menu]);

  const visible = useMemo(() => {
    const q = query.trim().toLowerCase();
    return meetings
      .filter((m) => (filter === 'all' ? true : filter === 'approved' ? m.status === 'approved' : m.status !== 'approved'))
      .filter((m) => !q || m.title.toLowerCase().includes(q))
      .sort((a, b) => (b.createdAt ?? '').localeCompare(a.createdAt ?? ''));
  }, [meetings, query, filter]);

  const grouped = useMemo(
    () => GROUPS.map((group) => ({ group, items: visible.filter((m) => groupOf(m.createdAt, now) === group) })).filter((g) => g.items.length > 0),
    [visible, now],
  );

  const open = (m: CurrentMeeting) => {
    setCurrentMeeting({ id: m.id, title: m.title });
    router.push(`/meeting-details?id=${m.id}`);
  };

  const openMenu = (m: CurrentMeeting, x: number, y: number) => setMenu({ meeting: m, x, y });

  return (
    <div className="h-full overflow-y-auto custom-scrollbar">
      <div className="flex flex-col gap-6 p-8">
        <header data-tauri-drag-region data-notice-anchor className="flex items-center justify-between gap-4 flex-wrap">
          <div className="flex flex-col gap-0.5">
            <h1 data-tauri-drag-region className="text-[30px] font-bold tracking-[-0.6px] text-gray-900">Записи</h1>
            <span data-tauri-drag-region className="text-sm text-[color:var(--muted-text)]">{countLabel(meetings.length)}</span>
          </div>
          <div className="flex items-center gap-2.5 flex-wrap">
            {betaFeatures.importAndRetranscribe && (
              <button type="button" onClick={() => openImportDialog()} className="secondary-button h-11 px-4 gap-2 text-sm font-semibold">
                <Upload className="w-4 h-4" />
                Імпортувати аудіо
              </button>
            )}
            <SourcePicker />
            <button type="button" onClick={handleRecordingToggle} className="record-button h-11 px-5 gap-2.5 text-[15px] font-semibold">
              <span className="w-2.5 h-2.5 rounded-full bg-[#fff]" aria-hidden="true" />
              Почати запис
            </button>
          </div>
        </header>

        {meetings.length === 0 ? (
          <section className="card px-6 py-12 flex flex-col items-center gap-3 text-center">
            <h2 className="text-lg font-bold text-gray-900">Ще немає записів</h2>
            <p className="text-sm text-[color:var(--muted-text)] max-w-sm">Почніть запис або імпортуйте аудіофайл (можна просто перетягнути його у вікно), і тут з’явиться стенограма з розрізненими спікерами.</p>
            <button type="button" onClick={handleRecordingToggle} className="record-button h-11 px-5 gap-2.5 text-[15px] font-semibold mt-2">
              <span className="w-2.5 h-2.5 rounded-full bg-[#fff]" aria-hidden="true" />
              Почати запис
            </button>
          </section>
        ) : (
          <>
            <div className="flex items-center gap-3 flex-wrap">
              <label className="sidebar-search flex items-center gap-2 h-9 px-3 w-full max-w-[360px]">
                <SearchIcon className="w-4 h-4 text-[color:var(--muted-text)] flex-shrink-0" />
                <input
                  type="search"
                  value={query}
                  onChange={(e) => setQuery(e.target.value)}
                  placeholder="Пошук за назвою"
                  aria-label="Пошук за назвою"
                  className="flex-1 min-w-0 bg-transparent text-sm outline-none placeholder:text-[color:var(--muted-text)]"
                />
                {query && (
                  <button type="button" onClick={() => setQuery('')} aria-label="Очистити пошук" className="text-[color:var(--muted-text)] hover:text-gray-900">
                    <X className="w-4 h-4" />
                  </button>
                )}
              </label>
              <div role="radiogroup" aria-label="Фільтр за статусом" className="segmented flex gap-0.5">
                {FILTERS.map((f) => (
                  <button
                    key={f.value}
                    type="button"
                    role="radio"
                    aria-checked={filter === f.value}
                    data-active={filter === f.value}
                    onClick={() => setFilter(f.value)}
                    className="segmented-item h-[30px] px-3.5 text-[13px] font-medium"
                  >
                    {f.label}
                  </button>
                ))}
              </div>
            </div>

            <section aria-label="Список записів" className="card py-2">
              {grouped.length === 0 && <p className="px-6 py-6 text-sm text-[color:var(--muted-text)]">Нічого не знайдено</p>}
              {grouped.map(({ group, items }) => (
                <div key={group}>
                  <h2 className="px-6 pt-3 pb-1 text-xs font-semibold text-[color:var(--muted-text)]">{group}</h2>
                  <ul>
                    {items.map((m) => (
                      <li key={m.id} className="group relative px-2">
                        <button
                          type="button"
                          onClick={() => open(m)}
                          onContextMenu={(e) => {
                            e.preventDefault();
                            const x = e.clientX;
                            const y = e.clientY;
                            void popupNativeMenu([
                              { text: 'Відкрити', action: () => open(m) },
                              'separator',
                              { text: 'Перейменувати…', action: () => window.dispatchEvent(new CustomEvent(RENAME_MEETING_EVENT, { detail: { id: m.id, title: m.title } })) },
                              { text: 'Видалити…', action: () => window.dispatchEvent(new CustomEvent(DELETE_MEETING_EVENT, { detail: { id: m.id } })) },
                            ]).then((shown) => {
                              if (!shown) openMenu(m, x, y);
                            });
                          }}
                          className="w-full h-14 grid grid-cols-[minmax(0,1fr)_auto_auto_112px_32px] gap-4 items-center px-4 rounded-[10px] text-left hover:bg-[var(--hover)]"
                        >
                          <span className="text-[15px] font-semibold text-gray-900 truncate">{m.title}</span>
                          <span className="text-[13px] text-[color:var(--muted-text)] whitespace-nowrap">{formatWhen(m.createdAt, now)}</span>
                          <span className="text-[13px] text-[color:var(--muted-text)] whitespace-nowrap tabular-nums w-[72px] text-right">{formatDuration(m.durationSeconds)}</span>
                          <span className="justify-self-end">
                            {m.status === 'approved' ? (
                              <span className="inline-flex items-center h-[26px] px-2.5 rounded-full bg-[var(--green-tint)] text-xs font-semibold text-[color:var(--green-text)]">Затверджено</span>
                            ) : (
                              <span className="inline-flex items-center h-[26px] px-2.5 rounded-full bg-[var(--control-fill)] text-xs font-semibold text-[color:var(--muted-text)]">Чернетка</span>
                            )}
                          </span>
                          <span aria-hidden="true" />
                        </button>
                        <button
                          type="button"
                          onClick={(e) => {
                            e.stopPropagation();
                            const rect = e.currentTarget.getBoundingClientRect();
                            openMenu(m, rect.right - 180, rect.bottom + 4);
                          }}
                          className="icon-btn-sm absolute right-4 top-1/2 -translate-y-1/2"
                          aria-label={`Дії з «${m.title}»`}
                          aria-haspopup="menu"
                        >
                          <MoreHorizontal className="w-4 h-4" />
                        </button>
                      </li>
                    ))}
                  </ul>
                </div>
              ))}
            </section>
          </>
        )}
      </div>

      {menu && (
        <div
          role="menu"
          aria-label="Дії із записом"
          className="glass-dialog fixed z-50 w-[180px] p-1.5 rounded-[10px]"
          style={{ left: Math.min(menu.x, window.innerWidth - 196), top: Math.min(menu.y, window.innerHeight - 100) }}
          onClick={(e) => e.stopPropagation()}
        >
          <button
            type="button"
            role="menuitem"
            onClick={() => {
              window.dispatchEvent(new CustomEvent(RENAME_MEETING_EVENT, { detail: { id: menu.meeting.id, title: menu.meeting.title } }));
              setMenu(null);
            }}
            className="nav-row h-8 px-2.5 gap-2 text-sm"
          >
            <Pencil className="w-4 h-4" />
            Перейменувати
          </button>
          <button
            type="button"
            role="menuitem"
            onClick={() => {
              window.dispatchEvent(new CustomEvent(DELETE_MEETING_EVENT, { detail: { id: menu.meeting.id } }));
              setMenu(null);
            }}
            className="nav-row h-8 px-2.5 gap-2 text-sm !text-[#E5352B]"
          >
            <Trash2 className="w-4 h-4" />
            Видалити
          </button>
        </div>
      )}
    </div>
  );
}

function countLabel(n: number): string {
  if (n === 0) return 'Ще немає записів';
  const mod10 = n % 10;
  const mod100 = n % 100;
  const word = mod10 === 1 && mod100 !== 11 ? 'запис' : mod10 >= 2 && mod10 <= 4 && (mod100 < 12 || mod100 > 14) ? 'записи' : 'записів';
  return `${n} ${word}`;
}
