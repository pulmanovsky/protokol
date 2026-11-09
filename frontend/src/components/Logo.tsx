import React from "react";

// Logo geometry (64×64): dark rounded square, white П centred at 32,32, red dot on the baseline
const LOGO_TILE = "#1D1D1F";
const LOGO_RED = "#E5352B";

export function LogoMark({ size = 24 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 64 64" aria-hidden="true" className="flex-shrink-0">
      <rect width="64" height="64" rx="14" fill={LOGO_TILE} />
      {/* Hairline edge keeps the dark tile visible on the dark sidebar */}
      <rect x="0.75" y="0.75" width="62.5" height="62.5" rx="13.25" fill="none" className="stroke-white/0 dark:stroke-white/15" strokeWidth="1.5" />
      <path d="M21 20h22v24h-6.5V26h-9v18H21z" fill="#FFFFFF" />
      <circle cx="50" cy="40.5" r="3.5" fill={LOGO_RED} />
    </svg>
  );
}

export function Wordmark() {
  return (
    <span className="text-[17px] font-bold text-gray-900" style={{ letterSpacing: "-0.2px" }}>
      протокол<span style={{ color: LOGO_RED }}>.</span>
    </span>
  );
}

interface LogoProps {
  isCollapsed: boolean;
}

const Logo = ({ isCollapsed }: LogoProps) =>
  isCollapsed ? (
    <span role="img" aria-label="протокол">
      <LogoMark size={28} />
    </span>
  ) : (
    <span className="flex items-center gap-2">
      <LogoMark size={24} />
      <Wordmark />
    </span>
  );

export default Logo;
