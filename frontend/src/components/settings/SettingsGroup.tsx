import React from 'react';

/**
 * macOS-style grouped list: optional title above, one card with 44px rows
 * separated by hairlines, help text under the card.
 */
export function SettingsGroup({
  title,
  help,
  children,
}: {
  title?: string;
  help?: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <section className="flex flex-col gap-1.5">
      {title && <h3 className="settings-group-title">{title}</h3>}
      <div className="settings-list">{children}</div>
      {help && <div className="settings-help">{help}</div>}
    </section>
  );
}

/** One row: label on the left, control on the right */
export function SettingsRow({
  label,
  htmlFor,
  children,
}: {
  label: React.ReactNode;
  htmlFor?: string;
  children?: React.ReactNode;
}) {
  return (
    <div className="settings-row">
      {htmlFor ? (
        <label htmlFor={htmlFor} className="min-w-0">
          {label}
        </label>
      ) : (
        <span className="min-w-0">{label}</span>
      )}
      {children && <div className="flex items-center gap-2 flex-shrink-0">{children}</div>}
    </div>
  );
}

/** A full-width block inside a group (pickers, editors) that does not fit a single row */
export function SettingsBlock({ children }: { children: React.ReactNode }) {
  return <div className="settings-block">{children}</div>;
}

/** A row that opens something: a page, a form or a link (› or ↗ on the right) */
export function SettingsLinkRow({
  label,
  sub,
  onClick,
  external = false,
}: {
  label: React.ReactNode;
  sub?: React.ReactNode;
  onClick: () => void;
  external?: boolean;
}) {
  return (
    <button type="button" className="settings-row w-full text-left hover:bg-[var(--hover)]" onClick={onClick}>
      <span className="flex flex-col min-w-0">
        <span>{label}</span>
        {sub && <span className="text-xs text-[color:var(--muted-text)]">{sub}</span>}
      </span>
      <span className="flex-none text-[color:var(--muted-text)]" aria-hidden="true">
        {external ? '↗' : '›'}
      </span>
    </button>
  );
}
