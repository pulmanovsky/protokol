'use client';

/**
 * The parts of the recording capsule shared by the floating island and the
 * Google Meet window's toolbar: icons, level bars, the clock.
 */

export const BAR_WEIGHTS = [0.55, 0.85, 1, 0.75, 0.5];

export function formatClock(totalSeconds: number): string {
  const s = Math.max(0, Math.floor(totalSeconds));
  const h = Math.floor(s / 3600);
  const mm = String(Math.floor((s % 3600) / 60)).padStart(2, '0');
  const ss = String(s % 60).padStart(2, '0');
  return h > 0 ? `${h}:${mm}:${ss}` : `${mm}:${ss}`;
}

// ---------------------------------------------------------------------------
// Icons (16 px, stroke follows the text colour)
// ---------------------------------------------------------------------------

const stroke = {
  fill: 'none',
  stroke: 'currentColor',
  strokeWidth: 1.5,
  strokeLinecap: 'round' as const,
  strokeLinejoin: 'round' as const,
};

export function MicIcon({ off = false, size = 16 }: { off?: boolean; size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 16 16" {...stroke} aria-hidden="true">
      <rect x="5.5" y="1.5" width="5" height="8.5" rx="2.5" />
      <path d="M3 7.5a5 5 0 0 0 10 0M8 12.5v2" />
      {off && <path d="M2.5 2.5l11 11" />}
    </svg>
  );
}

export function CameraIcon({ off = false, size = 16 }: { off?: boolean; size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 16 16" {...stroke} aria-hidden="true">
      <rect x="1.5" y="4" width="9" height="8" rx="2" />
      <path d="M10.5 7l4-2.5v7l-4-2.5" />
      {off && <path d="M1.5 2.5l11 11" />}
    </svg>
  );
}

export function CallIcon({ off = false, size = 16 }: { off?: boolean; size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 16 16" {...stroke} aria-hidden="true">
      <path d="M2 6h2.5L8 3v10L4.5 10H2z" />
      {off ? <path d="M11 6l3.5 4M14.5 6L11 10" /> : <path d="M10.5 5.5a3.5 3.5 0 0 1 0 5M12.5 3.5a6.5 6.5 0 0 1 0 9" />}
    </svg>
  );
}

export function WarnIcon() {
  return (
    <svg width="16" height="16" viewBox="0 0 16 16" {...stroke} aria-hidden="true">
      <path d="M8 2.2l6.2 11H1.8z" />
      <path d="M8 6.5v3M8 11.6v.01" />
    </svg>
  );
}

export function PauseIcon() {
  return (
    <svg width="14" height="14" viewBox="0 0 14 14" aria-hidden="true">
      <rect x="3" y="2.5" width="2.6" height="9" rx="1" fill="currentColor" />
      <rect x="8.4" y="2.5" width="2.6" height="9" rx="1" fill="currentColor" />
    </svg>
  );
}

export function PlayIcon() {
  return (
    <svg width="14" height="14" viewBox="0 0 14 14" aria-hidden="true">
      <path d="M4 2.4v9.2l7.4-4.6z" fill="currentColor" />
    </svg>
  );
}

export function StopIcon() {
  return (
    <svg width="12" height="12" viewBox="0 0 12 12" aria-hidden="true">
      <rect x="1.5" y="1.5" width="9" height="9" rx="2" fill="currentColor" />
    </svg>
  );
}

export function Chevron({ towards }: { towards: 'right' | 'left' }) {
  return (
    <svg width="12" height="12" viewBox="0 0 12 12" {...stroke} strokeWidth={1.6} aria-hidden="true">
      <path d={towards === 'right' ? 'M4.5 2.5L8 6l-3.5 3.5' : 'M7.5 2.5L4 6l3.5 3.5'} />
    </svg>
  );
}

export function Ring() {
  return (
    <svg className="isl-ring" width="18" height="18" viewBox="0 0 18 18" aria-hidden="true">
      <circle cx="9" cy="9" r="7" fill="none" stroke="var(--isl-sep)" strokeWidth="2.4" />
      <path d="M9 2a7 7 0 0 1 6.6 4.7" fill="none" stroke="currentColor" strokeWidth="2.4" strokeLinecap="round" />
    </svg>
  );
}

export function DoneIcon() {
  return (
    <svg width="18" height="18" viewBox="0 0 18 18" aria-hidden="true">
      <circle cx="9" cy="9" r="8" fill="currentColor" />
      <path d="M5.2 9.3l2.4 2.4 5-5" fill="none" stroke="var(--isl-done-check)" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" />
    </svg>
  );
}

/** Five level bars; the weights rotate with each update so speech looks alive */
export function Bars({ percent, shift }: { percent: number; shift: number }) {
  return (
    <span className={`isl-bars ${percent <= 0 ? 'isl-bars-quiet' : ''}`} aria-hidden="true">
      {BAR_WEIGHTS.map((_, i) => {
        const weight = BAR_WEIGHTS[(i + shift) % BAR_WEIGHTS.length];
        return <i key={i} style={{ height: `${3 + Math.round((13 * percent * weight) / 100)}px` }} />;
      })}
    </span>
  );
}


export function XIcon() {
  return (
    <svg width="12" height="12" viewBox="0 0 12 12" {...stroke} strokeWidth={1.7} aria-hidden="true">
      <path d="M2.5 2.5l7 7M9.5 2.5l-7 7" />
    </svg>
  );
}
