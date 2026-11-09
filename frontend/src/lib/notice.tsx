'use client';

/**
 * One glass notice at the top centre of the content column.
 *
 * Drop-in replacement for the `toast` API of sonner that the app used
 * (`toast`, `.success`, `.error`, `.info`, `.warning`, `.message`, `.loading`,
 * `.custom`, `.dismiss`, `.promise`), so call sites keep their code.
 *
 * Rules (design canvas «Поява плашки»):
 * - one notice at a time; a new one cross-fades over the previous one;
 * - 40 px high (56 px with a second line), 240–480 px wide;
 * - in: 0.28 s, slight 6 px slide and fade; out: 0.2 s fade;
 * - hides after 4 s; errors stay until closed; nothing hides under the pointer;
 * - glass like the toolbar, colour only in the small icon, no sound.
 */

import React, { useCallback, useEffect, useLayoutEffect, useRef, useState, useSyncExternalStore } from 'react';

type Kind = 'plain' | 'done' | 'info' | 'warning' | 'error' | 'progress' | 'custom';
type Id = string | number;

interface NoticeAction {
  label: React.ReactNode;
  onClick: (event: React.MouseEvent<HTMLButtonElement>) => void;
}

export interface NoticeOptions {
  id?: Id;
  description?: React.ReactNode;
  duration?: number;
  action?: NoticeAction | React.ReactNode;
  cancel?: NoticeAction | React.ReactNode;
  icon?: React.ReactNode;
  onDismiss?: (n: { id: Id }) => void;
  onAutoClose?: (n: { id: Id }) => void;
  /** Accepted for compatibility; the place is always the same */
  position?: string;
  [key: string]: unknown;
}

interface Notice {
  id: Id;
  key: number;
  kind: Kind;
  title: React.ReactNode;
  options: NoticeOptions;
  render?: (id: Id) => React.ReactNode;
}

const DEFAULT_DURATION = 4000;
const DONE_DURATION = 3000;

let current: Notice | null = null;
let serial = 0;
const listeners = new Set<() => void>();

function emit() {
  listeners.forEach((listener) => listener());
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

function getSnapshot() {
  return current;
}

function show(kind: Kind, title: React.ReactNode, options: NoticeOptions = {}, render?: Notice['render']): Id {
  serial += 1;
  const id = options.id ?? `notice-${serial}`;
  current = { id, key: serial, kind, title, options, render };
  emit();
  return id;
}

function dismiss(id?: Id) {
  if (!current) return;
  if (id === undefined || current.id === id) {
    const gone = current;
    current = null;
    emit();
    gone.options.onDismiss?.({ id: gone.id });
  }
}

type Content = React.ReactNode | (() => React.ReactNode);

function text(content: Content): React.ReactNode {
  return typeof content === 'function' ? (content as () => React.ReactNode)() : content;
}

interface PromiseMessages<T> {
  loading?: React.ReactNode;
  success?: React.ReactNode | ((value: T) => React.ReactNode);
  error?: React.ReactNode | ((error: unknown) => React.ReactNode);
}

function promise<T>(work: Promise<T> | (() => Promise<T>), messages: PromiseMessages<T>, options: NoticeOptions = {}) {
  const id = show('progress', messages.loading ?? 'Зачекайте…', options);
  const run = typeof work === 'function' ? work() : work;
  run.then(
    (value) => {
      if (messages.success !== undefined) {
        const title = typeof messages.success === 'function' ? messages.success(value) : messages.success;
        show('done', title, { ...options, id });
      } else {
        dismiss(id);
      }
    },
    (error) => {
      if (messages.error !== undefined) {
        const title = typeof messages.error === 'function' ? messages.error(error) : messages.error;
        show('error', title, { ...options, id });
      } else {
        dismiss(id);
      }
    },
  );
  return id;
}

type ToastFn = ((title: Content, options?: NoticeOptions) => Id) & {
  success: (title: Content, options?: NoticeOptions) => Id;
  error: (title: Content, options?: NoticeOptions) => Id;
  info: (title: Content, options?: NoticeOptions) => Id;
  warning: (title: Content, options?: NoticeOptions) => Id;
  message: (title: Content, options?: NoticeOptions) => Id;
  loading: (title: Content, options?: NoticeOptions) => Id;
  custom: (render: (id: Id) => React.ReactNode, options?: NoticeOptions) => Id;
  dismiss: (id?: Id) => void;
  promise: typeof promise;
};

export const toast: ToastFn = Object.assign(
  (title: Content, options?: NoticeOptions) => show('plain', text(title), options),
  {
    success: (title: Content, options?: NoticeOptions) => show('done', text(title), options),
    error: (title: Content, options?: NoticeOptions) => show('error', text(title), options),
    info: (title: Content, options?: NoticeOptions) => show('info', text(title), options),
    warning: (title: Content, options?: NoticeOptions) => show('warning', text(title), options),
    message: (title: Content, options?: NoticeOptions) => show('plain', text(title), options),
    loading: (title: Content, options?: NoticeOptions) => show('progress', text(title), options),
    custom: (render: (id: Id) => React.ReactNode, options?: NoticeOptions) => show('custom', null, options, render),
    dismiss,
    promise,
  },
);

/* ------------------------------------------------------------------ */
/* Wording: a plain sentence on top, technical text behind «Подробиці» */

const CYRILLIC = /[А-Яа-яІіЇїЄєҐґ]/;

interface Split {
  title: React.ReactNode;
  line: React.ReactNode | null;
  details: string | null;
}

function isTechnical(value: string) {
  return !CYRILLIC.test(value) || value.length > 140 || /\b(at |Error:|panicked|stack|errno|os error|0x[0-9a-f]+)/i.test(value);
}

function splitText(notice: Notice): Split {
  const { title, options, kind } = notice;
  const description = options.description;
  let head: React.ReactNode = title;
  let details: string | null = null;
  let line: React.ReactNode | null = null;

  if (typeof title === 'string') {
    const trimmed = title.trim();
    const colon = trimmed.indexOf(': ');
    if (kind === 'error' && colon > 0 && isTechnical(trimmed.slice(colon + 2))) {
      head = trimmed.slice(0, colon);
      details = trimmed.slice(colon + 2);
    } else if (kind === 'error' && (trimmed === '' || isTechnical(trimmed))) {
      head = 'Не вдалося виконати дію';
      details = trimmed || null;
    }
  }

  if (typeof description === 'string') {
    if (isTechnical(description)) {
      details = details ? `${details}\n${description}` : description;
    } else {
      line = description;
    }
  } else if (description !== undefined && description !== null) {
    line = description;
  }

  return { title: head, line, details };
}

/* ------------------------------------------------------------------ */
/* Icons: small, colour only here                                       */

function Icon({ kind }: { kind: Kind }) {
  switch (kind) {
    case 'done':
      return (
        <svg className="notice-icon" width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
          <path d="M3 8.5l3 3 7-7" />
        </svg>
      );
    case 'info':
      return (
        <svg className="notice-icon notice-icon-muted" width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" aria-hidden="true">
          <circle cx="8" cy="8" r="6.25" />
          <path d="M8 7.2v4M8 4.9v.1" />
        </svg>
      );
    case 'warning':
    case 'error':
      return (
        <svg className={`notice-icon ${kind === 'error' ? 'notice-icon-error' : 'notice-icon-warning'}`} width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
          <path d="M8 2.2 14.5 13.5h-13z" />
          <path d="M8 6.5v3.2M8 11.6v.1" />
        </svg>
      );
    case 'progress':
      return (
        <svg className="notice-icon notice-spinner" width="16" height="16" viewBox="0 0 16 16" fill="none" aria-hidden="true">
          <circle cx="8" cy="8" r="6" stroke="currentColor" strokeOpacity="0.2" strokeWidth="2" />
          <path d="M8 2a6 6 0 0 1 6 6" stroke="currentColor" strokeWidth="2" strokeLinecap="round" />
        </svg>
      );
    default:
      return null;
  }
}

function isAction(value: unknown): value is NoticeAction {
  return !!value && typeof value === 'object' && 'label' in (value as object) && 'onClick' in (value as object);
}

/* ------------------------------------------------------------------ */
/* One notice                                                          */

function NoticeView({ notice, leaving }: { notice: Notice; leaving: boolean }) {
  const [hovered, setHovered] = useState(false);
  const [open, setOpen] = useState(false);
  const [copied, setCopied] = useState(false);
  const { kind, options } = notice;
  const split = splitText(notice);

  const duration =
    options.duration ??
    (kind === 'error' || kind === 'progress' || kind === 'custom' ? Infinity : kind === 'done' ? DONE_DURATION : DEFAULT_DURATION);

  // Hide after `duration`, paused while the pointer is over the notice or details are open
  const remaining = useRef(duration);
  useEffect(() => {
    if (leaving || hovered || open || !Number.isFinite(remaining.current)) return;
    const started = Date.now();
    const timer = setTimeout(() => {
      options.onAutoClose?.({ id: notice.id });
      dismiss(notice.id);
    }, remaining.current);
    return () => {
      clearTimeout(timer);
      remaining.current -= Date.now() - started;
    };
  }, [hovered, open, leaving, notice.id, options]);

  const runAction = (action: NoticeAction) => (event: React.MouseEvent<HTMLButtonElement>) => {
    action.onClick(event);
    if (!event.defaultPrevented) dismiss(notice.id);
  };

  const copyDetails = async () => {
    if (!split.details) return;
    try {
      await navigator.clipboard.writeText(split.details);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      // Copy is a convenience
    }
  };

  if (kind === 'custom' && notice.render) {
    return (
      <div
        className={`notice notice-custom ${leaving ? 'notice-leaving' : ''}`}
        onMouseEnter={() => setHovered(true)}
        onMouseLeave={() => setHovered(false)}
      >
        {notice.render(notice.id)}
      </div>
    );
  }

  const twoLines = !!split.line || open;
  const closable = kind === 'error' || kind === 'warning' || !Number.isFinite(duration);

  return (
    <div
      role={kind === 'error' ? 'alert' : 'status'}
      aria-live={kind === 'error' ? 'assertive' : 'polite'}
      className={`notice notice-${kind} ${twoLines ? 'notice-two' : ''} ${open ? 'notice-open' : ''} ${leaving ? 'notice-leaving' : ''}`}
      onMouseEnter={() => setHovered(true)}
      onMouseLeave={() => setHovered(false)}
    >
      <div className="notice-row">
        {options.icon ? <span className="notice-icon">{options.icon}</span> : <Icon kind={kind} />}
        <div className="notice-text">
          <span className="notice-title">{split.title}</span>
          {split.line ? <span className="notice-line">{split.line}</span> : null}
        </div>
        {split.details && !open ? (
          <button type="button" className="notice-button notice-button-quiet" onClick={() => setOpen(true)}>
            Подробиці
          </button>
        ) : null}
        {isAction(options.cancel) ? (
          <button type="button" className="notice-button notice-button-quiet" onClick={runAction(options.cancel)}>
            {options.cancel.label}
          </button>
        ) : null}
        {isAction(options.action) ? (
          <button type="button" className="notice-button" onClick={runAction(options.action)}>
            {options.action.label}
          </button>
        ) : options.action ? (
          options.action
        ) : null}
        {closable ? (
          <button type="button" className="notice-close" aria-label="Закрити" onClick={() => dismiss(notice.id)}>
            <svg width="10" height="10" viewBox="0 0 10 10" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" aria-hidden="true">
              <path d="M2 2l6 6M8 2 2 8" />
            </svg>
          </button>
        ) : null}
      </div>
      {open && split.details ? (
        <div className="notice-details">
          <pre>{split.details}</pre>
          <button type="button" className="notice-button notice-button-quiet" onClick={copyDetails}>
            {copied ? 'Скопійовано' : 'Копіювати'}
          </button>
        </div>
      ) : null}
    </div>
  );
}

/* ------------------------------------------------------------------ */
/* Host: centred over the content column, under the page bars          */

interface Shown {
  notice: Notice;
  leaving: boolean;
}

export function NoticeHost({ left = 0 }: { left?: number }) {
  const latest = useSyncExternalStore(subscribe, getSnapshot, () => null);
  const [shown, setShown] = useState<Shown[]>([]);

  const settle = useCallback((key: number) => {
    setShown((items) => items.filter((item) => !(item.leaving && item.notice.key === key)));
  }, []);

  useEffect(() => {
    setShown((items) => {
      const kept = items
        .filter((item) => !item.leaving)
        .filter((item) => !latest || item.notice.key !== latest.key)
        .map((item) => ({ ...item, leaving: true }));
      kept.forEach((item) => setTimeout(() => settle(item.notice.key), 200));
      return latest ? [...kept, { notice: latest, leaving: false }] : kept;
    });
  }, [latest, settle]);

  // Under the page's bars: every bar marked data-notice-anchor in the top
  // half of the window, so a notice never covers a button
  const [top, setTop] = useState(16);
  const visible = shown.length > 0;
  useLayoutEffect(() => {
    if (!visible) return;
    const place = () => {
      let bottom = 0;
      document.querySelectorAll<HTMLElement>('[data-notice-anchor]').forEach((el) => {
        const r = el.getBoundingClientRect();
        if (r.height > 0 && r.bottom < window.innerHeight / 2) bottom = Math.max(bottom, r.bottom);
      });
      setTop(bottom > 0 ? Math.round(bottom + 8) : 16);
    };
    place();
    window.addEventListener('resize', place);
    return () => window.removeEventListener('resize', place);
  }, [visible, latest]);

  if (!visible) return null;

  return (
    <div className="notice-host" style={{ left, top }}>
      <div className="notice-stack">
        {shown.map(({ notice, leaving }) => (
          <NoticeView key={notice.key} notice={notice} leaving={leaving} />
        ))}
      </div>
    </div>
  );
}
