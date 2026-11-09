import { invoke } from '@tauri-apps/api/core';

/*
 * A probe for test runs only (the app started with PROTOKOL_PERF=1): what the
 * page spends while following the sound, written to the app's log every 10 s.
 * Off for everyone else: one boolean check.
 */
let on = false;
const sums = new Map<string, { ms: number; n: number }>();
let started = 0;

if (typeof window !== 'undefined') {
  invoke<boolean>('api_perf_enabled')
    .then((v) => {
      on = v;
      if (!on) return;
      started = performance.now();
      window.setInterval(() => {
        const secs = (performance.now() - started) / 1000;
        const parts = [...sums.entries()].map(([k, v]) => `${k} ${v.ms.toFixed(1)}ms/${v.n} (${(v.ms / secs).toFixed(2)} ms/s)`);
        void invoke('api_perf_log', { text: `${secs.toFixed(0)}s: ${parts.join(' · ')}` }).catch(() => {});
        sums.clear();
        started = performance.now();
      }, 10000);
    })
    .catch(() => {});
}

export function perfOn(): boolean {
  return on;
}

/** Adds a measured stretch; with `layout`, also what the browser then spends on style and layout */
export function perfAdd(name: string, ms: number, layout = false) {
  if (!on) return;
  const s = sums.get(name) ?? { ms: 0, n: 0 };
  s.ms += ms;
  s.n += 1;
  sums.set(name, s);
  if (layout) {
    const t0 = performance.now();
    void document.body.offsetHeight;
    perfAdd('style+layout', performance.now() - t0);
  }
}

/** Counts something (renders) */
export function perfCount(name: string) {
  if (!on) return;
  const s = sums.get(name) ?? { ms: 0, n: 0 };
  s.n += 1;
  sums.set(name, s);
}
