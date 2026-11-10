let cutter = null;
// the helper's LEAD_MS and HOLD_MS
const CUT_LEAD_MS = 1500;
const CUT_HOLD_MS = 500;
const CUT_SHORTEST_MS = 1000;
const CUT_FIRST_MS = 30000;
// tall enough to stay sharp in a wide window
const CUT_DRAW_PX = 640;
const CUT_LINE_PX = 64;
const CUT_HANDLE_PX = 10;
const CUT_SNAP_PX = 6;
const CUT_NEAR_MS = 60;
const CUT_LOOP = "hashi-cut-loop";
const CUT_VOLUME = "hashi-cut-volume";
// the game size window's song volume starts at 60 too, and goes up the same curve
const CUT_VOLUME_FIRST = 60;
const cutGain = (volume) => (Math.min(100, Math.max(0, volume)) / 100) ** 2;
const SCISSORS = `<circle cx="2.5" cy="7.6" r="1.6"/><circle cx="7.5" cy="7.6" r="1.6"/><path d="M3.5 6.4 8.2.6M6.5 6.4 1.8.6"/>`;
const SCISSORS_HALF = `<circle class="loop" cx="5.6" cy="5.2" r="3.7"/><path class="blade" d="M8.1 8.3 12.9 26.6Q10.7 20 9.2 15.8z"/>`;

function openCutter() {
  settingsOpen = goalsOpen = notesOpen = reportOpen = false;
  analysis = maker = null;
  dropTrainer();
  dropCutter();
  skinPage = profilePage = null;
  board = null;
  clearTimeout(boardTimer);
  const c = (cutter = {
    map: null, notes: [], loading: true, error: null, busy: false, made: null, makeError: null,
    from: 0, to: 0, at: 0, end: 1, playing: false, loop: stored(CUT_LOOP, true), speed: stored(SKIN_SPEED, 24),
    volume: Number(stored(CUT_VOLUME, CUT_VOLUME_FIRST)),
    song: null, songState: "loading", skin: null,
    view: el("canvas", { class: "cut-view", width: 240, height: 340, title: "Click to play or pause" }),
    line: cutLineSvg(),
    scissors: cutScissors(),
    clock: el("span", { class: "cut-clock" }),
    fromText: el("b"),
    toText: el("b"),
    picked: el("span", { class: "cut-picked" }),
  });
  c.view.addEventListener("click", () => cutPlay(c));
  cutLineEvents(c);
  render();
  loadCutter(c);
}

function dropCutter() {
  if (!cutter) return;
  cutSilence(cutter);
  cutter.audio?.close().catch(() => {});
  cutter = null;
}

function closeCutter() {
  dropCutter();
  render();
}

async function loadCutter(c) {
  let map;
  try {
    const res = await fetch(`${BRIDGE}/cut/map`);
    map = await res.json();
    if (!res.ok) throw new Error(map.error);
  } catch (e) {
    if (cutter !== c) return;
    c.loading = false;
    c.error = e.message || "Couldn't reach Hashi's helper.";
    return render();
  }
  if (cutter !== c) return;
  c.map = map;
  c.notes = map.notes.map(([col, t, e]) => ({ c: col, t, e })).sort((a, b) => a.t - b.t);
  c.last = c.notes.reduce((m, n) => Math.max(m, n.e ?? n.t), 0);
  c.end = c.last + 2000;
  [c.from, c.to] = cutBusiest(c.notes, CUT_FIRST_MS);
  c.at = Math.max(0, c.from - CUT_LEAD_MS);
  c.loading = false;
  render();
  cutLoadSkin(c);
  cutLoadSong(c);
  requestAnimationFrame((t) => cutTick(c, t));
}

function cutBusiest(notes, span) {
  const first = notes[0].t;
  const last = notes[notes.length - 1].t;
  if (last - first <= span * 1.5) return [first, Math.max(last, first + CUT_SHORTEST_MS)];
  let best = [0, 0];
  for (let i = 0, j = 0; i < notes.length; i++) {
    while (j < notes.length && notes[j].t <= notes[i].t + span) j++;
    if (j - i > best[1] - best[0]) best = [i, j];
  }
  return [notes[best[0]].t, notes[best[1] - 1].t];
}

async function cutLoadSkin(c) {
  let info = null;
  try {
    info = await (await fetch(`${BRIDGE}/skin`)).json();
  } catch {}
  if (cutter !== c) return;
  try {
    // skin scenes stop at 10K
    const loaded = info?.found && c.map.keys <= 10 && (await loadSkinScene({ keys: c.map.keys, stamp: Date.now(), editsJson: "", info }));
    if (cutter === c && loaded?.scene.columns.length === c.map.keys) c.skin = cutLive(loaded);
  } catch {}
  if (cutter !== c) return;
  c.skinTried = true;
  render();
}

function cutLive({ scene, images }) {
  const left = Math.min(...scene.columns.map((col) => col.x));
  const right = Math.max(...scene.columns.map((col) => col.x + col.w));
  const pad = (right - left) * 0.08;
  const x = Math.max(0, left - pad);
  const crop = { x, w: Math.min(scene.width, right + pad) - x };
  const k = Math.max(1, Math.min(3, (CUT_DRAW_PX * (devicePixelRatio || 1)) / scene.height));
  const off = document.createElement("canvas");
  off.width = Math.round(scene.width * k);
  off.height = Math.round(scene.height * k);
  const live = { scene, images, ctx: off.getContext("2d"), k, pressed: [], chart: null, now: 0, crop, off, drawnAt: null };
  skinLayers(live);
  return live;
}

async function cutLoadSong(c) {
  const Context = window.AudioContext ?? window.webkitAudioContext;
  try {
    if (!Context) throw new Error();
    const res = await fetch(`${BRIDGE}/cut/song/${c.map.md5}`);
    if (!res.ok) throw new Error();
    const bytes = await res.arrayBuffer();
    if (cutter !== c) return;
    const rate = new DataView(bytes, 0, 8).getUint32(0, true);
    const pcm = new Int16Array(bytes, 8, (bytes.byteLength - 8) >> 1);
    const audio = new Context();
    const song = audio.createBuffer(1, pcm.length, rate);
    const out = song.getChannelData(0);
    for (let i = 0; i < pcm.length; i++) out[i] = pcm[i] / 32768;
    const gain = audio.createGain();
    gain.gain.value = cutGain(c.volume);
    gain.connect(audio.destination);
    Object.assign(c, { audio, song, gain, songState: "ready" });
    if (c.playing && !c.dragging) cutSound(c);
  } catch {
    if (cutter === c) c.songState = "none";
  }
  if (cutter === c) render();
}

function cutSound(c) {
  cutSilence(c);
  if (!c.song) return;
  c.audio.resume().catch(() => {});
  const source = c.audio.createBufferSource();
  source.buffer = c.song;
  source.connect(c.gain);
  const when = c.audio.currentTime + 0.03;
  source.start(when, Math.max(0, c.at / 1000));
  Object.assign(c, { source, startedAt: when, startedFrom: c.at, ctxTime: null, ctxMoved: performance.now() });
}

function cutSilence(c) {
  try {
    c.source?.stop();
  } catch {}
  c.source = null;
}

// currentTime moves in coarse steps, so the time since its last step is added on. With no audio
// device it stands still, then the page keeps its own time
function cutHeard(c) {
  const a = c.audio;
  if (!a || !c.source) return null;
  const now = performance.now();
  if (a.currentTime !== c.ctxTime) [c.ctxTime, c.ctxMoved] = [a.currentTime, now];
  const stalled = now - c.ctxMoved > 300;
  if (stalled !== !!c.silent) {
    c.silent = stalled;
    render();
  }
  if (stalled) return null;
  const late = (a.outputLatency || 0) + (a.baseLatency || 0);
  const ctxNow = c.ctxTime + (now - c.ctxMoved) / 1000;
  return c.startedFrom + Math.max(0, ctxNow - late - c.startedAt) * 1000;
}

function cutPlay(c) {
  if (!c.map) return;
  if (c.playing) {
    c.playing = false;
    cutSilence(c);
  } else {
    if (c.at >= c.end - 100) c.at = Math.max(0, c.from - CUT_LEAD_MS);
    c.playing = true;
    cutSound(c);
  }
  render();
}

function cutSeek(c, t) {
  c.at = Math.min(c.end, Math.max(0, t));
  if (c.playing && !c.dragging) cutSound(c);
}

// a dragged end stops at the other one, a mark set past it pushes it just far enough
function cutSetEdge(c, which, t, push = false) {
  t = Math.min(c.end, Math.max(0, t));
  if (which === "from") {
    c.from = Math.min(t, push ? c.end - CUT_SHORTEST_MS : c.to - CUT_SHORTEST_MS);
    c.to = Math.max(c.to, c.from + CUT_SHORTEST_MS);
  } else {
    c.to = Math.max(t, push ? CUT_SHORTEST_MS : c.from + CUT_SHORTEST_MS);
    c.from = Math.min(c.from, c.to - CUT_SHORTEST_MS);
  }
  c.made = c.makeError = null;
}

// a note on the line comes in, even a few ms past it
function cutMark(c, which) {
  if (!c.map) return;
  const near = c.notes.filter((n) => Math.abs(n.t - c.at) <= CUT_NEAR_MS);
  const t = near.length ? (which === "from" ? near[0].t : near[near.length - 1].t) : c.at;
  cutSetEdge(c, which, t, true);
  render();
}

function cutIndex(notes, t) {
  let lo = 0;
  let hi = notes.length;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (notes[mid].t < t) lo = mid + 1;
    else hi = mid;
  }
  return lo;
}

const cutCount = (c) => cutIndex(c.notes, c.to + 1e-6) - cutIndex(c.notes, c.from);

function cutSnap(c, t, width) {
  const reach = (CUT_SNAP_PX / width) * c.end;
  const i = cutIndex(c.notes, t);
  const near = [c.notes[i - 1], c.notes[i]].filter((n) => n && Math.abs(n.t - t) <= reach);
  near.sort((a, b) => Math.abs(a.t - t) - Math.abs(b.t - t));
  return near[0]?.t ?? t;
}

function cutLineEvents(c) {
  const line = c.line;
  const box = () => line.getBoundingClientRect();
  const timeAt = (x, r) => (Math.min(1, Math.max(0, (x - r.left) / r.width))) * c.end;
  const grabbed = (x, r) => {
    const at = (t) => r.left + (t / c.end) * r.width;
    const [a, b] = [Math.abs(x - at(c.from)), Math.abs(x - at(c.to))];
    if (Math.min(a, b) > CUT_HANDLE_PX) return null;
    if (Math.abs(a - b) < 1) return x < at(c.from) ? "from" : "to";
    return a < b ? "from" : "to";
  };
  line.addEventListener("pointermove", (ev) => {
    if (c.dragging || !c.map) return;
    line.style.cursor = grabbed(ev.clientX, box()) ? "ew-resize" : "";
  });
  line.addEventListener("pointerdown", (ev) => {
    if (ev.button !== 0 || !c.map) return;
    const r = box();
    const grab = grabbed(ev.clientX, r);
    c.dragging = true;
    cutSilence(c);
    line.setPointerCapture(ev.pointerId);
    const move = (e) => {
      const t = timeAt(e.clientX, r);
      if (!grab) return cutSeek(c, t);
      cutSetEdge(c, grab, cutSnap(c, t, r.width));
      cutSeek(c, grab === "from" ? c.from : c.to - cutScroll(c) * 0.6);
    };
    const up = () => {
      line.removeEventListener("pointermove", move);
      line.removeEventListener("pointerup", up);
      line.removeEventListener("pointercancel", up);
      c.dragging = false;
      if (c.playing) cutSound(c);
      render();
    };
    move(ev);
    line.addEventListener("pointermove", move);
    line.addEventListener("pointerup", up);
    line.addEventListener("pointercancel", up);
  });
}

addEventListener("keydown", (e) => {
  const c = cutter;
  if (!c?.map || e.ctrlKey || e.metaKey || e.altKey || e.target.closest?.("input, textarea, select")) return;
  const step = e.shiftKey ? 10000 : 2000;
  if (e.code === "Space") {
    e.target.closest?.("button")?.blur();
    cutPlay(c);
  } else if (e.code === "KeyI") cutMark(c, "from");
  else if (e.code === "KeyO") cutMark(c, "to");
  else if (e.code === "ArrowLeft") cutSeek(c, c.at - step);
  else if (e.code === "ArrowRight") cutSeek(c, c.at + step);
  else if (e.code === "Home") cutSeek(c, c.from - CUT_LEAD_MS);
  else return;
  e.preventDefault();
});

const cutScroll = (c) => skinSpeedMs(c.speed, c.skin?.scene.ownHit);

function cutTick(c, t) {
  if (cutter !== c) return;
  requestAnimationFrame((n) => cutTick(c, n));
  const dt = c.tickAt == null ? 0 : Math.min(100, t - c.tickAt);
  c.tickAt = t;
  if (c.playing && !c.dragging) {
    const heard = cutHeard(c);
    // never back: until the song is heard after a seek, the notes wait for it
    const at = heard == null ? c.at + dt : Math.max(c.at, heard);
    const until = c.to + CUT_HOLD_MS;
    if (c.loop && c.at <= until && at > until) cutSeek(c, c.from - CUT_LEAD_MS);
    else if (at >= c.end) {
      c.at = c.end;
      c.playing = false;
      cutSilence(c);
      render();
    } else c.at = at;
  }
  if (c.view.isConnected && c.skinTried) cutDraw(c);
  if (c.line.isConnected) cutDrawLine(c);
  cutReadouts(c);
}

const cutClock = (ms) => {
  const s = Math.max(0, ms) / 1000;
  return `${Math.floor(s / 60)}:${(s % 60).toFixed(1).padStart(4, "0")}`;
};

function cutReadouts(c) {
  c.clock.textContent = `${cutClock(c.at)} / ${cutClock(c.end)}`;
  c.fromText.textContent = cutClock(c.from);
  c.toText.textContent = cutClock(c.to);
  const n = cutCount(c);
  c.picked.textContent = `${((c.to - c.from) / 1000).toFixed(1)} s, ${n} ${n === 1 ? "note" : "notes"}`;
}

function cutDraw(c) {
  const view = c.view;
  const ctx = view.getContext("2d");
  const scroll = cutScroll(c);
  const live = c.skin;
  let stage;
  if (live) {
    const s = live.scene;
    const w = Math.round(live.crop.w * live.k);
    if (view.width !== w || view.height !== live.off.height) Object.assign(view, { width: w, height: live.off.height });
    if (!live.chart || live.drawnAt == null || c.at < live.drawnAt || c.at > live.drawnAt + 2000) live.chart = cutChart(c.notes, c.at);
    live.now = live.drawnAt = c.at;
    skinStep(live, scroll);
    ctx.clearRect(0, 0, view.width, view.height);
    ctx.drawImage(live.off, live.crop.x * live.k, 0, w, view.height, 0, 0, w, view.height);
    stage = { k: live.k, hitY: s.hitY, height: s.height, up: s.upscroll };
  } else {
    stage = cutPlain(c, view, ctx, scroll);
  }
  cutShade(c, ctx, view.width, stage, scroll);
}

function cutChart(list, from) {
  const i = cutIndex(list, from);
  const chart = { notes: list.slice(Math.max(0, i - 400), i).filter((n) => n.e != null && n.e >= from), i };
  chart.until = (to) => {
    while (chart.i < list.length && list[chart.i].t < to) chart.notes.push(list[chart.i++]);
  };
  return chart;
}

// 402 is osu!'s default HitPosition on its 480 tall stage
function cutPlain(c, view, ctx, scroll) {
  const keys = c.map.keys;
  const k = Math.max(1, (CUT_DRAW_PX * (devicePixelRatio || 1)) / 480);
  const colW = 30;
  const [w, h, hitY] = [keys * colW + 16, 480, 402];
  if (view.width !== Math.round(w * k)) Object.assign(view, { width: Math.round(w * k), height: Math.round(h * k) });
  ctx.setTransform(k, 0, 0, k, 0, 0);
  ctx.fillStyle = "#0b0b0e";
  ctx.fillRect(0, 0, w, h);
  const speed = hitY / scroll;
  const x = (col) => 8 + col * colW;
  ctx.fillStyle = "rgba(255, 255, 255, 0.05)";
  for (let col = 1; col < keys; col++) ctx.fillRect(x(col), 0, 1, h);
  const i = cutIndex(c.notes, c.at - 10000);
  const pressed = Array(keys).fill(false);
  for (let j = i; j < c.notes.length; j++) {
    const n = c.notes[j];
    if (n.t > c.at + scroll * 1.2) break;
    const colour = keys % 2 === 1 && n.c === (keys - 1) / 2 ? "#ffd84a" : (n.c < keys / 2 ? n.c : keys - 1 - n.c) % 2 === 0 ? "#ffffff" : "#4fa8ff";
    const head = hitY - Math.max(0, n.t - c.at) * speed;
    if (n.e != null) {
      if (n.e < c.at) continue;
      if (n.t <= c.at) pressed[n.c] = true;
      const tail = hitY - (n.e - c.at) * speed;
      ctx.fillStyle = colour;
      ctx.globalAlpha = 0.55;
      ctx.fillRect(x(n.c) + 6, tail, colW - 12, head - tail);
      ctx.globalAlpha = 1;
      ctx.fillRect(x(n.c) + 2, head - 10, colW - 4, 10);
    } else if (n.t >= c.at) {
      ctx.fillStyle = colour;
      ctx.fillRect(x(n.c) + 2, head - 10, colW - 4, 10);
    } else if (c.at - n.t < SKIN_TAP_PRESS_MS) pressed[n.c] = true;
  }
  ctx.fillStyle = "rgba(255, 255, 255, 0.8)";
  ctx.fillRect(8, hitY, keys * colW, 2);
  pressed.forEach((on, col) => {
    ctx.fillStyle = on ? "rgba(255, 255, 255, 0.5)" : "rgba(255, 255, 255, 0.1)";
    ctx.fillRect(x(col) + 3, hitY + 12, colW - 6, 40);
  });
  ctx.setTransform(1, 0, 0, 1, 0, 0);
  return { k, hitY, height: h, up: false };
}

function cutShade(c, ctx, width, { k, hitY, height, up }, scroll) {
  const speed = hitY / scroll;
  const y = (t) => {
    const v = Math.max(-1, Math.min(hitY, hitY - (t - c.at) * speed));
    return (up ? height - v : v) * k;
  };
  const line = up ? (height - hitY) * k : hitY * k;
  const rect = (a, b) => ctx.fillRect(0, Math.min(a, b), width, Math.abs(b - a));
  ctx.fillStyle = "rgba(0, 0, 0, 0.4)";
  if (c.from > c.at) rect(y(c.from), line);
  rect(y(c.to), up ? height * k : 0);
  ctx.strokeStyle = cutInk(c);
  ctx.lineWidth = 2 * k;
  ctx.setLineDash([6 * k, 5 * k]);
  for (const t of [c.from, c.to]) {
    if (t < c.at || t > c.at + hitY / speed) continue;
    ctx.beginPath();
    ctx.moveTo(0, y(t));
    ctx.lineTo(width, y(t));
    ctx.stroke();
  }
  ctx.setLineDash([]);
}

// looked up again now and then for a theme change
function cutInk(c) {
  const now = Date.now();
  if (!c.ink || now - c.inkAt > 1000) {
    c.ink = getComputedStyle(c.line).color;
    c.inkAt = now;
  }
  return c.ink;
}

function cutLineSvg() {
  const node = svg("svg", { class: "cut-line", height: CUT_LINE_PX, "aria-label": "The map, with the part to keep" },
    svg("defs", {}, svg("clipPath", { id: "cut-kept" }, svg("rect", { class: "kept-area", y: 0, height: CUT_LINE_PX }))),
    svg("g", { class: "inked" }, svg("path", { class: "hill" }), svg("rect", { class: "ground", height: 2.5, rx: 1.25 })),
    svg("g", { "clip-path": "url(#cut-kept)" }, svg("path", { class: "hill kept" })),
    ["from", "to"].map((which) => svg("g", { class: `edge ${which}` },
      svg("line", { y1: 2, y2: CUT_LINE_PX - 9 }),
      svg("circle", { cy: CUT_LINE_PX - 6, r: 4.5 }))),
    svg("line", { class: "now", y1: 0, y2: CUT_LINE_PX }));
  return node;
}

function cutScissors() {
  const node = el("span", { class: "cut-scissors" });
  node.innerHTML = `<svg viewBox="0 0 20 28" width="26" height="36" aria-hidden="true"><g class="ink">
    <g class="half">${SCISSORS_HALF}</g>
    <g class="half other"><g transform="translate(20 0) scale(-1 1)">${SCISSORS_HALF}</g></g>
    <circle class="pin" cx="10" cy="15.6" r="1.2"/>
  </g></svg>`;
  return node;
}

// notes a second over a second either side
function cutHill(c, w) {
  const bins = new Float32Array(w);
  for (const n of c.notes) {
    const i = Math.floor((n.t / c.end) * w);
    if (i >= 0 && i < w) bins[i]++;
  }
  const r = Math.max(1, Math.round((1000 / c.end) * w));
  const sums = new Float32Array(w + 1);
  for (let i = 0; i < w; i++) sums[i + 1] = sums[i] + bins[i];
  const level = Array.from({ length: w }, (_, i) => sums[Math.min(w, i + r + 1)] - sums[Math.max(0, i - r)]);
  const top = Math.max(1, ...level);
  const base = CUT_LINE_PX - 3;
  const y = (v) => (base - (v / top) * (base - 8)).toFixed(1);
  let d = `M0 ${base}`;
  for (let i = 0; i < w; i += 2) d += `L${i} ${y(level[i])}`;
  return `${d}L${w} ${base}Z`;
}

function cutDrawLine(c) {
  const line = c.line;
  const w = Math.round(line.clientWidth || line.getBoundingClientRect().width);
  if (!w) return;
  const q = (sel) => line.querySelector(sel);
  if (w !== c.lineW) {
    c.lineW = w;
    c.edgesAt = null;
    line.setAttribute("viewBox", `0 0 ${w} ${CUT_LINE_PX}`);
    const d = cutHill(c, w);
    line.querySelectorAll(".hill").forEach((p) => p.setAttribute("d", d));
    Object.entries({ x: 0, y: CUT_LINE_PX - 3, width: w }).forEach(([k, v]) => q(".ground").setAttribute(k, v));
  }
  const x = (t) => (t / c.end) * w;
  const edges = `${c.from} ${c.to}`;
  if (edges !== c.edgesAt) {
    c.edgesAt = edges;
    const [a, b] = [x(c.from), x(c.to)];
    q(".kept-area").setAttribute("x", a);
    q(".kept-area").setAttribute("width", Math.max(0, b - a));
    for (const [which, at] of [["from", a], ["to", b]]) q(`.edge.${which}`).setAttribute("transform", `translate(${at.toFixed(1)} 0)`);
    c.scissors.style.transform = `translateX(${(a - 13).toFixed(1)}px)`;
  }
  q(".now").setAttribute("transform", `translate(${x(c.at).toFixed(1)} 0)`);
}

async function makeCut(c) {
  if (c.busy || !c.map) return;
  c.busy = true;
  c.made = c.makeError = null;
  render();
  try {
    const q = new URLSearchParams({ from: c.from, to: c.to, name: c.nameField?.value.trim() ?? "" });
    const res = await fetch(`${BRIDGE}/cut/${c.map.md5}?${q}`, { method: "POST" });
    const body = await res.json().catch(() => ({}));
    if (cutter !== c) return;
    if (res.ok) c.made = body;
    else c.makeError = body.error ?? "Couldn't make the map.";
  } catch {
    if (cutter === c) c.makeError = "Lost Hashi's helper.";
  }
  if (cutter !== c) return;
  c.busy = false;
  render();
}

// kept across redraws so a drag isn't cut off
function cutVolumeField(c) {
  const show = (input) => input.style.setProperty("--at", `${input.value}%`);
  c.volumeField ??= el("input", {
    type: "range", class: "skin-seek-bar cut-volume", min: 0, max: 100, step: 1, value: c.volume, "aria-label": "Song volume",
    oninput: (ev) => {
      c.volume = Number(ev.target.value);
      c.gain?.gain.setTargetAtTime(cutGain(c.volume), c.audio.currentTime, 0.015);
      ev.target.title = `Song volume ${c.volume}%`;
      show(ev.target);
    },
    onchange: (ev) => store(CUT_VOLUME, Number(ev.target.value)),
  });
  c.volumeField.title = `Song volume ${c.volume}%`;
  show(c.volumeField);
  return c.volumeField;
}

function cutSpeed(c, by) {
  c.speed = Math.min(SKIN_SPEED_MAX, Math.max(SKIN_SPEED_MIN, c.speed + by));
  store(SKIN_SPEED, c.speed);
  render();
}

// the helper's name when none is given
function cutDefaultName(c) {
  const [i, j] = [cutIndex(c.notes, c.from), cutIndex(c.notes, c.to + 1e-6) - 1];
  if (j < i) return "";
  const at = (ms) => clock(Math.floor(ms / 1000));
  return `Hashi cut ${c.map.version ?? ""} ${at(c.notes[i].t)}-${at(c.notes[j].t)}`;
}

function cutNameField(c) {
  c.nameField ??= el("input", { type: "text", maxlength: 80, spellcheck: "false", "aria-label": "The new difficulty's name" });
  c.nameField.placeholder = cutDefaultName(c);
  return c.nameField;
}

function cutterView() {
  const c = cutter;
  const m = c.map;
  const selected = snapshot.tosu.map;
  const other = m && selected && selected.md5 !== m.md5;
  cutReadouts(c);
  const mark = (label, which, text, key) => el("div", { class: "cut-mark" },
    el("span", { class: "cut-label" }, label),
    text,
    el("button", { class: "key", title: `${label} where the song is now (${key})`, onclick: () => cutMark(c, which) }, "Here"));
  return el("div", { class: "cutter" },
    el("div", { class: "head" },
      el("p", { class: "say" }, "Cut a map"),
      el("button", { class: "link", onclick: closeCutter }, "Back to plays")),
    m && el("div", { class: "map" },
      el("span", { class: "title" }, m.title ?? "Unknown"),
      m.artist && el("span", { class: "artist" }, m.artist),
      el("div", { class: "facts" },
        el("span", {}, diffName(m.keys, m.version)),
        el("span", {}, `${c.notes.length} notes`),
        el("span", {}, clock(c.last / 1000)))),
    other && el("p", { class: "caption cut-other" }, "Another map is selected in osu!. ", el("button", { class: "link", onclick: openCutter }, "Cut that one instead")),
    c.loading && el("p", { class: "doing" }, el("i", { class: "spinner", style: `animation-delay: -${Date.now() % SPIN_MS}ms` }), "Reading the map"),
    c.error && el("p", { class: "status bad cut-error" }, c.error),
    m && [
      el("div", { class: "cut-stage" },
        el("div", { class: "cut-view-box" },
          c.view,
          !c.skinTried && el("i", { class: "spinner", style: `animation-delay: -${Date.now() % SPIN_MS}ms` })),
        el("div", { class: "cut-side" },
          mark("Start", "from", c.fromText, "I"),
          mark("End", "to", c.toText, "O"),
          c.picked,
          el("p", { class: "cut-hint" }, "Drag the dashed lines, or pause on a note and press Here. Space plays, the arrows skip."))),
      el("div", { class: "cut-track" }, c.scissors, c.line),
      el("div", { class: "cut-tools" },
        el("div", { class: "segs" },
          el("button", { class: "seg", onclick: () => cutPlay(c) }, c.playing ? "Pause" : "Play"),
          el("button", {
            class: `seg${c.loop ? " on" : ""}`, "aria-pressed": String(c.loop), title: "Go back to the start after the end",
            onclick: () => {
              c.loop = !c.loop;
              store(CUT_LOOP, c.loop);
              render();
            },
          }, "Loop"),
          el("label", { class: "cut-sound" }, el("span", { "aria-hidden": "true" }, "Volume"), cutVolumeField(c))),
        c.clock,
        el("div", { class: "segs", title: "Scroll speed" },
          el("button", { class: "seg", disabled: c.speed <= SKIN_SPEED_MIN, "aria-label": "Slower", onclick: () => cutSpeed(c, -1) }, "−"),
          el("span", { class: "skin-scroll" }, `Speed ${c.speed}`),
          el("button", { class: "seg", disabled: c.speed >= SKIN_SPEED_MAX, "aria-label": "Faster", onclick: () => cutSpeed(c, 1) }, "+"))),
      (c.silent || c.songState === "none") && el("p", { class: "caption cut-silent" }, NO_SOUND[snapshot.soundHelp] ?? "No sound. Check that your computer has a working audio output."),
      part("cut-make",
        el("p", { class: "caption" }, "A new difficulty with only the notes between the two lines, and the song cut to fit. Plays of it aren't sent to Mania Tracker."),
        el("label", { class: "cut-name" }, el("span", { class: "cut-label" }, "Name"), cutNameField(c)),
        el("button", { class: "primary cut-go", disabled: c.busy || cutCount(c) === 0 || c.to - c.from < CUT_SHORTEST_MS, onclick: () => makeCut(c) }, c.busy ? "Cutting the song…" : "Make it and open in osu!"),
        c.makeError && el("p", { class: "status bad" }, c.makeError),
        c.made && el("p", { class: "status good" }, `Sent to osu! as “${c.made.version}”.`)),
    ]);
}
