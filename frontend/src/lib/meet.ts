import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

/** Settings → Google Meet switches (meet/mod.rs) */
export interface MeetSettings {
  remindStart: boolean;
  offerStop: boolean;
  split: boolean;
  micSync: boolean;
}

export type MeetSettingKey = keyof MeetSettings;

/** The Meet window as the recording view shows it (meet/live.rs) */
export interface MeetLive {
  open: boolean;
  /** The calendar event the window was opened for */
  title: string | null;
  /** The page hears the call (Meet plays sound only after a click in it) */
  audioOk: boolean;
  inCall: boolean;
  participants: number;
  speaking: string[];
  /** Meet's microphone button: true off, false on, null not on the page */
  meetMic: boolean | null;
  /** Meet's camera button: true off, false on, null not on the page */
  meetCam: boolean | null;
}

/** An offer to start or end the recording when a Meet call starts or ends */
export interface MeetPrompt {
  kind: 'start' | 'stop';
  title: string | null;
}

/** How the last speaker recognition used the Meet window (speakers/meet.rs) */
export interface MeetStatus {
  used: boolean;
  accounts: number;
  opened_at_s: number | null;
  mismatch: boolean;
}

/** A Meet account and the saved person behind it (speakers/meet_names.rs) */
export interface MeetAccount {
  meetName: string;
  person: string | null;
  shared: boolean;
  linkedAt: string | null;
  lastSeen: string | null;
  voiceLike: string | null;
}

/** «Прив'язати облікові записи Google Meet до людей у пам'яті»: one row */
export interface LinkCandidate {
  meetName: string;
  label: string;
  shown: string;
  speechSeconds: number;
  person: string | null;
  reason: 'voice' | 'name' | 'maybe' | 'none' | 'shared';
  checked: boolean;
}

export interface VoiceMatch {
  name: string;
  similarity: number;
  level: 'high' | 'medium' | 'low';
}

/** Where a speaker's shown name came from (meeting_speakers.name_source) */
export type NameSource = 'voice' | 'meet_link' | 'meet_link_doubt' | 'meet_same_name' | 'meet';

const DEFAULT_SETTINGS: MeetSettings = { remindStart: true, offerStop: true, split: true, micSync: true };

export function isMeetUrl(url: string | null | undefined): boolean {
  if (!url) return false;
  try {
    const u = new URL(url);
    return u.protocol === 'https:' && u.hostname === 'meet.google.com';
  } catch {
    return false;
  }
}

export async function loadMeetSettings(): Promise<MeetSettings> {
  try {
    return await invoke<MeetSettings>('get_meet_settings');
  } catch {
    return DEFAULT_SETTINGS;
  }
}

/** The switches, read once per mount */
export function useMeetSettings(): MeetSettings {
  const [settings, setSettings] = useState<MeetSettings>(DEFAULT_SETTINGS);
  useEffect(() => {
    let alive = true;
    loadMeetSettings().then((s) => alive && setSettings(s));
    return () => {
      alive = false;
    };
  }, []);
  return settings;
}

/** Opens the Meet window (Meet's start page without a link); `title` names a recording started from it */
export async function openMeet(url?: string | null, title?: string | null): Promise<void> {
  await invoke('meet_open', { url: url ?? null, title: title ?? null });
}

/** A meeting link: a Google Meet link always opens in Protokol's Meet window,
 * never in a browser; other services (Zoom, Teams) open in their own app */
export async function openCallLink(url: string, title?: string | null): Promise<void> {
  if (isMeetUrl(url)) {
    await openMeet(url, title);
  } else {
    await invoke('open_external_url', { url });
  }
}

/** The Meet window while it is open (participants, who speaks) */
export function useMeetLive(): MeetLive {
  const [live, setLive] = useState<MeetLive>({ open: false, title: null, audioOk: true, inCall: false, participants: 0, speaking: [], meetMic: null, meetCam: null });
  useEffect(() => {
    let alive = true;
    invoke<MeetLive>('get_meet_live')
      .then((l) => alive && setLive(l))
      .catch(() => {});
    const un = listen<MeetLive>('meet-live', (e) => setLive(e.payload));
    return () => {
      alive = false;
      un.then((f) => f());
    };
  }, []);
  return live;
}

/** «учасників: 6» */
export function participantsText(n: number): string {
  return `учасників: ${n}`;
}
