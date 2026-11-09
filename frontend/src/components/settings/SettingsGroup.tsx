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
