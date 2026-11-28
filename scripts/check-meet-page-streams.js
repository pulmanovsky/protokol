// Checks what the Meet page script (frontend/src-tauri/src/meet/capture.js)
// sends of the streams' own sound before, during and after a recording.
// No Meet and no app are needed: a blank page stands in for the call with
// two tone streams, and the app's replies («a recording runs») are faked.
//
//   node scripts/check-meet-page-streams.js        (needs the `playwright` package and its Chromium)
//
// Passes when: nothing is sent with no recording; the first blocks of a
// recording reach back before its start (the page keeps a few seconds);
// blocks follow each other without holes; nothing is sent while the
// recording is paused, the blocks before the pause reach it and those after
// it reach back before the recording went on; the last blocks of a
// recording are still sent; nothing is sent after it.
const path = require('path');
const fs = require('fs');
const { chromium } = require('playwright');

const script = fs.readFileSync(path.join(__dirname, '..', 'frontend', 'src-tauri', 'src', 'meet', 'capture.js'), 'utf8');
const wait = (ms) => new Promise((r) => setTimeout(r, ms));

(async () => {
  const browser = await chromium.launch({ args: ['--autoplay-policy=no-user-gesture-required'] });
  const page = await browser.newPage();
  await page.route('https://meet.google.com/**', (route) => route.fulfill({ contentType: 'text/html', body: '<!doctype html><html><body><audio id="a"></audio><audio id="b"></audio></body></html>' }));
  await page.addInitScript(`
    window.__pushes = []; window.__rec = 'off';
    window.__TAURI_INTERNALS__ = { invoke: async (cmd, args) => {
      if (cmd !== 'plugin:meetprobe|push') return null;
      const r = args.report;
      window.__pushes.push({ t: r.t, rec: window.__rec, slots: r.slots ? r.slots.map((s) => {
        const b = atob(s.pcm); let loud = 0;
        for (let k = 0; k + 1 < b.length; k += 2) { let v = b.charCodeAt(k) | (b.charCodeAt(k + 1) << 8); if (v > 32767) v -= 65536; loud = Math.max(loud, Math.abs(v)); }
        return { i: s.i, bt: s.bt, bn: s.bn, bytes: b.length, loud };
      }) : null });
      return { rec: window.__rec, elapsed: 0, micOff: false };
    } };
  `);
  await page.addInitScript(script);
  await page.goto('https://meet.google.com/abc-defg-hij');
  await page.evaluate(() => {
    const c = new AudioContext();
    for (const [id, hz] of [['a', 220], ['b', 330]]) {
      const o = c.createOscillator(); o.frequency.value = hz;
      const g = c.createGain(); g.gain.value = 0.2;
      const d = c.createMediaStreamDestination();
      o.connect(g); g.connect(d); o.start();
      document.getElementById(id).srcObject = d.stream;
    }
  });
  await wait(13000);
  const on = Date.now();
  await page.evaluate(() => { window.__rec = 'on'; });
  await wait(8000);
  const paused = Date.now();
  await page.evaluate(() => { window.__rec = 'paused'; });
  await wait(13000);
  const resumed = Date.now();
  await page.evaluate(() => { window.__rec = 'on'; });
  await wait(8000);
  const off = Date.now();
  await page.evaluate(() => { window.__rec = 'off'; });
  await wait(7000);
  const pushes = await page.evaluate(() => window.__pushes);
  await browser.close();

  const failures = [];
  const check = (ok, what) => { if (!ok) failures.push(what); };
  const before = pushes.filter((p) => p.t < on);
  check(before.length >= 4 && before.every((p) => !p.slots), 'with no recording nothing is sent');
  const sent = pushes.filter((p) => p.slots && p.slots.length);
  check(sent.length >= 4, 'blocks are sent while the recording runs');
  for (const i of [0, 1]) {
    const blocks = [];
    for (const p of sent) for (const s of p.slots.filter((x) => x.i === i)) {
      check(s.bytes === 2 * s.bn.reduce((a, b) => a + b, 0), `stream ${i}: the sound is as long as its blocks say`);
      check(s.loud > 1000, `stream ${i}: the tone is in the sound`);
      s.bt.forEach((t, k) => blocks.push([t, s.bn[k]]));
    }
    check(blocks.length > 0 && blocks[0][0] <= on - 2000, `stream ${i}: the first blocks reach back before the start (${blocks.length ? on - blocks[0][0] : '-'} ms)`);
    check(blocks.length > 0 && blocks[0][0] >= on - 12000, `stream ${i}: but not further than the page keeps`);
    const last = blocks[blocks.length - 1];
    check(last && last[0] + last[1] / 16 >= off - 300, `stream ${i}: the last blocks of the recording are sent`);
    // One hole: the pause. What was sent before it reaches the pause, what
    // is sent after it reaches back before the recording went on.
    const holes = [];
    for (let k = 1; k < blocks.length; k++) {
      const end = blocks[k - 1][0] + blocks[k - 1][1] / 16;
      if (Math.abs(blocks[k][0] - end) > 60) holes.push([end, blocks[k][0]]);
    }
    check(holes.length === 1, `stream ${i}: blocks follow each other but for the pause (${holes.length} holes)`);
    if (holes.length === 1) {
      check(holes[0][0] >= paused - 300, `stream ${i}: the blocks before the pause reach it (${Math.round(holes[0][0] - paused)} ms)`);
      check(holes[0][1] <= resumed - 1000, `stream ${i}: the blocks after the pause reach back before it ended (${Math.round(resumed - holes[0][1])} ms)`);
      check(holes[0][1] >= paused + 3000, `stream ${i}: the pause itself is not sent (${Math.round(holes[0][1] - paused)} ms)`);
    }
  }
  const during = pushes.filter((p) => p.t > paused + 2500 && p.t < resumed);
  check(during.length >= 3 && during.every((p) => !p.slots), 'while the recording is paused nothing is sent');
  const after = pushes.filter((p) => p.t > off + 2500);
  check(after.length >= 1 && after.every((p) => !p.slots), 'after the recording nothing is sent');
  if (failures.length) {
    console.error('FAILED:\n  ' + failures.join('\n  '));
    process.exit(1);
  }
  console.log(`ok: ${pushes.length} reports, ${sent.length} with blocks`);
})();
