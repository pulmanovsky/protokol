// Protokol: who speaks in a Google Meet call.
//
// Runs inside the Meet page opened in Protokol's own window. It LOOKS at
// which audio streams the page plays, how loud each one is every 100 ms,
// which participant tiles are lit as speaking, the tile names, whether
// Meet's own microphone button is on and the text of Meet's captions. Every
// 2 s it hands a report to Protokol
// on this Mac (plugin:meetprobe|push), and the microphone state also at once
// when it changes (plugin:meetprobe|mic), so the recording takes the
// secretary's microphone only while it is on in Meet. Nothing is sent
// anywhere else: what Protokol sees is shown in its own toolbar above the
// page. On the page itself it only presses Meet's own buttons: the
// microphone when the secretary presses Protokol's (window.__protokolSetMic),
// the camera off when joining, and the captions on, in Ukrainian (see
// «Captions on» below). And so that Meet keeps
// lighting the tiles while its window is minimised, the page is always told
// it is visible and its animation frames keep coming (see «Minimised window»
// below).
//
// Parts adapted from Vexa (https://github.com/Vexa-ai/vexa,
// core/meetings/modules/gmeet-capture: gmeet-capture.ts, gmeet-speakers.ts,
// pcm-capture.ts), Apache License 2.0, Copyright 2025-2026 Vexa.ai Inc. and
// the Vexa contributors. Changed: one shared AudioContext with an
// AnalyserNode per stream (levels only, no audio is copied), tile classes
// recorded for analysis, reports to the Protokol app instead of a WebSocket.
(() => {
  if (window.__protokolMeetProbe) return;
  if (location.hostname !== 'meet.google.com') return;
  // The page only: Meet's own frames inside it would report a second, different page
  if (window.top !== window) return;
  window.__protokolMeetProbe = true;

  const VERSION = 12;
  // Each stream's own sound is copied for the app while a recording runs
  // (see slotTap), unless the app turned that off. TEST BUILDS ONLY
  // (window.__protokolSlots): copied all the time, with each stream's CSRCs
  const SLOTS = window.__protokolSlots === true;
  const STREAMS = SLOTS || window.__protokolStreams !== false;
  // What the app said in reply to the last report: a recording runs (and is not paused)
  let recording = false;
  // With no recording, and while one is paused, the page keeps only this
  // much of the streams' sound, so a recording has it from a little before
  // the page learned that it started or went on
  const PRE_ROLL_MS = 6000;

  // TEST BUILDS ONLY: Meet sends the loudest people over a few fixed audio
  // streams and names the true source of each packet in its CSRC. The page's
  // peer connections are kept so that each stream's receiver can say, every
  // 100 ms, which sources are speaking on it now (see speakingSources)
  const peerConnections = [];
  if (SLOTS && typeof window.RTCPeerConnection === 'function') {
    try {
      const Orig = window.RTCPeerConnection;
      const Wrapped = function (...args) { const pc = new Orig(...args); peerConnections.push(pc); return pc; };
      Wrapped.prototype = Orig.prototype;
      Object.setPrototypeOf(Wrapped, Orig);
      window.RTCPeerConnection = Wrapped;
      if (window.webkitRTCPeerConnection) window.webkitRTCPeerConnection = Wrapped;
    } catch (e) {
      note(`csrc: ${e && e.message}`);
    }
  }
  // A CSRC entry stays listed for seconds after its source stops: only one
  // heard within SOURCE_FRESH_MS and with a level above 0 is speaking now
  const SOURCE_FRESH_MS = 150;
  function receiverOf(s) {
    if (s.rx && s.rx.track && s.rx.track.readyState !== 'ended' && s.rx.track.id === s.trackId) return s.rx;
    s.rx = null;
    for (const pc of peerConnections) {
      if (pc.connectionState === 'closed') continue;
      for (const r of pc.getReceivers()) {
        if (r.track && r.track.id === s.trackId) { s.rx = r; return r; }
      }
    }
    return null;
  }
  function speakingSources(s) {
    const rx = s.trackId ? receiverOf(s) : null;
    if (!rx || typeof rx.getContributingSources !== 'function') return [];
    const now = Date.now();
    try {
      return rx.getContributingSources().filter((x) => now - x.timestamp <= SOURCE_FRESH_MS && x.audioLevel > 0).map((x) => x.source);
    } catch (e) {
      return [];
    }
  }
  const LEVEL_MS = 100;
  const TILE_MS = 200;
  const REPORT_MS = 2000;
  const RESCAN_MS = 2000;
  const KNOWN_SPEAKING = ['Oaajhc', 'HX2H7', 'wEsLMd', 'OgVli', 'speaking', 'active-speaker', 'speaker-active', 'speaking-indicator'];
  const JUNK_NAME = /^Google Participant \(|spaces\/|devices\//;
  const JUNK_PHRASES = ['let participants', 'send messages', 'turn on captions'];

  const started = Date.now();
  const errors = [];
  const note = (msg) => {
    if (errors.length < 40) errors.push(`${Math.round((Date.now() - started) / 1000)}s ${msg}`);
  };

  // ── Minimised window ─────────────────────────────────────────────────────
  // A minimised window hides the page: WebKit stops its animation frames and
  // Meet stops updating the tiles, keeping the last speaker lit to the end of
  // the call. So the page is always told it is visible, and while it really
  // is hidden its animation frames come from a timer (Protokol turns WebKit's
  // timer slow-down for hidden pages off, meet/mod.rs). The report keeps the
  // real state, and how much Meet changed on the page, so Protokol can tell
  // live tiles from frozen ones.
  const realVisibility = Object.getOwnPropertyDescriptor(Document.prototype, 'visibilityState');
  const reallyHidden = () => {
    try { return !!realVisibility && realVisibility.get.call(document) === 'hidden'; } catch (_) { return false; }
  };
  try {
    Object.defineProperty(Document.prototype, 'visibilityState', { configurable: true, enumerable: true, get() { return 'visible'; } });
    Object.defineProperty(Document.prototype, 'hidden', { configurable: true, enumerable: true, get() { return false; } });
    const swallow = (e) => { e.stopImmediatePropagation(); };
    window.addEventListener('visibilitychange', swallow, true);
  } catch (e) {
    note(`visibility: ${e && e.message}`);
  }
  const HIDDEN_FRAME_MS = 100;
  const LATE_FRAME_MS = 1000;
  try {
    const nativeFrame = window.requestAnimationFrame.bind(window);
    const nativeCancel = window.cancelAnimationFrame.bind(window);
    const waiting = new Map(); // id -> { frame, timer }
    let frameId = 0;
    // A frame asked for while the page is hidden comes after 100 ms; one asked
    // for before it was hidden, after at most 1 s instead of never
    window.requestAnimationFrame = function requestAnimationFrame(cb) {
      const id = ++frameId;
      const run = (t) => {
        const w = waiting.get(id);
        if (!w) return;
        waiting.delete(id);
        nativeCancel(w.frame);
        clearTimeout(w.timer);
        cb(t);
      };
      waiting.set(id, { frame: nativeFrame(run), timer: setTimeout(() => run(performance.now()), reallyHidden() ? HIDDEN_FRAME_MS : LATE_FRAME_MS) });
      return id;
    };
    window.cancelAnimationFrame = function cancelAnimationFrame(id) {
      const w = waiting.get(id);
      if (!w) return;
      waiting.delete(id);
      nativeCancel(w.frame);
      clearTimeout(w.timer);
    };
  } catch (e) {
    note(`frames: ${e && e.message}`);
  }
  // Class and style changes Meet made on the page since the last report
  let pageChanges = 0;
  const watchChanges = () => {
    try {
      new MutationObserver((records) => { pageChanges += records.length; })
        .observe(document.documentElement, { subtree: true, attributes: true, attributeFilter: ['class', 'style'] });
    } catch (e) {
      note(`changes: ${e && e.message}`);
    }
  };
  if (document.documentElement) watchChanges();
  else document.addEventListener('DOMContentLoaded', watchChanges, { once: true });

  // ── Audio: one AudioContext, an analyser per stream ──────────────────────
  let ctx = null;
  const streams = new Map(); // MediaStream id -> state
  let nextIndex = 0;

  function audioContext() {
    if (ctx) return ctx;
    const AC = window.AudioContext || window.webkitAudioContext;
    if (!AC) {
      note('no AudioContext');
      return null;
    }
    ctx = new AC();
    ctx.resume().catch(() => {});
    return ctx;
  }
  // Meet only plays sound after a click, and WebKit keeps a context created
  // before that suspended: resume on the first clicks and keys
  for (const ev of ['pointerdown', 'keydown']) {
    window.addEventListener(ev, () => { if (ctx && ctx.state !== 'running') ctx.resume().catch(() => {}); }, true);
  }

  function mediaElements() {
    return Array.from(document.querySelectorAll('audio, video')).filter((el) => {
      const s = el.srcObject;
      return s && typeof s.getAudioTracks === 'function' && s.getAudioTracks().length > 0;
    });
  }

  function connect(el) {
    const stream = el.srcObject;
    if (!stream || streams.has(stream.id)) return;
    const c = audioContext();
    if (!c) return;
    try {
      const source = c.createMediaStreamSource(stream);
      const analyser = c.createAnalyser();
      analyser.fftSize = 2048;
      const mute = c.createGain();
      mute.gain.value = 0;
      source.connect(analyser);
      analyser.connect(mute);
      mute.connect(c.destination);
      const track = stream.getAudioTracks()[0];
      const state = {
        index: nextIndex++,
        trackId: track ? track.id : null,
        src: [],
        id: stream.id.slice(0, 8),
        tag: el.tagName.toLowerCase(),
        analyser,
        source,
        buf: new Float32Array(analyser.fftSize),
        levels: [],
        el,
        since: Date.now(),
        ended: false,
      };
      if (track) track.addEventListener('ended', () => { state.ended = true; });
      streams.set(stream.id, state);
      if (STREAMS) slotTap(c, source, mute, state);
    } catch (e) {
      note(`connect ${el.tagName}: ${e && e.message}`);
    }
  }

  // ── Each stream's own sound ──────────────────────────────────────────────
  // 16 kHz 16-bit mono blocks with the wall clock of each block's start. While
  // a recording runs they are sent with the report and the app keeps of them
  // what sounded while it recorded (people talking at once are then heard
  // apart); otherwise, and while the recording is paused, only the last
  // PRE_ROLL_MS stay in the page and nothing is sent
  function slotTap(c, source, sink, state) {
    try {
      const proc = c.createScriptProcessor(4096, 1, 1);
      const step = c.sampleRate / 16000;
      state.pcm = []; state.bt = []; state.bn = [];
      proc.onaudioprocess = (e) => {
        const x = e.inputBuffer.getChannelData(0);
        const n = Math.floor(x.length / step);
        const out = new Int16Array(n);
        for (let i = 0; i < n; i++) {
          let sum = 0; const a = Math.floor(i * step), b = Math.floor((i + 1) * step);
          for (let k = a; k < b; k++) sum += x[k];
          const v = sum / (b - a);
          out[i] = Math.max(-32768, Math.min(32767, Math.round(v * 32767)));
        }
        state.pcm.push(out);
        state.bt.push(Date.now() - Math.round(1000 * x.length / c.sampleRate));
        state.bn.push(n);
      };
      source.connect(proc);
      proc.connect(sink);
      state.proc = proc;
    } catch (e) {
      note(`slot: ${e && e.message}`);
    }
  }
  function slotOut() {
    const out = [];
    for (const s of streams.values()) {
      if (!s.pcm || !s.pcm.length) continue;
      const parts = s.pcm.splice(0, s.pcm.length);
      const total = parts.reduce((a, p) => a + p.length, 0);
      const all = new Int16Array(total);
      let o = 0; for (const p of parts) { all.set(p, o); o += p.length; }
      const bytes = new Uint8Array(all.buffer);
      let bin = '';
      for (let i = 0; i < bytes.length; i += 0x8000) bin += String.fromCharCode.apply(null, bytes.subarray(i, i + 0x8000));
      out.push({ i: s.index, id: s.id, bt: s.bt.splice(0, s.bt.length), bn: s.bn.splice(0, s.bn.length), pcm: btoa(bin) });
    }
    return out;
  }

  function slotTrim() {
    const from = Date.now() - PRE_ROLL_MS;
    for (const s of streams.values()) {
      if (!s.pcm) continue;
      let k = 0;
      while (k < s.bt.length && s.bt[k] < from) k++;
      if (k) { s.pcm.splice(0, k); s.bt.splice(0, k); s.bn.splice(0, k); }
    }
  }

  function rescan() {
    for (const el of mediaElements()) connect(el);
  }

  function levelTick() {
    for (const s of streams.values()) {
      if (SLOTS) s.src.push(s.ended ? [] : speakingSources(s));
      if (s.ended) { s.levels.push(-1); continue; }
      s.analyser.getFloatTimeDomainData(s.buf);
      let sum = 0;
      for (let i = 0; i < s.buf.length; i++) sum += s.buf[i] * s.buf[i];
      s.levels.push(Math.round(Math.sqrt(sum / s.buf.length) * 10000));
    }
    litFrames.push(litNow.slice());
    // The wall clock of each frame: the app lines the call up with its recording by it
    frameTimes.push(Date.now());
  }

  // ── Tiles: names, self, who is lit ───────────────────────────────────────
  const names = [];
  const nameIndex = (n) => {
    let i = names.indexOf(n);
    if (i < 0) { names.push(n); i = names.length - 1; }
    return i;
  };
  let litNow = [];
  const litFrames = [];
  const frameTimes = [];
  const classHits = {};
  let selfName = null;
  let tileCount = 0;
  const lastClasses = new Map(); // tile id -> Set of classes
  let classChanges = [];

  function tileName(el) {
    const nt = el.querySelector('span.notranslate');
    let t = (nt && nt.textContent || '').trim();
    if (!t) {
      const labeled = el.querySelector('[data-self-name]');
      t = (labeled && labeled.getAttribute('data-self-name') || '').trim();
    }
    if (!t || t.length < 2 || t.length > 50) return null;
    if (JUNK_NAME.test(t)) return null;
    const lower = t.toLowerCase();
    if (JUNK_PHRASES.some((p) => lower.includes(p))) return null;
    return t;
  }

  function speaking(el) {
    for (const cls of KNOWN_SPEAKING) {
      if (el.classList.contains(cls) || el.querySelector('.' + CSS.escape(cls))) {
        classHits[cls] = (classHits[cls] || 0) + 1;
        return true;
      }
    }
    return false;
  }

  function tileTick() {
    const marker = document.querySelector('[data-self-name]');
    const selfTile = marker && marker.closest('[data-participant-id]');
    const selfId = selfTile ? selfTile.getAttribute('data-participant-id') : null;
    const lit = [];
    const seen = new Set();
    // Ids on the call's own tiles. Meet's people panel lists everyone in
    // rows, also those still waiting to be admitted, so its rows count only
    // when no tile is found outside such lists.
    const onStage = new Set();
    const tiles = document.querySelectorAll('[data-participant-id]');
    for (const el of tiles) {
      const id = el.getAttribute('data-participant-id') || '';
      if (!id || seen.has(id)) continue;
      // Someone asking to join is shown in a dialog («Admit»): not in the call yet
      if (el.closest('[role="dialog"], [role="alertdialog"]')) continue;
      if (!el.closest('[role="list"], [role="listitem"], [role="listbox"], [role="option"]')) onStage.add(id);
      seen.add(id);
      const name = tileName(el);
      const isSelf = el.hasAttribute('data-self-name') || !!el.querySelector('[data-self-name]') || id === selfId;
      if (isSelf && name) selfName = name;
      if (!isSelf && name && speaking(el)) lit.push(nameIndex(name));
      if (name) nameIndex(name);
    }
    tileCount = onStage.size || seen.size;
    litNow = lit;
  }

  // Classes that come and go on each tile, to find the speaking marker if
  // the known ones do not match in this browser
  function classTick() {
    const now = Date.now();
    for (const el of document.querySelectorAll('[data-participant-id]')) {
      const id = el.getAttribute('data-participant-id') || '';
      if (!id) continue;
      const cur = new Set();
      for (const node of [el, ...el.querySelectorAll('*')]) {
        for (const c of node.classList) cur.add(c);
      }
      const prev = lastClasses.get(id);
      if (prev) {
        const added = [...cur].filter((c) => !prev.has(c));
        const removed = [...prev].filter((c) => !cur.has(c));
        if ((added.length || removed.length) && classChanges.length < 200) {
          const name = tileName(el);
          classChanges.push({ t: now, n: name ? nameIndex(name) : -1, add: added.slice(0, 12), del: removed.slice(0, 12) });
        }
      }
      lastClasses.set(id, cur);
    }
  }


  // ── Meet's own microphone button ─────────────────────────────────────────
  // The button carries data-is-muted on the page before joining and in the
  // call. Found by its stable name, its label in any of several languages or
  // its icon; buttons inside participant tiles are someone else's.
  const MIC_LABEL = /microphone|мікрофон|микрофон|mikrofon|micr[oó]fono/i;
  const MIC_ICONS = ['mic', 'mic_off', 'mic_none'];
  // While Meet changes pages (joining, leaving) its button is replaced and
  // can show «on» for a moment before it settles on the person's choice, so
  // «on» from a page that just changed counts once it has held a while, and
  // a button that is gone counts once it stays gone. «Off» counts at once:
  // a moment too much silence is better than a moment of a muted room.
  const MIC_SETTLE_MS = 3000;
  const MIC_ON_HOLD_MS = 700;
  const MIC_GONE_HOLD_MS = 1500;
  let micSent; // undefined: nothing sent yet
  let micSeen; // the state last read
  let micSeenAt = 0; // since when
  let micPage = ''; // the button and the page it was read on
  let micPageAt = 0; // since when
  let micEl = null;

  function isMicButton(el) {
    if (el.closest('[data-participant-id]')) return false;
    if (el.getAttribute('jsname') === 'hw0c9') return true;
    const label = el.getAttribute('aria-label') || el.getAttribute('data-tooltip') || '';
    if (MIC_LABEL.test(label)) return true;
    const icon = el.querySelector('i, .google-symbols, .google-material-icons');
    return !!icon && MIC_ICONS.includes((icon.textContent || '').trim());
  }

  /** Meet's microphone button and its state: true off, false on, null no button */
  function micState() {
    for (const el of document.querySelectorAll('[data-is-muted]')) {
      if (!isMicButton(el)) continue;
      const v = el.getAttribute('data-is-muted');
      if (v === 'true') return { el, muted: true };
      if (v === 'false') return { el, muted: false };
    }
    return { el: null, muted: null };
  }

  function micMuted() {
    return micState().muted;
  }

  // Protokol's own microphone button was pressed: Meet's follows. A click on
  // Meet's button, as a person would make; nothing else on the page is touched.
  window.__protokolSetMic = (muted) => {
    for (const el of document.querySelectorAll('[data-is-muted]')) {
      if (!isMicButton(el)) continue;
      const now = el.getAttribute('data-is-muted') === 'true';
      if (now !== muted) el.click();
      return true;
    }
    return false;
  };

  // ── Meet's own camera button ─────────────────────────────────────────────
  // The camera stays off in Protokol's window: «Створити» joins at once, and
  // the secretary is there to record, not to be seen. Each time Meet shows
  // its camera on (the page before joining, the call, «Rejoin»), it is turned
  // off, unless the person turned it on for this meeting: in Meet (a click on
  // the camera button, its keyboard shortcut) or in Protokol's toolbar. The
  // choice is kept for the meeting until the person turns the camera off.
  const CAM_LABEL = /camera|камер|kamera|c[aá]mara/i;
  const CAM_ICONS = ['videocam', 'videocam_off'];
  const CAM_WAIT_MS = 1500; // after a press, before pressing again
  const CAM_TRIES = 5; // presses on one button, so a stuck button is left alone
  let camEl = null;
  let camTries = 0;
  let camPressedAt = 0;
  let camWantedHere = false; // the choice where the address has no meeting code

  function isCamButton(el) {
    if (el.closest('[data-participant-id]') || isMicButton(el)) return false;
    const label = el.getAttribute('aria-label') || el.getAttribute('data-tooltip') || '';
    if (CAM_LABEL.test(label)) return true;
    const icon = el.querySelector('i, .google-symbols, .google-material-icons');
    return !!icon && CAM_ICONS.includes((icon.textContent || '').trim());
  }

  /** Meet's camera button and its state: true off, false on, null no button */
  function camState() {
    for (const el of document.querySelectorAll('[data-is-muted]')) {
      if (!isCamButton(el)) continue;
      const v = el.getAttribute('data-is-muted');
      if (v === 'true') return { el, off: true };
      if (v === 'false') return { el, off: false };
    }
    return { el: null, off: null };
  }

  function camOff() {
    return camState().off;
  }

  function meetingCode() {
    return (location.pathname.match(/^\/(?:meet\/)?([a-z]{3}-[a-z]{4}-[a-z]{3})/) || [])[1] || null;
  }

  function camWanted() {
    const code = meetingCode();
    if (!code) return camWantedHere;
    try { return sessionStorage.getItem(`protokolCameraOn:${code}`) === '1'; } catch (_) { return camWantedHere; }
  }

  function wantCam(on) {
    camWantedHere = on;
    const code = meetingCode();
    if (!code) return;
    try {
      if (on) sessionStorage.setItem(`protokolCameraOn:${code}`, '1');
      else sessionStorage.removeItem(`protokolCameraOn:${code}`);
    } catch (_) { /* storage off */ }
  }

  function setCam(off) {
    const { el, off: now } = camState();
    if (!el) return false;
    if (now !== off) {
      camPressedAt = Date.now();
      el.click();
    }
    return true;
  }

  // The person reaches for Meet's camera: read before Meet acts, so the
  // state now is the one being changed. Only real input counts; Protokol's
  // own presses (el.click()) are not trusted events.
  function camIntent() {
    const off = camOff();
    if (off === null) return;
    wantCam(off);
    if (off) camTries = 0;
  }

  function camTarget(t) {
    const b = t && t.closest && t.closest('button, [role="button"], [data-is-muted]');
    if (!b || b.closest('[data-participant-id]') || isMicButton(b)) return false;
    if (isCamButton(b)) return true;
    // Meet's «Turn on camera» link over the preview has no state of its own
    return CAM_LABEL.test(b.getAttribute('aria-label') || b.textContent || '');
  }

  try {
    document.addEventListener('pointerdown', (e) => {
      if (e.isTrusted && camTarget(e.target)) camIntent();
    }, true);
    document.addEventListener('keydown', (e) => {
      if (!e.isTrusted) return;
      const shortcut = (e.metaKey || e.ctrlKey) && !e.altKey && e.code === 'KeyE';
      const press = (e.key === 'Enter' || e.key === ' ') && camTarget(e.target);
      if (shortcut || press) camIntent();
    }, true);
  } catch (e) {
    note(`camera listeners: ${e && e.message}`);
  }

  // Protokol's camera button was pressed: Meet's follows, the choice is kept
  // for the meeting, and the toolbar learns the new state with the next
  // report, sent sooner
  window.__protokolSetCam = (off) => {
    wantCam(!off);
    camTries = 0;
    const done = setCam(off);
    setTimeout(() => { try { report(); } catch (_) { /* page changing */ } }, 400);
    return done;
  };

  function camTick() {
    const { el, off } = camState();
    if (!el) { camEl = null; return; }
    if (el !== camEl) { camEl = el; camTries = 0; }
    if (off !== false || camWanted()) return;
    const now = Date.now();
    if (now - camPressedAt < CAM_WAIT_MS || camTries >= CAM_TRIES) return;
    camTries += 1;
    camPressedAt = now;
    el.click();
    note('camera turned off');
  }

  function micTick() {
    const { el, muted } = micState();
    const now = Date.now();
    const page = `${location.pathname}|${tileCount > 0}`;
    if (el !== micEl || page !== micPage) {
      micEl = el;
      micPage = page;
      micPageAt = now;
    }
    if (muted !== micSeen) {
      micSeen = muted;
      micSeenAt = now;
    }
    if (muted === micSent) return;
    let hold = 0;
    if (muted === null) hold = MIC_GONE_HOLD_MS;
    else if (muted === false && now - micPageAt < MIC_SETTLE_MS) hold = MIC_ON_HOLD_MS;
    if (now - micSeenAt < hold) return;
    micSent = muted;
    const ipc = window.__TAURI_INTERNALS__;
    if (!ipc || typeof ipc.invoke !== 'function') return;
    ipc.invoke('plugin:meetprobe|mic', { muted })
      .then(() => {})
      .catch((e) => { micSent = undefined; note(`mic: ${String(e && (e.message || e)).slice(0, 120)}`); });
  }

  // At once when Meet flips the button, and every 250 ms in case the button
  // itself was replaced
  try {
    new MutationObserver(() => { try { micTick(); } catch (_) { /* page changing */ } })
      .observe(document, { subtree: true, attributes: true, attributeFilter: ['data-is-muted'] });
  } catch (e) {
    note(`mic observer: ${e && e.message}`);
  }

  // ── Meet's own captions ──────────────────────────────────────────────────
  // When captions are on in Meet (its CC button, or the «c» key), Meet shows
  // what each person says, by name. They are Google's recognition, made
  // anyway for anyone in the call who turns captions on; Protokol reads
  // them off the page, as a second text to check its own against, and sends
  // them nowhere but to Protokol. It turns them on itself (below).
  //
  // The captions sit in a region (role="region", tabindex="0"); each block
  // holds the speaker's name and, right after it, the text, which Meet keeps
  // rewriting while the person speaks and corrects a little afterwards. Each
  // text element gets an id; every version of its text is kept with the time
  // it was first and last seen, so Protokol can use the last one. A text that
  // suddenly gets much shorter (Meet starts over after a long speech) gets a
  // new id. Adapted from TranscripTonic (https://github.com/vivek-nexus/transcriptonic,
  // extension/content-scripts/google-meet), MIT License, Copyright (c) 2024 Vivek G (THIRD_PARTY_NOTICES.md).
  const CC_ICONS = ['closed_caption', 'closed_caption_off', 'translate_cc', 'subtitles', 'subtitles_off'];
  const CAPTION_MS = 300;
  const CAPTION_MAX_TEXT = 4000;
  const CAPTION_MAX_PER_REPORT = 300;
  const captionIds = new WeakMap(); // text element -> { id, text, first }
  let captionNext = 0;
  let captionRegion = null;
  let captionObserver = null;
  const captionDirty = new Set(); // text elements changed since the last pass
  let captionOut = []; // { id, who, text, t0, t1 } since the last report

  function captionsButton() {
    for (const icon of document.querySelectorAll('.google-symbols, .google-material-icons, i')) {
      if (!CC_ICONS.includes((icon.textContent || '').trim())) continue;
      const b = icon.closest('button');
      if (b && !b.closest('[data-participant-id]')) return b;
    }
    return null;
  }

  /** Meet's captions button: true on, false off, null not on the page (or
   *  neither its state nor its icon tells) */
  function captionsOn() {
    const b = captionsButton();
    if (!b) return null;
    const v = b.getAttribute('aria-pressed');
    if (v === 'true') return true;
    if (v === 'false') return false;
    // Without a pressed state, the icon: «closed_caption_off» while they are off
    for (const icon of b.querySelectorAll('.google-symbols, .google-material-icons, i')) {
      const name = (icon.textContent || '').trim();
      if (name === 'closed_caption_off' || name === 'subtitles_off') return false;
      if (name === 'closed_caption' || name === 'subtitles') return true;
    }
    return null;
  }

  function captionText(node) {
    const el = node && (node.nodeType === Node.TEXT_NODE ? node.parentElement : node);
    if (!el || !captionRegion || !captionRegion.contains(el) || el === captionRegion) return;
    captionDirty.add(el);
  }

  function captionPass() {
    if (!captionDirty.size) return;
    const now = Date.now();
    for (const el of captionDirty) {
      if (!el.isConnected) continue;
      // The name is the element right before the text, inside the same block
      const nameEl = el.previousElementSibling;
      const who = (nameEl && nameEl.textContent || '').trim();
      const text = (el.textContent || '').trim();
      if (!who || who.length > 60 || !text || text === who) continue;
      let c = captionIds.get(el);
      if (c && text.length < c.text.length - 250) c = null;
      if (!c) {
        c = { id: captionNext++, text: '', first: now };
        captionIds.set(el, c);
      }
      if (c.text === text) continue;
      c.text = text;
      if (captionOut.length < CAPTION_MAX_PER_REPORT) {
        captionOut.push({ id: c.id, who: who.slice(0, 60), text: text.slice(0, CAPTION_MAX_TEXT), t0: c.first, t1: now });
      }
    }
    captionDirty.clear();
  }

  function captionWatch() {
    const region = document.querySelector('div[role="region"][tabindex="0"]');
    if (region === captionRegion) return;
    if (captionObserver) captionObserver.disconnect();
    captionRegion = region;
    captionObserver = null;
    if (!region) return;
    try {
      captionObserver = new MutationObserver((records) => {
        for (const r of records) {
          if (r.type === 'characterData') captionText(r.target);
          else for (const n of r.addedNodes) {
            if (n.nodeType === Node.TEXT_NODE) captionText(n);
            else if (n.nodeType === Node.ELEMENT_NODE) {
              const w = document.createTreeWalker(n, NodeFilter.SHOW_TEXT);
              for (let t = w.nextNode(); t; t = w.nextNode()) captionText(t);
            }
          }
        }
      });
      captionObserver.observe(region, { subtree: true, childList: true, characterData: true });
    } catch (e) {
      note(`captions: ${e && e.message}`);
    }
  }

  // ── Captions on ──────────────────────────────────────────────────────────
  // So that every call in this window has the second text, Protokol turns
  // Meet's captions on by itself once in the call and sets their language to
  // Ukrainian; they stay on the screen as Meet shows them. Captions in Meet
  // are only on this screen; no one else in the call is told. If the person
  // reaches for captions themselves (Meet's captions button or the «c» key),
  // they are left to the person for the rest of the meeting: Protokol does
  // not turn them back on.
  const CC_TICK_MS = 1000;
  const CC_WAIT_MS = 2500;
  const CC_TRIES = 6;
  const CC_LANG_TRIES = 4;
  const CC_TURN_ON = /turn on captions|увімкнути субтитри|включить субтитры/i;
  const CC_SWITCH = /^(captions|субтитри|субтитры)$/i;
  const CC_LANG_NAME = /english|ukrainian|russian|polish|german|french|spanish|italian|portuguese|англійськ|українськ|російськ|польськ|німецьк|французьк|іспанськ|английск|украинск|русск/i;
  const CC_NOT_LANG = /translat|переклад|перевод/i;
  const CC_CHOICES = '[role="menuitem"], [role="menuitemradio"], [role="menuitemcheckbox"], [role="option"], [role="switch"], [role="radio"], [role="combobox"], button, [role="button"]';
  const CC_LIST = '[role="listbox"], [role="menu"], [role="radiogroup"]';
  const ICONS = '.google-symbols, .google-material-icons, .material-icons-extended, .material-symbols-outlined, i';
  let ccState = 'not in call';
  let ccTries = 0;
  let ccPressedAt = 0;
  let ccLangTries = 0;
  let ccLangAt = 0;
  let ccLang = null;
  let ccPage = '';
  let ccChoiceHere = null;

  /** The language Protokol sets: Ukrainian; another one in test runs only */
  function ccTarget() {
    const t = window.__protokolCaptionsLang;
    if (t && typeof t.name === 'string' && typeof t.code === 'string') {
      return { name: new RegExp(t.name.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'), 'i'), code: t.code.toLowerCase() };
    }
    return { name: /ukrainian|українськ|украинск/i, code: 'uk' };
  }

  /** 'person' once the person has reached for captions in this meeting */
  function ccChoice() {
    const code = meetingCode();
    if (!code) return ccChoiceHere;
    try { return sessionStorage.getItem(`protokolCaptions:${code}`) || null; } catch (_) { return ccChoiceHere; }
  }

  function setCcChoice(v) {
    ccChoiceHere = v;
    const code = meetingCode();
    if (!code) return;
    try { sessionStorage.setItem(`protokolCaptions:${code}`, v); } catch (_) { /* storage off */ }
  }

  /** An element's own words, without the names of its icons */
  function plainText(el) {
    let s = '';
    const w = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
    for (let t = w.nextNode(); t; t = w.nextNode()) {
      const p = t.parentElement;
      if (p && p.closest(ICONS)) continue;
      s += `${t.textContent} `;
    }
    return s.replace(/\s+/g, ' ').trim();
  }

  /** The shortest control on the page whose words pass the test (not inside `not`) */
  function ccFind(test, not) {
    let best = null;
    let bestLen = Infinity;
    for (const el of document.querySelectorAll(CC_CHOICES)) {
      if (not && (el === not || el.contains(not) || not.contains(el))) continue;
      if (el.closest('[data-participant-id]') || !el.getClientRects().length) continue;
      const t = plainText(el);
      if (!t || t.length > 80 || !test(t, el)) continue;
      if (t.length < bestLen) { best = el; bestLen = t.length; }
    }
    return best;
  }

  /** Meet's chip with the captions' language, which opens the list of languages */
  function ccChip() {
    return ccFind((t, el) => t.length <= 40 && CC_LANG_NAME.test(t) && !CC_NOT_LANG.test(t)
      && !el.closest(CC_LIST)
      && (el.hasAttribute('aria-haspopup') || el.hasAttribute('aria-expanded')
        || /arrow_drop_down|expand_more|keyboard_arrow_down/.test(el.textContent || '')), null);
  }

  /** The wanted language in an open list of languages */
  function ccOption(target) {
    for (const el of document.querySelectorAll('[data-value]')) {
      const v = (el.getAttribute('data-value') || '').toLowerCase();
      if ((v === target.code || v.startsWith(`${target.code}-`) || v.startsWith(`${target.code}_`))
        && (el.matches('[role="option"], [role="menuitemradio"], [role="radio"]') || el.closest(CC_LIST))) return el;
    }
    return ccFind((t, el) => t.length <= 60 && target.name.test(t) && !CC_NOT_LANG.test(t)
      && (el.matches('[role="option"], [role="menuitemradio"], [role="menuitem"], [role="radio"]') || !!el.closest(CC_LIST)), null);
  }

  // Meet's own shortcut, as a key press on the page
  function ccKey() {
    for (const type of ['keydown', 'keyup']) {
      const ev = new KeyboardEvent(type, { key: 'c', code: 'KeyC', bubbles: true, cancelable: true, composed: true });
      try {
        Object.defineProperty(ev, 'keyCode', { get: () => 67 });
        Object.defineProperty(ev, 'which', { get: () => 67 });
      } catch (_) { /* read-only */ }
      document.body.dispatchEvent(ev);
    }
  }

  // Test runs only: what Meet shows around its captions, to find their controls
  function ccDump(why) {
    if (!window.__protokolCaptionsDump) return;
    const out = [];
    const b = captionsButton();
    if (b) out.push(`button «${b.getAttribute('aria-label') || ''}» pressed=${b.getAttribute('aria-pressed')} popup=${b.getAttribute('aria-haspopup')}`);
    for (const el of document.querySelectorAll(CC_CHOICES)) {
      if (out.length >= 14) break;
      if (el.closest('[data-participant-id]') || !el.getClientRects().length) continue;
      const near = captionRegion && captionRegion.parentElement && captionRegion.parentElement.contains(el);
      if (!near && !el.closest(`${CC_LIST}, [role="dialog"]`)) continue;
      const role = el.getAttribute('role') || el.tagName.toLowerCase();
      out.push(`${role} «${plainText(el).slice(0, 40)}» value=${el.getAttribute('data-value') || ''} checked=${el.getAttribute('aria-checked') || ''}`);
    }
    note(`captions dump ${why}: ${out.join(' | ').slice(0, 900)}`);
  }

  // The person reaches for captions: from now on they are the person's
  function ccIntent() {
    if (ccChoice() !== 'person') setCcChoice('person');
  }

  const ccOnButton = (t) => {
    const b = captionsButton();
    return !!(b && t && t.nodeType === Node.ELEMENT_NODE && (t === b || b.contains(t)));
  };
  const editable = (t) => !!(t && t.closest && t.closest('input, textarea, [contenteditable=""], [contenteditable="true"]'));

  try {
    document.addEventListener('pointerdown', (e) => {
      if (e.isTrusted && ccOnButton(e.target)) ccIntent();
    }, true);
    document.addEventListener('keydown', (e) => {
      if (!e.isTrusted || e.metaKey || e.ctrlKey || e.altKey) return;
      const shortcut = e.code === 'KeyC' && !editable(e.target);
      const press = (e.key === 'Enter' || e.key === ' ') && ccOnButton(e.target);
      if (shortcut || press) ccIntent();
    }, true);
  } catch (e) {
    note(`captions listeners: ${e && e.message}`);
  }

  function ccLangTick() {
    const chip = ccChip();
    const text = chip ? plainText(chip) : null;
    ccLang = text ? text.slice(0, 40) : null;
    const target = ccTarget();
    if (!chip || target.name.test(text)) return;
    const now = Date.now();
    if (now - ccLangAt < 700) return;
    const option = ccOption(target);
    if (option) {
      ccLangAt = now;
      option.click();
      note(`captions language chosen: ${plainText(option).slice(0, 40)}`);
      return;
    }
    if (ccLangTries >= CC_LANG_TRIES) {
      // A list left open by the last try is closed again
      if (chip.getAttribute('aria-expanded') === 'true') { ccLangAt = now; chip.click(); }
      return;
    }
    if (now - ccLangAt < CC_WAIT_MS) return;
    ccLangTries += 1;
    ccLangAt = now;
    chip.click();
    note(`captions language list opened (${ccLangTries}), was «${ccLang}»`);
    setTimeout(() => { try { ccDump('language list'); } catch (_) { /* page changing */ } }, 600);
  }

  function ccTick() {
    const page = `${meetingCode() || location.pathname}|${tileCount > 0}`;
    if (page !== ccPage) {
      ccPage = page;
      ccTries = 0;
      ccLangTries = 0;
    }
    if (!(tileCount > 0)) { ccState = 'not in call'; return; }
    if (ccChoice() === 'person') {
      ccState = 'person';
      return;
    }
    const on = captionsOn();
    if (on === true) {
      if (!captionRegion || !captionRegion.isConnected) captionWatch();
      ccLangTick();
      ccState = 'on';
      return;
    }
    const b = captionsButton();
    if (!b) { ccState = 'no button'; return; }
    // Nothing is pressed while it is unclear whether they are on: a press
    // could turn them off
    if (on !== false) { ccState = 'state unknown'; return; }
    const now = Date.now();
    // A menu opened by the last press: its «Turn on captions»
    if (ccPressedAt && now - ccPressedAt < CC_WAIT_MS * 2) {
      const item = ccFind((t, el) => CC_TURN_ON.test(t)
        || (CC_SWITCH.test(t) && (el.getAttribute('aria-checked') === 'false' || el.getAttribute('aria-pressed') === 'false')), b);
      if (item) {
        ccPressedAt = now;
        item.click();
        note(`captions: «${plainText(item).slice(0, 40)}» pressed`);
        return;
      }
    }
    if (ccTries >= CC_TRIES) { ccState = 'not turned on'; return; }
    ccState = 'turning on';
    if (now - ccPressedAt < CC_WAIT_MS) return;
    ccTries += 1;
    ccPressedAt = now;
    // The shortcut first (nothing opens on the screen), then the button
    if (ccTries % 2 === 1) {
      ccKey();
      note(`captions: key c (${ccTries})`);
    } else {
      b.click();
      note(`captions: button pressed (${ccTries})`);
      setTimeout(() => { try { ccDump('after the button'); } catch (_) { /* page changing */ } }, 600);
    }
  }

  // ── Report ───────────────────────────────────────────────────────────────

  function structure() {
    return mediaElements().slice(0, 24).map((el) => {
      const s = el.srcObject;
      const tile = el.closest('[data-participant-id]');
      const state = streams.get(s.id);
      return {
        tag: el.tagName.toLowerCase(),
        paused: el.paused,
        muted: el.muted,
        stream: state ? state.index : null,
        tracks: s.getAudioTracks().length,
        inTile: !!tile,
        tileName: tile ? tileName(tile) : null,
      };
    });
  }

  function report() {
    try { captionPass(); } catch (e) { note(`caption: ${e && e.message}`); }
    const frames = litFrames.splice(0, litFrames.length);
    const ft = frameTimes.splice(0, frameTimes.length);
    const out = [];
    for (const s of streams.values()) {
      const levels = s.levels.splice(0, s.levels.length);
      const entry = { i: s.index, id: s.id, tag: s.tag, since: s.since, ended: s.ended, levels };
      // TEST BUILDS ONLY: the CSRCs speaking on this stream, one array per level frame
      if (SLOTS) entry.src = s.src.splice(0, s.src.length);
      out.push(entry);
    }
    const snap = {
      v: VERSION,
      t: Date.now(),
      started,
      stepMs: LEVEL_MS,
      frames: frames.length,
      // meet.google.com/abc-defg-hij, or /meet/abc-defg-hij as Meet also writes it
      meeting: meetingCode() || location.pathname.slice(0, 40),
      // Meet's own start page (/, /landing or /home): Protokol shows its start screen instead
      landing: /^\/((landing|home)\/?)?$/.test(location.pathname),
      // The real state: the page itself is always told «visible»
      visible: reallyHidden() ? 'hidden' : 'visible',
      changes: pageChanges,
      ctx: ctx ? { state: ctx.state, rate: ctx.sampleRate } : null,
      media: structure(),
      streams: out,
      tiles: tileCount,
      // Participant tiles exist only in the call, not in the lobby before joining
      inCall: tileCount > 0,
      participants: tileCount,
      // Meet's microphone button: true off, false on, null not on the page
      mic: micMuted(),
      // Meet's camera button: true off, false on, null not on the page
      cam: camOff(),
      self: selfName,
      names,
      // Meet's captions button: true on, false off, null not on the page
      cc: captionsOn(),
      // Caption texts that appeared or changed since the last report
      captions: captionOut.splice(0, captionOut.length),
      // What Protokol does with the captions: 'on', 'person' (left to the
      // person), 'turning on', 'not turned on', 'no button', 'state unknown',
      // 'not in call'; and their language as Meet shows it
      ccAuto: ccState,
      ccLang,
      lit: frames,
      ft,
      classHits,
      classChanges,
      errors: errors.splice(0, errors.length),
    };
    if (SLOTS || recording) snap.slots = slotOut();
    else if (STREAMS) slotTrim();
    classChanges = [];
    pageChanges = 0;
    const ipc = window.__TAURI_INTERNALS__;
    if (!ipc || typeof ipc.invoke !== 'function') return;
    ipc.invoke('plugin:meetprobe|push', { report: snap })
      .then((state) => { recording = !!state && state.rec === 'on'; })
      .catch((e) => { note(`push: ${String(e && (e.message || e)).slice(0, 120)}`); });
  }

  setInterval(() => { try { levelTick(); } catch (e) { note(`level: ${e && e.message}`); } }, LEVEL_MS);
  setInterval(() => { try { tileTick(); } catch (e) { note(`tile: ${e && e.message}`); } }, TILE_MS);
  setInterval(() => { try { rescan(); classTick(); captionWatch(); } catch (e) { note(`scan: ${e && e.message}`); } }, RESCAN_MS);
  setInterval(() => { try { captionPass(); } catch (e) { note(`caption: ${e && e.message}`); } }, CAPTION_MS);
  setInterval(() => { try { report(); } catch (e) { note(`report: ${e && e.message}`); } }, REPORT_MS);
  setInterval(() => { try { micTick(); } catch (e) { note(`mic: ${e && e.message}`); } }, 250);
  setInterval(() => { try { camTick(); } catch (e) { note(`camera: ${e && e.message}`); } }, 250);
  setInterval(() => { try { ccTick(); } catch (e) { note(`captions: ${e && e.message}`); } }, CC_TICK_MS);
})();
