'use client';

import React, { createContext, useCallback, useContext, useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { getCurrentWindow } from '@tauri-apps/api/window';

export type ThemeMode = 'system' | 'light' | 'dark';
export type ResolvedTheme = 'light' | 'dark';

export const THEME_STORAGE_KEY = 'protokol-theme';

/**
 * Inline script for <head>: applies the saved theme before first paint so the
 * window never flashes light when the user picked dark.
 */
export const themeBootScript = `(function(){try{var m=localStorage.getItem('${THEME_STORAGE_KEY}')||'system';var d=m==='dark'||(m==='system'&&window.matchMedia('(prefers-color-scheme: dark)').matches);document.documentElement.classList.toggle('dark',d);document.documentElement.style.colorScheme=d?'dark':'light';}catch(e){}})();`;

interface ThemeContextValue {
  mode: ThemeMode;
  resolved: ResolvedTheme;
  setMode: (mode: ThemeMode) => void;
}

const ThemeContext = createContext<ThemeContextValue | null>(null);

function readStoredMode(): ThemeMode {
  try {
    const value = localStorage.getItem(THEME_STORAGE_KEY);
    if (value === 'light' || value === 'dark' || value === 'system') return value;
  } catch {
    // Storage unavailable — fall back to following the system
  }
  return 'system';
}

function systemPrefersDark(): boolean {
  return typeof window !== 'undefined' && window.matchMedia('(prefers-color-scheme: dark)').matches;
}

export function ThemeProvider({ children }: { children: React.ReactNode }) {
  const [mode, setModeState] = useState<ThemeMode>('system');
  const [systemDark, setSystemDark] = useState(false);

  useEffect(() => {
    setModeState(readStoredMode());
    setSystemDark(systemPrefersDark());

    const media = window.matchMedia('(prefers-color-scheme: dark)');
    const onChange = (e: MediaQueryListEvent) => setSystemDark(e.matches);
    media.addEventListener('change', onChange);
    return () => media.removeEventListener('change', onChange);
  }, []);

  const resolved: ResolvedTheme = mode === 'dark' || (mode === 'system' && systemDark) ? 'dark' : 'light';

  useEffect(() => {
    const root = document.documentElement;
    root.classList.toggle('dark', resolved === 'dark');
    root.style.colorScheme = resolved;
  }, [resolved]);

  // Keep the native macOS vibrancy behind the webview in step with the chosen theme
  useEffect(() => {
    getCurrentWindow()
      .setTheme(mode === 'system' ? null : mode)
      .catch((error) => console.warn('[Theme] Failed to set native window theme:', error));
    // The Google Meet window's frame follows the app too
    invoke('meet_set_theme', { theme: mode === 'system' ? null : mode }).catch(() => {});
  }, [mode]);

  const setMode = useCallback((next: ThemeMode) => {
    setModeState(next);
    try {
      localStorage.setItem(THEME_STORAGE_KEY, next);
    } catch {
      // Not persisted; the choice still applies for this session
    }
  }, []);

  return <ThemeContext.Provider value={{ mode, resolved, setMode }}>{children}</ThemeContext.Provider>;
}

export function useTheme(): ThemeContextValue {
  const context = useContext(ThemeContext);
  if (!context) throw new Error('useTheme must be used within a ThemeProvider');
  return context;
}
