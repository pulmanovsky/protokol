// Protokol: who speaks in a Google Meet call.
//
// Runs inside the Meet page opened in Protokol's own window. It only LOOKS:
// which audio streams the page plays, how loud each one is every 100 ms,
// which participant tiles are lit as speaking, the tile names and whether
// Meet's own microphone button is on. Every 2 s it hands a report to Protokol
// on this Mac (plugin:meetprobe|push), and the microphone state also at once
// when it changes (plugin:meetprobe|mic), so the recording takes the
// secretary's microphone only while it is on in Meet. Nothing is sent
// anywhere else, and nothing on the page is changed: what Protokol sees is
// shown in its own toolbar above the page. The one thing Protokol asks of the
// page is to press Meet's microphone button when the secretary presses
// Protokol's (window.__protokolSetMic).
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

  const VERSION = 8;
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
    } catch (e) {
      note(`connect ${el.tagName}: ${e && e.message}`);
    }
  }

  function rescan() {
    for (const el of mediaElements()) connect(el);
  }

  function levelTick() {
    for (const s of streams.values()) {
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
    const frames = litFrames.splice(0, litFrames.length);
    const ft = frameTimes.splice(0, frameTimes.length);
    const out = [];
    for (const s of streams.values()) {
      const levels = s.levels.splice(0, s.levels.length);
      out.push({ i: s.index, id: s.id, tag: s.tag, since: s.since, ended: s.ended, levels });
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
      visible: document.visibilityState,
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
      lit: frames,
      ft,
      classHits,
      classChanges,
      errors: errors.splice(0, errors.length),
    };
    classChanges = [];
    const ipc = window.__TAURI_INTERNALS__;
    if (!ipc || typeof ipc.invoke !== 'function') return;
    ipc.invoke('plugin:meetprobe|push', { report: snap })
      .catch((e) => { note(`push: ${String(e && (e.message || e)).slice(0, 120)}`); });
  }

  setInterval(() => { try { levelTick(); } catch (e) { note(`level: ${e && e.message}`); } }, LEVEL_MS);
  setInterval(() => { try { tileTick(); } catch (e) { note(`tile: ${e && e.message}`); } }, TILE_MS);
  setInterval(() => { try { rescan(); classTick(); } catch (e) { note(`scan: ${e && e.message}`); } }, RESCAN_MS);
  setInterval(() => { try { report(); } catch (e) { note(`report: ${e && e.message}`); } }, REPORT_MS);
  setInterval(() => { try { micTick(); } catch (e) { note(`mic: ${e && e.message}`); } }, 250);
  setInterval(() => { try { camTick(); } catch (e) { note(`camera: ${e && e.message}`); } }, 250);
})();
