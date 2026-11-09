'use client';

import { useCallback, useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { toast } from '@/lib/notice';

interface LearnedMeeting {
  meetingId: string;
  title: string;
  meetingType: string;
  examples: number;
  approvedAt?: string | null;
}

/**
 * «Документи й голоси в пам'яті»: approved protocols kept as examples (by meeting
 * type) and how many voices it knows by name. Corrections are listed below
 * in «Вивчені виправлення».
 */
export function LearnedOverview({ version }: { version: number }) {
  const [meetings, setMeetings] = useState<LearnedMeeting[] | null>(null);
  const [voices, setVoices] = useState(0);

  const load = useCallback(() => {
    invoke<LearnedMeeting[]>('api_learned_meetings').then(setMeetings).catch(() => setMeetings([]));
    invoke<number>('api_voice_profile_count').then(setVoices).catch(() => setVoices(0));
  }, []);
  useEffect(load, [load, version]);

  const forget = async (m: LearnedMeeting) => {
    try {
      await invoke('api_forget_learned_meeting', { meetingId: m.meetingId });
      toast.success(`Приклади з «${m.title || 'запису'}» прибрано з пам'яті`);
      load();
    } catch (e) {
      toast.error(`${e}`);
    }
  };

  const byType = new Map<string, LearnedMeeting[]>();
  for (const m of meetings ?? []) {
    byType.set(m.meetingType, [...(byType.get(m.meetingType) ?? []), m]);
  }

  return (
    <section className="card p-6 flex flex-col gap-3">
      <div className="flex flex-col gap-1">
        <h2 className="text-[15px] font-bold text-gray-900">Документи й голоси в пам&apos;яті</h2>
        <p className="text-sm text-[color:var(--muted-text)]">
          Коли секретар виправляє протокол і натискає «Затвердити», програма запам'ятовує, як він оформив кожен виступ: що
          прибрав, що дописав, як сформулював завдання і кого поставив виконавцем. Наступні записи того самого типу програма
          оформлює за цими прикладами. Голоси, які ви підписали, надалі впізнаються автоматично.
        </p>
      </div>
      <div className="flex gap-6 text-sm">
        <span>
          Затверджених протоколів: <b>{meetings?.length ?? 0}</b>
        </span>
        <span>
          Голосів, відомих за іменем: <b>{voices}</b>
        </span>
      </div>
      {[...byType.entries()].map(([type, list]) => (
        <div key={type} className="flex flex-col">
          <h3 className="text-[13px] font-semibold text-[color:var(--muted-text)] mt-1">{type}</h3>
          <ul className="flex flex-col divide-y divide-[var(--hairline,rgba(0,0,0,0.08))]">
            {list.map((m) => (
              <li key={m.meetingId} className="py-2 flex items-center justify-between gap-3">
                <span className="text-sm text-gray-900 min-w-0 truncate">
                  {m.title || 'Без назви'}
                  <span className="text-[color:var(--muted-text)]"> · прикладів: {m.examples}</span>
                </span>
                <button type="button" onClick={() => forget(m)} className="h-7 px-2 text-[12px] text-[color:var(--muted-text)] hover:underline flex-none">
                  Забути
                </button>
              </li>
            ))}
          </ul>
        </div>
      ))}
    </section>
  );
}
