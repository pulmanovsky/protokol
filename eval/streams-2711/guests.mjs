// Chrome guests for the captions test call. Each guest's microphone is a WebAudio
// stream (getUserMedia replaced before Meet loads), so every spoken turn is a known clip
// played at a logged time.
//   node guests.mjs launch <meet-code>   start 3 Chromes, open the call, join as «Гість N» (asks to join)
//   node guests.mjs state                what each guest's page shows
//   node guests.mjs mics on|off          all guests' Meet microphones
//   node guests.mjs speak                play turns.tsv in order, one guest at a time; timeline.jsonl
//   node guests.mjs stop                 close the guests
import fs from 'fs';
import { spawn, execSync } from 'child_process';
const D = new URL('.', import.meta.url).pathname;
const ALL = [9351, 9352, 9353, 9355, 9356];
const ports = ALL;
const only = (process.env.ONLY || '1,2,3,4,5').split(',').map(Number);
const CHROME = '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome';
const sleep = ms => new Promise(r => setTimeout(r, ms));

const INJECT = `(() => {
  if (window.__protokolGuest || window.top !== window) return; window.__protokolGuest = true;
  let ctx = null, dest = null;
  const setup = () => {
    if (ctx) return;
    ctx = new AudioContext({ sampleRate: 48000 }); dest = ctx.createMediaStreamDestination();
    const osc = ctx.createOscillator(); const g = ctx.createGain(); g.gain.value = 0.0001; osc.connect(g).connect(dest); osc.start();
  };
  window.__play = async (b64) => {
    setup(); await ctx.resume();
    const bytes = Uint8Array.from(atob(b64), c => c.charCodeAt(0));
    const buf = await ctx.decodeAudioData(bytes.buffer);
    const src = ctx.createBufferSource(); src.buffer = buf; src.connect(dest); src.start();
    return buf.duration;
  };
  const md = navigator.mediaDevices; if (!md) return; const orig = md.getUserMedia.bind(md);
  md.getUserMedia = async (c) => {
    const s = await orig(c);
    try {
      if (c && c.audio && s.getAudioTracks().length) {
        setup();
        const t = dest.stream.getAudioTracks()[0].clone();
        for (const a of s.getAudioTracks()) { s.removeTrack(a); a.stop(); }
        s.addTrack(t); ctx.resume();
      }
    } catch (e) { console.warn('protokol guest hook', e); }
    return s;
  };
})();`;

async function connect(port) {
  const list = await (await fetch(`http://127.0.0.1:${port}/json`)).json();
  const page = list.find(p => p.type === 'page');
  const ws = new WebSocket(page.webSocketDebuggerUrl); let id = 0; const pend = new Map();
  ws.onmessage = e => { const m = JSON.parse(e.data); if (m.id && pend.has(m.id)) { pend.get(m.id)(m); pend.delete(m.id); } };
  await new Promise(r => ws.onopen = r);
  const send = (method, params = {}) => new Promise(r => { const i = ++id; pend.set(i, r); ws.send(JSON.stringify({ id: i, method, params })); });
  const ev = async e => (await send('Runtime.evaluate', { expression: e, returnByValue: true, awaitPromise: true })).result?.result?.value;
  return { send, ev, ws };
}
const findMic = `([...document.querySelectorAll('button[data-is-muted]')].find(x=>/microphone|мікрофон/i.test(x.getAttribute('aria-label')||''))||[...document.querySelectorAll('button[aria-label]')].find(x=>/^turn (on|off) microphone/i.test(x.getAttribute('aria-label'))))`;
const micState = `(()=>{const b=${findMic}; if(!b) return 'none'; const m=b.getAttribute('data-is-muted'); return m!=null ? (m==='true'?'muted':'on') : (/turn off/i.test(b.getAttribute('aria-label'))?'on':'muted');})()`;
const setMic = on => `(async()=>{for(let k=0;k<4;k++){const b=${findMic}; if(!b) return 'none'; const m=b.getAttribute('data-is-muted'); const isOn = m!=null ? m!=='true' : /turn off/i.test(b.getAttribute('aria-label')); if(isOn===${on}) return ${on}?'on':'muted'; b.click(); await new Promise(r=>setTimeout(r,700));} return 'FAILED';})()`;

const [mode, arg] = process.argv.slice(2);
if (mode === 'launch') {
  const code = arg; if (!code) throw new Error('meet code');
  for (const [i, port] of ports.entries()) {
    if (!only.includes(i + 1)) continue;
    spawn(CHROME, [`--user-data-dir=/tmp/protokol-cap-guest-${i + 1}`, `--remote-debugging-port=${port}`, '--no-first-run', '--no-default-browser-check',
      '--use-fake-ui-for-media-stream', '--use-fake-device-for-media-stream', `--use-file-for-fake-audio-capture=${D}silence.wav`,
      '--autoplay-policy=no-user-gesture-required', '--lang=uk', '--window-size=640,480', `--window-position=${100 + i * 60},${100 + i * 60}`, 'about:blank'],
      { detached: true, stdio: 'ignore' }).unref();
  }
  await sleep(6000);
  for (const [i, port] of ports.entries()) {
    if (!only.includes(i + 1)) continue;
    const c = await connect(port);
    await c.send('Page.enable');
    await c.send('Page.addScriptToEvaluateOnNewDocument', { source: INJECT });
    await c.send('Page.navigate', { url: `https://meet.google.com/${code}` });
    globalThis.keep = (globalThis.keep || []).concat(c);
  }
  await sleep(12000);
}
if (mode === 'launch' || mode === 'join') {
  for (const [i, port] of ports.entries()) {
    if (!only.includes(i + 1)) continue;
    const c = await connect(port);
    const mic = await c.ev(setMic(true));
    await c.ev(`(()=>{const b=[...document.querySelectorAll('button')].find(x=>/Continue as guest|Продовжити як гість/i.test(x.innerText)); if(b) b.click();})()`);
    await sleep(2500);
    const inp = await c.ev(`(()=>{const i=[...document.querySelectorAll('input')].find(x=>x.offsetParent); if(!i) return 'no input'; i.focus(); return 'ok';})()`);
    if (inp === 'ok') await c.send('Input.insertText', { text: `Гість ${i + 1}` });
    await sleep(800);
    const asked = await c.ev(`(()=>{const b=[...document.querySelectorAll('button')].find(x=>/Ask to join|Попросити|Join now|Приєднатися/i.test(x.innerText)); if(!b) return 'no button'; b.click(); return 'asked: '+b.innerText.trim();})()`);
    const injected = await c.ev('!!window.__play');
    console.log(`Гість ${i + 1}: mic ${mic}, name ${inp}, ${asked}, audio hook ${injected}`);
    c.ws.close();
  }
}

if (mode === 'overlap') {
  // turns where a second guest cuts in: overlap.tsv (turn, guest, offset s, text); clips/oN.wav in file order
  const g = await Promise.all(ports.map(connect));
  const rows = fs.readFileSync(D + 'overlap.tsv', 'utf8').trim().split('\n').map(l => l.split('\t'));
  const out = fs.createWriteStream(D + 'timeline.jsonl', { flags: 'a' });
  const turns = [...new Set(rows.map(r => r[0]))];
  let k = 0;
  for (const t of turns) {
    const parts = rows.filter(r => r[0] === t).map(r => ({ guest: Number(r[1]), off: Number(r[2]), text: r[3], clip: ++k }));
    if (process.env.TWO) for (const p of parts) if (p.guest === 3) p.guest = parts.some(q => q.guest === 1) ? 2 : 1;
    const t0 = Date.now(); let end = 0;
    await Promise.all(parts.map(async p => {
      await sleep(p.off * 1000);
      const from = new Date().toISOString();
      const b64 = fs.readFileSync(`${D}clips/o${p.clip}.wav`).toString('base64');
      const dur = await g[p.guest - 1].ev(`window.__play(${JSON.stringify(b64)})`);
      end = Math.max(end, p.off + dur);
      out.write(JSON.stringify({ turn: Number(t), guest: p.guest, kind: 'overlap', from, seconds: dur, text: p.text }) + '\n');
      console.log(from, 'turn', t, 'Гість', p.guest, 'overlap', dur?.toFixed?.(1));
    }));
    await sleep(Math.max(0, (end + 1.5) * 1000 - (Date.now() - t0)));
  }
  out.end(); console.log('done');
}
if (mode === 'listen') {
  // A 4th guest «Слухач» with Protokol's page script (test slots on) in place of the app's
  // window: its reports land in listen/report.jsonl, each stream's sound in listen/slots/
  const code = arg; const port = Number(process.env.LISTEN_PORT || 9354); const L = D + (process.env.LISTEN_DIR || 'listen') + '/'; const who = process.env.LISTEN_NAME || 'Слухач';
  fs.mkdirSync(L + 'slots', { recursive: true });
  const capture = fs.readFileSync(process.env.CAPTURE_JS, 'utf8');
  const shim = `${process.env.LISTEN_FLAGS ?? 'window.__protokolSlots = true;'} window.__reps = []; window.__TAURI_INTERNALS__ = { invoke: (cmd, a) => { if (cmd === 'plugin:meetprobe|push') window.__reps.push(a.report); return Promise.resolve({}); } };`;
  spawn(CHROME, [`--user-data-dir=/tmp/protokol-cap-guest-${port}`, `--remote-debugging-port=${port}`, '--no-first-run', '--no-default-browser-check',
    '--use-fake-ui-for-media-stream', '--use-fake-device-for-media-stream', `--use-file-for-fake-audio-capture=${D}silence.wav`,
    '--autoplay-policy=no-user-gesture-required', '--lang=uk', '--window-size=900,600', '--window-position=400,100', 'about:blank'],
    { detached: true, stdio: 'ignore' }).unref();
  await sleep(6000);
  let c = await connect(port);
  await c.send('Page.enable');
  await c.send('Page.addScriptToEvaluateOnNewDocument', { source: `if (window.top === window) { ${shim}\n${capture} }` });
  await c.send('Page.navigate', { url: `https://meet.google.com/${code}` });
  await sleep(12000);
  const mic = await c.ev(setMic(false));
  await c.ev(`(()=>{const b=[...document.querySelectorAll('button')].find(x=>/Continue as guest|Продовжити як гість/i.test(x.innerText)); if(b) b.click();})()`);
  await sleep(2500);
  const inp = await c.ev(`(()=>{const i=[...document.querySelectorAll('input')].find(x=>x.offsetParent); if(!i) return 'no input'; i.focus(); return 'ok';})()`);
  if (inp === 'ok') await c.send('Input.insertText', { text: who });
  await sleep(800);
  const asked = await c.ev(`(()=>{const b=[...document.querySelectorAll('button')].find(x=>/Ask to join|Попросити|Join now|Приєднатися/i.test(x.innerText)); if(!b) return 'no button'; b.click(); return 'asked';})()`);
  console.log(`${who}: mic ${mic}, name ${inp}, ${asked}, script ${await c.ev('typeof window.__reps')}`);
  const rep = fs.createWriteStream(L + 'report.jsonl', { flags: 'a' });
  while (!fs.existsSync(L + 'STOP')) {
    await sleep(2000);
    let reps;
    try { reps = await c.ev('JSON.stringify(window.__reps.splice(0))'); } catch { try { c = await connect(port); } catch {} continue; }
    for (const r of JSON.parse(reps || '[]')) {
      for (const s of r.slots || []) {
        fs.appendFileSync(`${L}slots/slot-${s.i}.pcm`, Buffer.from(s.pcm, 'base64'));
        fs.appendFileSync(`${L}slots/slot-${s.i}.idx`, s.bt.map((t, j) => `${t} ${s.bn[j]}\n`).join(''));
      }
      delete r.slots;
      rep.write(JSON.stringify(r) + '\n');
    }
  }
  rep.end(); console.log('listen stopped');
}
if (mode === 'state') {
  for (const [i, port] of ports.entries()) {
    const c = await connect(port);
    console.log(`Гість ${i + 1}:`, await c.ev(micState), '| hook', await c.ev('!!window.__play'), '|', (await c.ev('document.body.innerText.slice(0,200)')).replace(/\n/g, ' / '));
    c.ws.close();
  }
} else if (mode === 'mics') {
  for (const [i, port] of ports.entries()) { const c = await connect(port); console.log(`Гість ${i + 1}:`, await c.ev(setMic(arg === 'on'))); c.ws.close(); }
} else if (mode === 'speak') {
  const g = await Promise.all(ports.map(connect));
  const turns = fs.readFileSync(D + 'turns.tsv', 'utf8').trim().split('\n').map(l => l.split('\t'));
  const out = fs.createWriteStream(D + 'timeline.jsonl', { flags: 'a' });
  const maxTurn = Number(process.env.MAXTURN || 99);
  for (const [n, kind, text] of turns) {
    if (Number(n) > maxTurn) break;
    let who = (Number(n) - 1) % 3;
    if (process.env.TWO && who === 2) who = Number(n) % 2;
    const b64 = fs.readFileSync(`${D}clips/t${n}.wav`).toString('base64');
    const mics = []; for (const c of g) mics.push(await c.ev(micState));
    const from = new Date().toISOString();
    const dur = await g[who].ev(`window.__play(${JSON.stringify(b64)})`);
    await sleep(Math.round((dur || 20) * 1000) + 3000);
    const rec = { turn: Number(n), guest: who + 1, kind, from, seconds: dur, to: new Date().toISOString(), mics, text };
    out.write(JSON.stringify(rec) + '\n');
    console.log(from, 'turn', n, 'Гість', who + 1, kind, dur?.toFixed?.(1), mics.join(','));
  }
  out.end(); console.log('done');
} else if (mode === 'stop') {
  try { execSync('pkill -f "user-data-dir=/tmp/protokol-cap-guest-"'); } catch {}
}
process.exit(0);
