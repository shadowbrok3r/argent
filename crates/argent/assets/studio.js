// argent studio — phone-first live studio. No framework, no build step.
'use strict';
(() => {
  const $ = (s, el = document) => el.querySelector(s);
  const $$ = (s, el = document) => [...el.querySelectorAll(s)];
  const clamp = (v, a, b) => Math.min(b, Math.max(a, v));
  const wrap180 = (a) => { let r = ((a + 180) % 360 + 360) % 360 - 180; return r === -180 ? 180 : r; };
  const round = (v, d = 0) => { const k = 10 ** d; return Math.round(v * k) / k; };

  function h(tag, attrs, ...kids) {
    const el = document.createElement(tag);
    for (const [k, v] of Object.entries(attrs || {})) {
      if (v == null || v === false) continue;
      if (k === 'class') el.className = v;
      else if (k === 'style' && typeof v === 'object') Object.assign(el.style, v);
      else if (k.startsWith('on')) el.addEventListener(k.slice(2), v);
      else if (k === 'text') el.textContent = v;
      else el.setAttribute(k, v === true ? '' : v);
    }
    for (const c of kids.flat()) if (c != null && c !== false) el.append(c.nodeType ? c : document.createTextNode(String(c)));
    return el;
  }

  const store = {
    get(k, d) { try { const v = localStorage.getItem('argent.' + k); return v == null ? d : JSON.parse(v); } catch { return d; } },
    set(k, v) { try { localStorage.setItem('argent.' + k, JSON.stringify(v)); } catch { /* private mode */ } },
  };

  const HOUSE = ['sterling', 'gold14k', 'bronze'];
  const HOUSE_NAME = { sterling: 'Sterling', gold14k: 'Gold-filled', bronze: 'Bronze' };
  const APERTURES = [['auto', 'Auto'], ['0', 'Sharp'], ['2.8', 'f/2.8'], ['4', 'f/4'], ['5.6', 'f/5.6'], ['8', 'f/8'], ['11', 'f/11'], ['16', 'f/16'], ['22', 'f/22'], ['32', 'f/32']];

  const S = {
    options: null, params: null, piece: null, live: {}, jobs: { active: [], log_version: 0 },
    gallery: [], galleryVersion: -1, library: null,
    ws: null, retry: 0, rt: 0,
    tab: store.get('tab', 'metal'), collapsed: false,
    editing: new Set(), pending: {}, raf: 0,
    frame: null, header: null, drawnSeq: 0, frameTimes: [],
    sync: [], ui: { stillSpp: store.get('stillSpp', 128), sheetSpp: 128, reelSpp: 32, sheetMetals: null, sheetViews: null, rig: 'turntable', seconds: 6, hook: '', audio: '', lastPreset: 'hero' },
  };

  // ---------- toasts ----------
  let toastTimer = 0;
  function toast(msg, err) {
    const t = $('#toast');
    t.textContent = msg;
    t.className = 'on' + (err ? ' err' : '');
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => (t.className = ''), err ? 4200 : 2200);
  }
  const toastErr = (e) => toast(e && e.message ? e.message : String(e), true);

  // ---------- network ----------
  async function api(method, path, body) {
    const r = await fetch(path, {
      method,
      headers: body ? { 'content-type': 'application/json' } : {},
      body: body ? JSON.stringify(body) : undefined,
    });
    const j = await r.json().catch(() => ({}));
    if (!r.ok) throw new Error(j.error || r.statusText);
    return j;
  }

  function setConn(state) {
    const c = $('#conn');
    c.className = 'conn ' + state;
    c.title = state === 'live' ? 'connected' : 'reconnecting…';
  }

  function connect() {
    clearTimeout(S.rt);
    const url = (location.protocol === 'https:' ? 'wss://' : 'ws://') + location.host + '/api/ws';
    let ws;
    try { ws = new WebSocket(url); } catch { scheduleReconnect(); return; }
    ws.binaryType = 'arraybuffer';
    S.ws = ws;
    setConn('wait');
    ws.onopen = () => {
      S.retry = 0;
      setConn('live');
      if (document.hidden) ws.send(JSON.stringify({ type: 'visibility', visible: false }));
      flush();
    };
    ws.onclose = () => { if (S.ws === ws) { S.ws = null; setConn('wait'); scheduleReconnect(); } };
    ws.onmessage = (ev) => (typeof ev.data === 'string' ? onJSON(JSON.parse(ev.data)) : onFrame(ev.data));
  }
  function scheduleReconnect() {
    clearTimeout(S.rt);
    S.rt = setTimeout(connect, Math.min(5000, 250 * 2 ** S.retry++));
  }
  // A hidden page tells the server it isn't watching, so the renderer can idle.
  document.addEventListener('visibilitychange', () => {
    if (S.ws && S.ws.readyState === 1) S.ws.send(JSON.stringify({ type: 'visibility', visible: !document.hidden }));
    if (!document.hidden && (!S.ws || S.ws.readyState > 1)) { S.retry = 0; connect(); }
  });

  /** Queue a look change; coalesced to one message per animation frame. */
  function setLook(patch, now) {
    for (const [k, v] of Object.entries(patch)) if (typeof v === 'number' && S.params) S.params[k] = v;
    Object.assign(S.pending, patch);
    if (now) flush();
    else if (!S.raf) S.raf = requestAnimationFrame(flush);
  }
  function flush() {
    cancelAnimationFrame(S.raf);
    S.raf = 0;
    const p = S.pending;
    if (!Object.keys(p).length) return;
    S.pending = {};
    if (S.ws && S.ws.readyState === 1) S.ws.send(JSON.stringify({ type: 'look', ...p }));
    else api('POST', '/api/look', p).then(applyParams).catch(toastErr);
  }

  function onJSON(m) {
    switch (m.type) {
      case 'hello':
        S.options = m.options;
        S.params = m.state.params;
        S.live = m.state.live || {};
        buildPanes();
        applyPiece(m.state.piece);
        applyJobs(m.state.jobs);
        applyLive(m.state.live);
        syncAll();
        loadLibrary();
        break;
      case 'params': applyParams(m.params); break;
      case 'piece': applyPiece(m.piece); break;
      case 'jobs': applyJobs(m.jobs); break;
      case 'live': applyLive(m.live); break;
      case 'error': toast(m.error, true); break;
    }
  }

  // ---------- frames ----------
  const canvas = $('#frame');
  const ctx = canvas.getContext('2d', { alpha: false });

  async function decode(blob) {
    if (window.createImageBitmap) return createImageBitmap(blob);
    const url = URL.createObjectURL(blob);
    const img = new Image();
    img.src = url;
    await img.decode();
    img._url = url;
    return img;
  }
  function release(img) {
    if (!img) return;
    if (img.close) img.close();
    if (img._url) URL.revokeObjectURL(img._url);
  }

  async function onFrame(buf) {
    let hdr = null;
    try {
      const n = new DataView(buf).getUint32(0, true);
      hdr = JSON.parse(new TextDecoder().decode(new Uint8Array(buf, 4, n)));
      const img = await decode(new Blob([new Uint8Array(buf, 4 + n)], { type: 'image/jpeg' }));
      if (hdr.seq > S.drawnSeq) {
        S.drawnSeq = hdr.seq;
        release(S.frame);
        S.frame = img;
        S.header = hdr;
        const now = performance.now();
        S.frameTimes.push(now);
        while (S.frameTimes.length && now - S.frameTimes[0] > 2000) S.frameTimes.shift();
        draw();
        updateHud();
      } else release(img);
    } catch (e) {
      console.warn('frame', e);
    } finally {
      if (hdr && S.ws && S.ws.readyState === 1) S.ws.send(JSON.stringify({ type: 'ack', seq: hdr.seq }));
    }
  }

  function formatSize() {
    const f = S.options && S.params && S.options.formats.find((x) => x.key === S.params.format);
    if (f) return [f.w, f.h];
    const m = S.params && /^(\d+)x(\d+)$/.exec(S.params.format || '');
    return m ? [+m[1], +m[2]] : [1080, 1920];
  }
  function frameRect(fw, fh) {
    const cw = canvas.width, ch = canvas.height;
    const s = Math.min(cw / fw, ch / fh);
    const w = Math.round(fw * s), hgt = Math.round(fh * s);
    return { x: Math.round((cw - w) / 2), y: Math.round((ch - hgt) / 2), w, h: hgt };
  }
  function draw() {
    ctx.fillStyle = '#050505';
    ctx.fillRect(0, 0, canvas.width, canvas.height);
    const [fw, fh] = S.frame && S.header ? [S.header.w, S.header.h] : formatSize();
    const r = frameRect(fw, fh);
    if (S.frame) {
      ctx.imageSmoothingEnabled = true;
      ctx.imageSmoothingQuality = 'high';
      ctx.drawImage(S.frame, r.x, r.y, r.w, r.h);
    } else {
      ctx.fillStyle = '#0b0a09';
      ctx.fillRect(r.x, r.y, r.w, r.h);
    }
    ctx.strokeStyle = 'rgba(236,229,216,0.07)';
    ctx.lineWidth = 1;
    ctx.strokeRect(r.x + 0.5, r.y + 0.5, r.w - 1, r.h - 1);
  }
  function sizeCanvas() {
    const r = canvas.getBoundingClientRect();
    const dpr = Math.min(window.devicePixelRatio || 1, 3);
    const w = Math.max(1, Math.round(r.width * dpr)), hgt = Math.max(1, Math.round(r.height * dpr));
    if (canvas.width !== w || canvas.height !== hgt) {
      canvas.width = w;
      canvas.height = hgt;
    }
    draw();
  }
  if (window.ResizeObserver) new ResizeObserver(sizeCanvas).observe(canvas);
  else window.addEventListener('resize', sizeCanvas);

  function updateHud() {
    const hd = S.header;
    const bar = $('#spp');
    if (!hd) { $('#hud').textContent = ''; bar.classList.add('done'); return; }
    const fps = S.frameTimes.length > 1 ? (S.frameTimes.length - 1) / ((S.frameTimes.at(-1) - S.frameTimes[0]) / 1000) : 0;
    $('#hud').textContent = hd.converged ? `${hd.spp} spp · final` : `${hd.spp} / ${hd.max_spp} spp`;
    $('#hud').title = `${hd.w}×${hd.h}${hd.half ? ' (quick look)' : ''} · ${hd.denoiser} · ${fps.toFixed(1)} fps · ${hd.ms} ms after the change`;
    $('#spp i').style.width = `${Math.min(100, (100 * hd.spp) / Math.max(1, hd.max_spp))}%`;
    bar.classList.toggle('done', !!hd.converged);
  }

  // ---------- gestures ----------
  const pointers = new Map();
  let drag = null;
  let lastTap = { t: 0, x: 0, y: 0 };
  const centroid = (pts) => ({ x: pts.reduce((a, p) => a + p.x, 0) / pts.length, y: pts.reduce((a, p) => a + p.y, 0) / pts.length });
  const dist = (a, b) => Math.hypot(a.x - b.x, a.y - b.y);

  function beginGesture() {
    if (!S.params) return;
    const pts = [...pointers.values()];
    drag = { az: S.params.azimuth, el: S.params.elevation, fill: S.params.fill, c: centroid(pts), d: pts.length > 1 ? dist(pts[0], pts[1]) : 0, n: pts.length, moved: false };
    ['azimuth', 'elevation', 'fill'].forEach((k) => S.editing.add(k));
    canvas.classList.add('dragging');
  }
  function moveGesture() {
    const pts = [...pointers.values()];
    if (!drag || pts.length !== drag.n) return beginGesture();
    const c = centroid(pts);
    const dx = c.x - drag.c.x, dy = c.y - drag.c.y;
    if (Math.abs(dx) + Math.abs(dy) > 3) drag.moved = true;
    if (!drag.moved) return;
    if (pts.length === 1) {
      setLook({ azimuth: round(wrap180(drag.az - dx * 0.45), 2), elevation: round(clamp(drag.el + dy * 0.32, -30, 89), 2) });
      gestureNote(`${Math.round(S.params.azimuth)}° · ${Math.round(S.params.elevation)}° up`);
    } else {
      const f = clamp((drag.fill * dist(pts[0], pts[1])) / Math.max(24, drag.d), 0.1, 3);
      setLook({ fill: round(f, 3) });
      gestureNote(`zoom ${f.toFixed(2)}`);
    }
  }
  function endGesture() {
    drag = null;
    canvas.classList.remove('dragging');
    flush();
    hideGestureSoon();
    setTimeout(() => { ['azimuth', 'elevation', 'fill'].forEach((k) => S.editing.delete(k)); syncAll(); }, 450);
  }
  canvas.addEventListener('pointerdown', (e) => {
    canvas.setPointerCapture(e.pointerId);
    pointers.set(e.pointerId, { x: e.clientX, y: e.clientY });
    const now = performance.now();
    if (pointers.size === 1 && now - lastTap.t < 320 && Math.hypot(e.clientX - lastTap.x, e.clientY - lastTap.y) < 30) {
      lastTap.t = 0;
      resetView();
      return;
    }
    lastTap = { t: now, x: e.clientX, y: e.clientY };
    beginGesture();
  });
  canvas.addEventListener('pointermove', (e) => {
    if (!pointers.has(e.pointerId)) return;
    pointers.set(e.pointerId, { x: e.clientX, y: e.clientY });
    moveGesture();
  });
  const up = (e) => {
    if (!pointers.delete(e.pointerId)) return;
    if (pointers.size === 0) endGesture();
    else beginGesture();
  };
  canvas.addEventListener('pointerup', up);
  canvas.addEventListener('pointercancel', up);
  canvas.addEventListener('wheel', (e) => {
    e.preventDefault();
    if (!S.params) return;
    S.editing.add('fill');
    const f = clamp(S.params.fill * Math.exp(-e.deltaY * 0.0012), 0.1, 3);
    setLook({ fill: round(f, 3) });
    gestureNote(`zoom ${f.toFixed(2)}`);
    hideGestureSoon();
    clearTimeout(canvas._wheel);
    canvas._wheel = setTimeout(() => { S.editing.delete('fill'); syncAll(); }, 450);
  }, { passive: false });
  document.addEventListener('gesturestart', (e) => e.preventDefault());

  function resetView() {
    const v = S.params && S.params.view !== 'custom' ? S.params.view : S.ui.lastPreset;
    setLook({ view: v || 'hero' }, true);
    gestureNote(`view: ${v || 'hero'}`);
    hideGestureSoon();
  }
  let gestureTimer = 0;
  function gestureNote(t) { const g = $('#gesture'); g.textContent = t; g.hidden = false; clearTimeout(gestureTimer); }
  function hideGestureSoon() { clearTimeout(gestureTimer); gestureTimer = setTimeout(() => ($('#gesture').hidden = true), 700); }

  // ---------- state appliers ----------
  function applyParams(p) {
    if (!p) return;
    const keep = {};
    for (const k of S.editing) if (S.params && k in S.params) keep[k] = S.params[k];
    S.params = { ...p, ...keep };
    if (S.params.view && S.params.view !== 'custom') S.ui.lastPreset = S.params.view;
    syncAll();
  }
  function syncAll() {
    if (!S.params) return;
    for (const f of S.sync) f(S.params);
    draw();
  }

  function applyPiece(pc) {
    S.piece = pc || { status: 'none' };
    const cur = S.piece.current;
    $('#pieceName').textContent = S.piece.status === 'preparing' ? S.piece.name : cur ? cur.name : 'Choose a piece';
    const prep = S.piece.status === 'preparing';
    $('#preparing').hidden = !prep;
    if (prep) {
      $('#prepName').textContent = S.piece.name;
      $('#prepBar').style.width = `${Math.round((S.piece.progress || 0) * 100)}%`;
      const left = Math.max(0, (S.piece.estimate || 0) - (S.piece.elapsed || 0));
      $('#prepNote').textContent = `posing, building the BVH and the patina mask · ${(S.piece.elapsed || 0).toFixed(0)} s${left > 1 ? ` · about ${Math.ceil(left)} s left` : ''}`;
    }
    $('#empty').hidden = prep || !!cur;
    if (S.piece.status === 'failed' && S.piece.error && S._lastErr !== S.piece.error) {
      S._lastErr = S.piece.error;
      toast(`Couldn't open ${S.piece.name}: ${S.piece.error}`, true);
    }
    renderPiecePane();
    renderStones();
    if (S.library) renderLibrary();
  }

  function applyJobs(j) {
    if (!j) return;
    S.jobs = j;
    const active = j.active || [];
    const b = $('#jobBadge');
    b.hidden = !active.length;
    b.textContent = active.length;
    const run = active.find((x) => x.status === 'running');
    const sh = $('#shutter');
    sh.classList.toggle('busy', !!run);
    $('.prog', sh).style.strokeDashoffset = run ? String(182.2 * (1 - (run.progress || 0))) : '182.2';
    const pb = $('#paused');
    pb.hidden = !run;
    if (run) pb.textContent = `Rendering ${run.kind} · ${Math.round((run.progress || 0) * 100)}%${run.eta_s ? ` · ${fmtDur(run.eta_s)} left` : ''} — preview paused`;
    renderJobs();
    if (j.log_version !== S.galleryVersion) loadGallery(j.log_version);
  }
  function applyLive(l) {
    if (!l) return;
    S.live = { ...S.live, ...l };
  }

  // ---------- panes ----------
  function setTab(t, toggle) {
    if (toggle && t === S.tab && window.matchMedia('(max-width: 899px)').matches) {
      S.collapsed = !S.collapsed;
    } else {
      S.collapsed = false;
      S.tab = t;
      store.set('tab', t);
    }
    $('#panel').classList.toggle('collapsed', S.collapsed);
    $$('#tabs button').forEach((b) => b.setAttribute('aria-selected', String(b.dataset.tab === S.tab && !S.collapsed)));
    $$('.pane').forEach((p) => p.classList.toggle('on', p.dataset.pane === S.tab));
    if (S.tab === 'render') refreshLive();
    requestAnimationFrame(sizeCanvas);
  }
  $$('#tabs button').forEach((b) => b.addEventListener('click', () => setTab(b.dataset.tab, true)));

  const pane = (n) => $(`.pane[data-pane="${n}"]`);
  const section = (label, ...kids) => h('div', { class: 'section' }, label ? h('div', { class: 'label' }, ...[].concat(label)) : null, ...kids);

  function chipRow(items, { key, cls, onPick, isOn }) {
    const row = h('div', { class: 'chips' + (cls ? ' ' + cls : '') });
    const btns = items.map((it) => {
      const b = h('button', { type: 'button', class: 'chip', title: it.title || null, onclick: () => onPick(it) }, it.dot ? h('span', { class: 'dot', style: { background: it.dot } }) : null, it.name, it.small ? h('small', {}, it.small) : null);
      b._it = it;
      row.append(b);
      return b;
    });
    S.sync.push((p) => btns.forEach((b) => b.classList.toggle('on', isOn ? isOn(b._it, p) : p[key] === b._it.key)));
    return row;
  }
  function segment(items, { key, onPick, isOn }) {
    const el = h('div', { class: 'seg' });
    const btns = items.map((it) => {
      const b = h('button', { type: 'button', onclick: () => onPick(it) }, it.name);
      b._it = it;
      el.append(b);
      return b;
    });
    const upd = (p) => btns.forEach((b) => b.classList.toggle('on', isOn ? isOn(b._it, p) : p[key] === b._it.key));
    S.sync.push(upd);
    el._update = () => S.params && upd(S.params);
    return el;
  }
  function slider({ key, label, min, max, step, fmt, ends, get, set }) {
    const val = h('span', { class: 'val' });
    const input = h('input', { type: 'range', min, max, step, 'aria-label': label });
    const read = get || ((p) => p[key]);
    const write = set || ((v) => ({ [key]: v }));
    let timer = 0;
    input.addEventListener('input', () => {
      S.editing.add(key);
      const v = parseFloat(input.value);
      val.textContent = fmt(v);
      setLook(write(v));
      clearTimeout(timer);
    });
    input.addEventListener('change', () => {
      flush();
      clearTimeout(timer);
      timer = setTimeout(() => { S.editing.delete(key); syncAll(); }, 450);
    });
    S.sync.push((p) => {
      if (S.editing.has(key)) return;
      const v = read(p);
      if (v == null) return;
      input.value = v;
      val.textContent = fmt(v);
    });
    return h('div', { class: 'section' }, h('div', { class: 'label' }, h('span', {}, label), val), input, ends ? h('div', { class: 'ends' }, h('span', {}, ends[0]), h('span', {}, ends[1])) : null);
  }

  function metalDot(sw) {
    return `radial-gradient(circle at 32% 28%, rgba(255,255,255,.9), ${sw} 38%, ${sw} 58%, rgba(0,0,0,.6) 100%)`;
  }

  function buildPanes() {
    S.sync = [];
    const o = S.options;
    // Metal
    const pm = pane('metal');
    pm.replaceChildren();
    const metals = o.metals.map((m) => ({ ...m, dot: metalDot(m.swatch) }));
    const house = HOUSE.map((k) => metals.find((m) => m.key === k)).filter(Boolean).map((m) => ({ ...m, name: HOUSE_NAME[m.key] || m.name }));
    const rest = metals.filter((m) => !HOUSE.includes(m.key)).map((m) => ({ ...m, name: m.name.replace(/ gold$/, '').replace(/^\.925 /, '') }));
    const pick = (m) => setLook({ metal: m.key }, true);
    S.sync.push((p) => { if (S._pieceMetal !== p.metal) { S._pieceMetal = p.metal; renderPiecePane(); } });
    const moreOpen = store.get('moreMetals', false);
    const more = h('details', { open: moreOpen || null, ontoggle: (e) => store.set('moreMetals', e.target.open) }, h('summary', {}, 'All alloys'), chipRow(rest, { key: 'metal', onPick: pick }));
    pm.append(
      section(['Metal', h('span', { class: 'val', id: 'metalName' })], chipRow(house, { key: 'metal', onPick: pick }), more),
      slider({ key: 'patina', label: 'Patina', min: 0, max: 1, step: 0.01, fmt: (v) => v.toFixed(2), ends: ['bright polish', 'black recesses'] }),
      slider({ key: 'roughness', label: 'Polish', min: 0.02, max: 0.4, step: 0.005, fmt: (v) => v.toFixed(3), ends: ['mirror', 'satin'] }),
      h('div', { id: 'stones' }),
    );
    S.sync.push((p) => { const m = o.metals.find((x) => x.key === p.metal); $('#metalName').textContent = m ? m.name : p.metal; });

    // Light
    const pl = pane('light');
    pl.replaceChildren();
    const studios = o.studios.map((s) => ({ ...s, title: s.blurb }));
    const blurb = h('p', { class: 'hint' });
    S.sync.push((p) => { const s = o.studios.find((x) => x.key === p.studio); blurb.textContent = s ? s.blurb : ''; });
    const hdriBox = h('div', { id: 'hdriBox', hidden: true },
      section('Environment', h('div', { class: 'chips', id: 'hdriChips' }), h('p', { class: 'hint' }, 'The studio keeps its floor; the softboxes give way to the photographed room.')),
      slider({ key: 'hdri_rotate', label: 'Rotate the room', min: 0, max: 359, step: 1, fmt: (v) => `${Math.round(v)}°`, ends: ['', 'moves the highlights'] }),
      slider({ key: 'hdri_exposure', label: 'Room brightness', min: -3, max: 3, step: 0.05, fmt: (v) => `${v > 0 ? '+' : ''}${v.toFixed(2)} EV` }),
    );
    const mode = segment([{ key: 'studio', name: 'Studio lights' }, { key: 'hdri', name: 'HDRI' }], {
      isOn: (it, p) => (it.key === 'hdri') === !!p.hdri,
      onPick: (it) => {
        if (it.key === 'studio') setLook({ hdri: 'none' }, true);
        else {
          const first = S.library && S.library.hdris && S.library.hdris[0];
          if (first) setLook({ hdri: first.path }, true);
          else toast('No .hdr files in the library yet', true);
        }
      },
    });
    pl.append(
      section('Lighting', mode),
      section('Studio', chipRow(studios, { key: 'studio', onPick: (s) => setLook({ studio: s.key }, true) }), blurb),
      hdriBox,
      slider({ key: 'exposure', label: 'Exposure', min: -3, max: 3, step: 0.05, fmt: (v) => `${v > 0 ? '+' : ''}${v.toFixed(2)} EV`, ends: ['darker', 'brighter'] }),
      section('Tone', segment(o.grades.map((g) => ({ key: g, name: g === 'aces' ? 'ACES' : g[0].toUpperCase() + g.slice(1) })), { key: 'grade', onPick: (g) => setLook({ grade: g.key }, true) }), h('p', { class: 'hint' }, 'Neutral keeps product colour true; Punchy, Base and Golden are AgX looks.')),
      section(null, switchRow('Denoise the preview', 'denoise')),
      section('Preview quality', segment(o.preview_spp.map((n) => ({ key: n, name: n >= 1024 ? `${n / 1024}k` : String(n) })), { key: 'max_spp', onPick: (it) => setLook({ max_spp: it.key }, true) }), h('p', { class: 'hint' }, 'Samples per pixel before the live view stops refining.')),
    );
    S.sync.push(() => renderHdri());

    // Camera
    const pc = pane('camera');
    pc.replaceChildren();
    pc.append(
      section('Frame', segment(o.formats.map((f) => ({ key: f.key, name: f.name })), { key: 'format', onPick: (f) => setLook({ format: f.key }, true) })),
      section('View', chipRow(o.views.map((v) => ({ key: v.key, name: v.key.replace('-', ' ') })), { key: 'view', onPick: (v) => setLook({ view: v.key }, true) }), h('p', { class: 'hint' }, 'Drag the picture to orbit, pinch or scroll to zoom, double-tap to snap back.')),
      slider({ key: 'azimuth', label: 'Around', min: -180, max: 180, step: 0.5, fmt: (v) => `${Math.round(v)}°` }),
      slider({ key: 'elevation', label: 'Height', min: -30, max: 89, step: 0.5, fmt: (v) => `${Math.round(v)}°` }),
      slider({ key: 'fill', label: 'Zoom', min: 0.2, max: 2.5, step: 0.01, fmt: (v) => v.toFixed(2), ends: ['wide', 'macro'] }),
      slider({ key: 'turntable', label: 'Turn the piece', min: 0, max: 359, step: 1, fmt: (v) => `${Math.round(v)}°` }),
      slider({ key: 'focal', label: 'Lens', min: 35, max: 300, step: 1, fmt: (v) => `${Math.round(v)} mm` }),
      section('Aperture', apertureSelect()),
    );

    // Render
    renderRenderPane();
    setTab(S.tab);
  }

  function switchRow(label, key) {
    const input = h('input', { type: 'checkbox', onchange: () => setLook({ [key]: input.checked }, true) });
    S.sync.push((p) => (input.checked = !!p[key]));
    return h('label', { class: 'switch' }, h('span', {}, label), input);
  }

  function apertureSelect() {
    const sel = h('select', {
      'aria-label': 'Aperture',
      onchange: () => setLook({ fstop: sel.value === 'auto' ? 'auto' : parseFloat(sel.value) }, true),
    }, APERTURES.map(([v, n]) => h('option', { value: v }, n)));
    const hint = h('p', { class: 'hint' });
    S.sync.push((p) => {
      const v = p.fstop == null ? 'auto' : String(p.fstop);
      if (![...sel.options].some((o) => o.value === v)) sel.append(h('option', { value: v }, `f/${v}`));
      sel.value = v;
      hint.textContent = p.fstop == null ? (p.studio === 'white' ? 'Auto: everything sharp in the white studio.' : 'Auto: f/16 — a little depth in the mood studios.') : p.fstop === 0 ? 'Everything sharp.' : 'Focus sits on the nearest surface at the centre.';
    });
    return h('div', {}, sel, hint);
  }

  function renderHdri() {
    const box = $('#hdriBox');
    if (!box || !S.params) return;
    box.hidden = !S.params.hdri;
    const list = (S.library && S.library.hdris) || [];
    const key = JSON.stringify([S.params.hdri, list.map((x) => x.id)]);
    if (box._key === key) return;
    box._key = key;
    $('#hdriChips').replaceChildren(...list.map((x) =>
      h('button', { type: 'button', class: 'chip' + (x.path === S.params.hdri ? ' on' : ''), title: x.rel, onclick: () => setLook({ hdri: x.path }, true) }, x.name)));
  }

  // ---------- piece pane ----------
  function fmtTris(n) { return n >= 1e6 ? `${(n / 1e6).toFixed(1)}M` : n >= 1e3 ? `${Math.round(n / 1e3)}k` : String(n); }
  function fmtMB(b) { return b >= 1e6 ? `${(b / 1e6).toFixed(b >= 1e8 ? 0 : 1)} MB` : `${Math.max(1, Math.round(b / 1e3))} kB`; }
  function fmtDur(s) { s = Math.max(0, s); return s < 60 ? `${Math.round(s)} s` : s < 3600 ? `${Math.floor(s / 60)} min ${Math.round(s % 60)} s` : `${Math.floor(s / 3600)} h ${Math.round((s % 3600) / 60)} min`; }
  function ago(t) { const s = Date.now() / 1000 - t; return s < 60 ? 'just now' : s < 3600 ? `${Math.round(s / 60)} min ago` : s < 86400 ? `${Math.round(s / 3600)} h ago` : `${Math.round(s / 86400)} d ago`; }

  function renderPiecePane() {
    const p = pane('piece');
    if (!p || !S.options) return;
    p.replaceChildren();
    const cur = S.piece && S.piece.current;
    if (!cur) {
      p.append(h('div', { class: 'card' }, h('h2', {}, 'No piece on set'), h('p', { class: 'muted small' }, 'Open one from the library; big castings take a few seconds to prepare.'), h('button', { class: 'btn primary block', type: 'button', onclick: openLibrary }, 'Open the library')));
      return;
    }
    const m = S.params && S.params.metal;
    const wt = cur.weights_g || {};
    const metalName = (k) => (S.options.metals.find((x) => x.key === k) || { name: k }).name;
    const size = cur.size_mm ? cur.size_mm.map((v) => v.toFixed(1)).join(' × ') + ' mm' : '';
    const facts = [
      cur.us_size != null ? ['Ring size', `US ${cur.us_size}`] : null,
      cur.bore_mm != null ? ['Bore', `${cur.bore_mm.toFixed(2)} mm`] : null,
      ['Size', size],
      ['Mesh', `${fmtTris(cur.triangles)} tris`],
      ...HOUSE.map((k) => (wt[k] != null ? [HOUSE_NAME[k], `${wt[k].toFixed(1)} g`] : null)),
      m && !HOUSE.includes(m) && wt[m] != null ? [metalName(m), `${wt[m].toFixed(1)} g`] : null,
    ].filter(Boolean);
    const card = h('div', { class: 'card' },
      h('p', { class: 'eyebrow' }, cur.kind === 'Other' ? 'Piece' : cur.kind),
      h('h2', {}, cur.name),
      h('div', { class: 'facts' }, facts.map(([k, v]) => h('div', { class: 'fact' }, h('b', {}, v), h('span', {}, k)))),
      h('p', { class: 'hint' }, 'Weights from the mesh volume (stones excluded); gold-filled is priced by its brass core, so use them as casting estimates.'),
    );
    const reopen = (extra) => api('POST', '/api/open', { path: cur.demo ? 'demo:ring' : cur.files[0], flip: cur.flip, rotate: cur.rotate, units: cur.units, chain: cur.chain, fit: cur.fit, scale: cur.scale, ...extra }).catch(toastErr);
    const fitIn = h('input', { type: 'number', min: '0.5', max: '500', step: '0.5', placeholder: 'e.g. 32', value: cur.fit || '' });
    const unitsSel = h('select', { 'aria-label': 'File units' }, ['mm', 'cm', 'm', 'in'].map((u) => h('option', { value: u, selected: cur.units === u || null }, u)));
    const actions = h('div', { class: 'card' },
      h('div', { class: 'row' },
        h('button', { class: 'btn', type: 'button', onclick: openLibrary }, 'Library'),
        h('button', { class: 'btn', type: 'button', disabled: cur.demo || null, onclick: () => reopen({ flip: !cur.flip }) }, cur.flip ? 'Unflip' : 'Flip 180°'),
      ),
      cur.kind === 'Pendant' || cur.chain !== 'auto' ? h('div', { class: 'section', style: { marginTop: '14px' } }, h('div', { class: 'label' }, 'Chain'), segChain(cur, reopen)) : null,
      h('details', { style: { marginTop: '10px' } },
        h('summary', {}, 'Units and scale'),
        h('div', { class: 'grid2', style: { marginTop: '8px' } },
          h('label', {}, h('div', { class: 'label' }, 'File units'), unitsSel),
          h('label', {}, h('div', { class: 'label' }, 'Fit largest side (mm)'), fitIn),
        ),
        h('button', { class: 'btn small', type: 'button', style: { marginTop: '10px' }, onclick: () => { const f = parseFloat(fitIn.value); reopen({ units: unitsSel.value, fit: f > 0 ? f : null }); } }, 'Re-open with these'),
        h('p', { class: 'hint' }, 'For sculpts exported in unknown units: scale the piece so its largest side is this long.'),
      ),
      cur.notes && cur.notes.length ? h('p', { class: 'hint' }, cur.notes.join(' · ')) : null,
    );
    p.append(card, actions);
  }
  function segChain(cur, reopen) {
    const el = h('div', { class: 'seg' });
    for (const k of ['auto', 'on', 'off']) el.append(h('button', { type: 'button', class: cur.chain === k ? 'on' : null, onclick: () => reopen({ chain: k }) }, k));
    return el;
  }

  // ---------- stones ----------
  function renderStones() {
    const box = $('#stones');
    if (!box || !S.options) return;
    const cur = S.piece && S.piece.current;
    const parts = (cur && cur.parts) || [];
    const stoneParts = parts.filter((p) => p.gem);
    if (parts.length < 2 && !stoneParts.length) { box.replaceChildren(); return; }
    const gems = S.options.gems;
    const label = (g) => g.replace(/_/g, ' ');
    const row = (part) => {
      const sel = h('select', { 'aria-label': `Stone for ${part.name}`, onchange: () => setLook({ stone: { part: part.name, gem: sel.value || 'metal' } }, true) },
        h('option', { value: '' }, `auto · ${part.gem ? label(part.gem) : 'metal'}${part.source === 'file' ? ' (file)' : ''}`),
        gems.map((g) => h('option', { value: g }, label(g))),
      );
      sel.value = part.source === 'set' ? part.gem : '';
      return h('div', { class: 'part' }, h('div', { class: 'nm' }, part.name, h('small', {}, `${fmtTris(part.triangles)} tris${part.gem ? '' : ' · metal'}`)), sel);
    };
    const all = h('select', { 'aria-label': 'Every stone', onchange: () => { if (all.value) setLook({ stones: stoneParts.map((p) => ({ part: p.name, gem: all.value })) }, true); } },
      h('option', { value: '' }, 'Set every stone to…'), gems.map((g) => h('option', { value: g }, label(g))));
    const list = h('div', { class: 'parts' }, parts.map(row));
    box.replaceChildren(
      section(['Stones', h('span', { class: 'val' }, `${stoneParts.length} of ${parts.length} parts`)],
        stoneParts.length > 1 ? h('div', { style: { marginBottom: '10px' } }, all) : null,
        parts.length > 8 ? h('details', {}, h('summary', {}, `Every part (${parts.length})`), list) : list,
        h('p', { class: 'hint' }, '"auto" uses the gem named in the CAD export, or diamond for unnamed stones.'),
      ),
    );
  }

  // ---------- render pane ----------
  async function refreshLive() {
    try { const st = await api('GET', '/api/state'); S.live = st.live || S.live; renderEstimates(); } catch { /* offline */ }
  }
  function throughput() { return Math.max(2, (S.live && S.live.msamples_per_s) || 20) * 1e6; }
  function renderEstimates() {
    for (const el of $$('[data-est]')) el.textContent = el._est ? `≈ ${fmtDur(el._est())}` : '';
  }

  function sppSeg(values, get, set) {
    const el = h('div', { class: 'seg' });
    const btns = values.map((n) => h('button', { type: 'button', onclick: () => { set(n); upd(); renderEstimates(); } }, String(n)));
    const upd = () => btns.forEach((b, i) => b.classList.toggle('on', values[i] === get()));
    el.append(...btns);
    upd();
    return el;
  }

  function renderRenderPane() {
    const p = pane('render');
    const o = S.options;
    p.replaceChildren();
    // Still
    const stillEst = h('span', { class: 'est', 'data-est': '' });
    stillEst._est = () => { const [w, hh] = formatSize(); return (w * hh * S.ui.stillSpp) / throughput() + (w * hh) / 1.6e6 * 2.5 + 1; };
    const fmtLabel = h('span', { class: 'val' });
    S.sync.push(() => { const [w, hh] = formatSize(); fmtLabel.textContent = `${w}×${hh}`; renderEstimates(); });
    p.append(h('div', { id: 'jobList' }));
    p.append(h('div', { class: 'card' },
      h('div', { class: 'label' }, h('span', {}, 'Still'), fmtLabel),
      sppSeg(o.still_spp, () => S.ui.stillSpp, (n) => { S.ui.stillSpp = n; store.set('stillSpp', n); }),
      h('div', { class: 'row', style: { marginTop: '12px' } }, stillEst, h('button', { class: 'btn primary', type: 'button', onclick: renderStill }, 'Render still')),
      h('p', { class: 'hint' }, 'Exactly this frame, at the full size of the chosen format (Camera → Frame).'),
    ));
    // Sheet
    S.ui.sheetMetals = S.ui.sheetMetals || [...o.sheet.metals];
    S.ui.sheetViews = S.ui.sheetViews || [...o.sheet.views];
    const toggles = (items, list, label) => {
      const row = h('div', { class: 'chips' });
      for (const it of items) {
        const b = h('button', { type: 'button', class: 'chip' + (list.includes(it.key) ? ' on' : ''), onclick: () => { const i = list.indexOf(it.key); if (i >= 0) { if (list.length > 1) list.splice(i, 1); } else list.push(it.key); b.classList.toggle('on', list.includes(it.key)); renderEstimates(); } }, it.dot ? h('span', { class: 'dot', style: { background: it.dot } }) : null, it.name);
        row.append(b);
      }
      return section(label, row);
    };
    const sheetEst = h('span', { class: 'est', 'data-est': '' });
    sheetEst._est = () => (S.ui.sheetMetals.length * S.ui.sheetViews.length * (1600 * 1600 * S.ui.sheetSpp / throughput() + 2.5));
    const title = h('input', { type: 'text', placeholder: 'Title (default: the piece name)', maxlength: '80' });
    const metalItems = o.metals.map((m) => ({ key: m.key, name: HOUSE_NAME[m.key] || m.name.replace(/^\.925 /, ''), dot: metalDot(m.swatch) }));
    p.append(h('div', { class: 'card' },
      h('div', { class: 'label' }, h('span', {}, 'Listing sheet'), h('span', { class: 'val' }, '1600×1600 tiles')),
      toggles(metalItems.filter((m) => HOUSE.includes(m.key)).concat(metalItems.filter((m) => !HOUSE.includes(m.key))), S.ui.sheetMetals, 'Metals'),
      toggles(o.views.map((v) => ({ key: v.key, name: v.key.replace('-', ' ') })), S.ui.sheetViews, 'Views'),
      section('Samples', sppSeg([64, 128, 256], () => S.ui.sheetSpp, (n) => (S.ui.sheetSpp = n))),
      title,
      h('div', { class: 'row', style: { marginTop: '12px' } }, sheetEst, h('button', { class: 'btn primary', type: 'button', onclick: () => renderSheet(title.value) }, 'Render sheet')),
      h('p', { class: 'hint' }, 'Every metal × view as stills plus a captioned contact sheet with weights, in the current studio and finish.'),
    ));
    // Reel
    const reelEst = h('span', { class: 'est', 'data-est': '' });
    reelEst._est = () => S.ui.seconds * 30 * ((1080 * 1920 * S.ui.reelSpp) / throughput() + 1.2);
    const hook = h('input', { type: 'text', placeholder: 'Hook over the first 2 s (≤ 7 words reads best)', maxlength: '120', value: S.ui.hook, oninput: () => (S.ui.hook = hook.value) });
    const audioSel = h('select', { 'aria-label': 'Soundtrack', onchange: () => (S.ui.audio = audioSel.value) }, h('option', { value: '' }, 'Silent track'));
    const fillAudio = () => {
      const list = (S.library && S.library.audio) || [];
      audioSel.replaceChildren(h('option', { value: '' }, 'Silent track'), ...list.map((a) => h('option', { value: a.path }, a.name)));
      audioSel.value = S.ui.audio;
    };
    audioSel._fill = fillAudio;
    fillAudio();
    const secVal = h('span', { class: 'val' }, `${S.ui.seconds} s`);
    const secIn = h('input', { type: 'range', min: 3, max: 20, step: 1, value: S.ui.seconds, oninput: () => { S.ui.seconds = +secIn.value; secVal.textContent = `${S.ui.seconds} s`; renderEstimates(); } });
    p.append(h('div', { class: 'card', id: 'reelCard' },
      h('div', { class: 'label' }, h('span', {}, 'Reel'), h('span', { class: 'val' }, '1080×1920 · 30 fps')),
      section('Motion', sppSegNamed([['turntable', 'Turntable'], ['reveal', 'Reveal'], ['sweep', 'Light sweep']], () => S.ui.rig, (v) => (S.ui.rig = v))),
      h('div', { class: 'section' }, h('div', { class: 'label' }, h('span', {}, 'Length'), secVal), secIn),
      section('Samples per frame', sppSeg([16, 32, 64], () => S.ui.reelSpp, (n) => (S.ui.reelSpp = n))),
      hook,
      h('div', { style: { marginTop: '10px' } }, audioSel),
      h('div', { class: 'row', style: { marginTop: '12px' } }, reelEst, h('button', { class: 'btn primary', type: 'button', onclick: renderReel }, 'Render reel')),
      h('p', { class: 'hint' }, "Forge's reel spec: h264 + AAC, ready for review. The live view pauses while it renders."),
    ));
    renderJobs();
    renderEstimates();
  }
  function sppSegNamed(items, get, set) {
    const el = h('div', { class: 'seg' });
    const btns = items.map(([v, n]) => h('button', { type: 'button', onclick: () => { set(v); upd(); } }, n));
    const upd = () => btns.forEach((b, i) => b.classList.toggle('on', items[i][0] === get()));
    el.append(...btns);
    upd();
    return el;
  }

  async function submit(body, what) {
    try {
      const j = await api('POST', '/api/render', body);
      toast(`${what} queued (${j.id})`);
    } catch (e) { toastErr(e); }
  }
  function renderStill() { submit({ kind: 'still', spp: S.ui.stillSpp }, 'Still'); }
  function renderSheet(title) { submit({ kind: 'sheet', metals: S.ui.sheetMetals, views: S.ui.sheetViews, spp: S.ui.sheetSpp, title: title || undefined }, 'Listing sheet'); }
  function renderReel() { submit({ kind: 'reel', seconds: S.ui.seconds, rig: S.ui.rig, spp: S.ui.reelSpp, hook: S.ui.hook || undefined, audio: S.ui.audio || undefined }, 'Reel'); }
  $('#shutter').addEventListener('click', () => {
    if (!S.piece || !S.piece.current) { openLibrary(); return; }
    renderStill();
  });

  function renderJobs() {
    const box = $('#jobList');
    if (!box) return;
    const active = (S.jobs && S.jobs.active) || [];
    const failed = (S.gallery || []).filter((j) => j.status === 'failed' && Date.now() / 1000 - (j.finished || 0) < 3600).slice(0, 3);
    if (!active.length && !failed.length) { box.replaceChildren(); return; }
    box.replaceChildren(section('Queue', h('div', { class: 'jobs' },
      active.map((j) => h('div', { class: 'job' },
        h('div', { class: 'top' }, h('b', {}, `${j.kind} · ${j.piece}`), j.status === 'running' ? h('span', { class: 'muted small' }, `${Math.round((j.progress || 0) * 100)}%`) : h('button', { class: 'btn small', type: 'button', onclick: () => api('POST', `/api/jobs/${j.id}/cancel`).catch(toastErr) }, 'Cancel')),
        h('div', { class: 'msg' }, j.message + (j.eta_s ? ` · ${fmtDur(j.eta_s)} left` : '')),
        j.status === 'running' ? h('div', { class: 'bar' }, h('i', { style: { width: `${Math.round((j.progress || 0) * 100)}%` } })) : null,
        j.status === 'running' && j.kind === 'still' ? h('button', { class: 'btn small', type: 'button', style: { marginTop: '8px' }, onclick: () => api('POST', `/api/jobs/${j.id}/cancel`).catch(toastErr) }, 'Stop') : null,
      )),
      failed.map((j) => h('div', { class: 'job failed' }, h('div', { class: 'top' }, h('b', {}, `${j.kind} · ${j.piece}`), h('span', { class: 'muted small' }, ago(j.finished || j.created))), h('div', { class: 'msg' }, j.error || 'failed'))),
    )));
  }

  // ---------- gallery ----------
  async function loadGallery(version) {
    try {
      const r = await api('GET', '/api/jobs?limit=120');
      S.gallery = r.jobs || [];
      S.galleryVersion = version != null ? version : r.log_version;
      renderGallery();
      renderJobs();
    } catch { /* offline */ }
  }
  function renderGallery() {
    const p = pane('gallery');
    if (!p) return;
    const done = S.gallery.filter((j) => j.status === 'done' && j.outputs && j.outputs.length);
    if (!done.length) { p.replaceChildren(h('p', { class: 'empty-note' }, 'Finished stills, sheets and reels land here.')); return; }
    const grid = h('div', { class: 'gallery' });
    for (const j of done) {
      const main = j.outputs[0];
      grid.append(h('button', { type: 'button', class: 'tile', onclick: () => openJob(j) },
        main.thumb ? h('img', { src: main.thumb, alt: main.label, loading: 'lazy' }) : h('div', { class: 'cap' }, main.label),
        h('span', { class: 'kind' }, j.kind),
        h('div', { class: 'cap' }, `${j.piece.split(' · ')[0]}`, h('br'), h('span', { class: 'muted' }, `${(j.spec && j.spec.set && j.spec.set.metal) || ''} · ${ago(j.finished || j.created)}`)),
      ));
    }
    p.replaceChildren(grid);
  }

  const LOOK_KEYS = ['metal', 'patina', 'roughness', 'studio', 'exposure', 'grade', 'stones', 'hdri', 'hdri_rotate', 'hdri_exposure', 'azimuth', 'elevation', 'fill', 'turntable', 'focal', 'fstop', 'format'];
  function openJob(j) {
    const lb = $('#lightbox');
    const body = $('#lbBody');
    $('#lbTitle').textContent = `${j.kind} · ${j.piece}`;
    const main = j.outputs[0];
    const show = (o) => o.kind === 'video'
      ? h('video', { src: o.url, poster: o.thumb || null, controls: true, playsinline: true, loop: true, autoplay: true, muted: true })
      : h('img', { src: o.url, alt: o.label });
    const stage = h('div', {}, show(main));
    const dl = h('a', { class: 'btn primary', href: main.url + '?dl=1', download: '' }, 'Download');
    const open = h('a', { class: 'btn', href: main.url, target: '_blank', rel: 'noopener' }, 'Full size');
    const pick = (o) => { stage.replaceChildren(show(o)); dl.href = o.url + '?dl=1'; open.href = o.url; };
    const imgs = j.outputs.filter((o) => o.kind !== 'data');
    const set = j.spec && j.spec.set;
    const restore = set ? h('button', { class: 'btn', type: 'button', onclick: () => {
      const patch = {};
      for (const k of LOOK_KEYS) if (k in set) patch[k] = set[k];
      if (patch.fstop == null) patch.fstop = 'auto';
      if (!patch.hdri) patch.hdri = 'none';
      if (S.piece && S.piece.current && S.piece.current.id !== j.piece_id) delete patch.stones;
      setLook(patch, true);
      closeSheet(lb);
      toast('Look restored');
    } }, 'Use this look') : null;
    const job = j.spec && j.spec.job;
    const meta = [
      set ? `${(S.options.metals.find((m) => m.key === set.metal) || { name: set.metal }).name} · ${set.studio}${set.hdri ? ' + HDRI' : ''} · patina ${(+set.patina).toFixed(2)}` : null,
      job ? [job.size, job.spp ? `${job.spp} spp` : null, job.seconds ? `${job.seconds} s ${job.rig || ''}` : null].filter(Boolean).join(' · ') : null,
      j.elapsed_s ? `rendered in ${fmtDur(j.elapsed_s)}, ${ago(j.finished || j.created)}` : null,
    ].filter(Boolean).join('\n');
    body.replaceChildren(
      stage,
      imgs.length > 1 ? h('div', { class: 'strip' }, imgs.map((o) => h('img', { src: o.thumb || o.url, alt: o.label, title: o.label, loading: 'lazy', onclick: () => pick(o) }))) : null,
      h('div', { class: 'actions' }, dl, open, restore),
      h('p', { class: 'meta', style: { whiteSpace: 'pre-line' } }, meta),
    );
    openSheet(lb);
  }

  // ---------- library ----------
  async function loadLibrary() {
    try {
      S.library = await api('GET', '/api/library');
      renderLibrary();
      renderHdri();
      const a = $('#reelCard select');
      if (a && a._fill) a._fill();
    } catch (e) { toastErr(e); }
  }
  function openLibrary() { openSheet($('#library')); loadLibrary(); setTimeout(() => { if (!matchMedia('(pointer: coarse)').matches) $('#libSearch').focus(); }, 50); }
  function renderLibrary() {
    const box = $('#libList');
    if (!S.library) { box.replaceChildren(h('p', { class: 'empty-note' }, 'Loading…')); return; }
    const q = $('#libSearch').value.trim().toLowerCase();
    const cur = S.piece && S.piece.current && S.piece.current.id;
    const prepId = S.piece && S.piece.status === 'preparing' ? S.piece.id : null;
    const items = S.library.pieces.filter((p) => !q || p.name.toLowerCase().includes(q) || p.rel.toLowerCase().includes(q));
    if (!items.length) { box.replaceChildren(h('p', { class: 'empty-note' }, S.library.pieces.length ? 'Nothing matches.' : 'No meshes in the library yet (STL, OBJ, PLY, 3MF).')); return; }
    const groups = new Map();
    for (const it of items) { if (!groups.has(it.group)) groups.set(it.group, []); groups.get(it.group).push(it); }
    // Biggest collections first (the castings); groups of tiny files (gem cut
    // models, test parts) fold away unless searched for.
    const bytes = (list) => list.reduce((a, it) => a + (it.bytes || 0), 0);
    const ordered = [...groups.entries()].sort((a, b) => (b[0] === 'demo') - (a[0] === 'demo') || bytes(b[1]) - bytes(a[1]));
    const out = [];
    for (const [g, list] of ordered) {
      const small = !q && g !== 'demo' && list.every((it) => (it.bytes || 0) < 200e3) && !list.some((it) => it.id === cur);
      const rows = [];
      for (const it of list) {
        const isCur = it.id === cur;
        rows.push(h('button', { type: 'button', class: 'lib-item' + (isCur ? ' cur' : ''), onclick: () => openPiece(it) },
          h('div', { class: 't' }, h('b', {}, it.name), h('small', {}, [it.rel, it.bytes ? fmtMB(it.bytes) : null, it.triangles ? `${fmtTris(it.triangles)} tris` : null].filter(Boolean).join(' · '))),
          it.stones ? h('span', { class: 'badge' }, 'stones') : null,
          isCur ? h('span', { class: 'badge gold' }, 'on set') : it.id === prepId ? h('span', { class: 'badge gold' }, 'preparing') : it.cached ? h('span', { class: 'badge' }, 'ready') : null,
        ));
      }
      if (small) out.push(h('details', { class: 'group' }, h('summary', {}, h('div', { class: 'group-title' }, g), h('small', {}, `${list.length} small files`)), rows));
      else out.push(h('div', { class: 'group-title' }, g), ...rows);
    }
    box.replaceChildren(...out);
  }
  $('#libSearch').addEventListener('input', renderLibrary);
  async function openPiece(it) {
    closeSheet($('#library'));
    try {
      const r = await api('POST', '/api/open', { id: it.id });
      if (r.status === 'ready') toast(`${it.name} on set`);
    } catch (e) { toastErr(e); }
  }

  // ---------- sheets ----------
  function openSheet(el) { el.hidden = false; document.body.style.overflow = 'hidden'; }
  function closeSheet(el) {
    el.hidden = true;
    document.body.style.overflow = '';
    $$('video', el).forEach((v) => v.pause());
  }
  $$('.sheet [data-close]').forEach((b) => b.addEventListener('click', () => closeSheet(b.closest('.sheet'))));
  document.addEventListener('keydown', (e) => { if (e.key === 'Escape') $$('.sheet').forEach((s) => !s.hidden && closeSheet(s)); });
  $('#pieceBtn').addEventListener('click', openLibrary);
  $$('[data-open-library]').forEach((b) => b.addEventListener('click', openLibrary));

  // ---------- go ----------
  sizeCanvas();
  connect();
})();
