// the notes follow the same rules as the helper's skin_render/scene.rs

// scroll speed as osu! counts it, 1 to 40, and the ms a note takes to fall to a judgement line `hit`
// down the 480 tall stage. Lazer keeps the notes' speed whatever HitPosition is, 402 its default
const SKIN_SPEED = "hashi-skin-speed";
const SKIN_SPEED_MIN = 1;
const SKIN_SPEED_MAX = 40;
const skinSpeedMs = (speed, hit) => ((11485 / speed) * (hit ?? 402)) / 402;
// ?skinfreeze shows the still the helper's /skin.png draws, to check one against the other
const SKIN_FREEZE = new URLSearchParams(location.search).has("skinfreeze");
const SKIN_TAP_PRESS_MS = 90;
const SKIN_FRAME_MS = 1000 / 60;
const SKIN_LEAD_MS = 1500;

// the game's shape, as the HUD sits against the screen's right edge and the stage against its left
function skinAspect(page) {
  const size = skinGameSize(page);
  return size.known ? `&aspect=${(size.w / size.h).toFixed(4)}` : "";
}

async function loadSkinScene(page) {
  const edits = page.editsJson ?? skinEditsJson(page);
  const query = `keys=${page.keys}&at=${page.stamp}${edits ? `&edits=${encodeURIComponent(edits)}` : ""}${skinAspect(page)}`;
  const res = await fetch(`${BRIDGE}/skin/scene?${query}`);
  if (!res.ok) throw new Error("scene");
  const scene = await res.json();
  // an edit only changes a few sprites, the rest are the ones already loaded
  const load = `${page.keys} ${page.stamp}`;
  if (page.sprites?.load !== load) page.sprites = { load, byId: new Map() };
  const known = page.sprites.byId;
  const images = await Promise.all(scene.sprites.map(async ([, , id], i) => {
    if (known.has(id)) return known.get(id);
    const img = new Image();
    img.src = `${BRIDGE}/skin/sprite/${i}.png?${query}`;
    try {
      await img.decode();
    } catch {
      return null;
    }
    known.set(id, img);
    return img;
  }));
  return { scene, images };
}

function refreshSkin(page) {
  clearTimeout(page.editTimer);
  page.editTimer = setTimeout(async () => {
    const seq = (page.editSeq = (page.editSeq ?? 0) + 1);
    let loaded = null;
    try {
      loaded = await loadSkinScene(page);
    } catch {}
    if (skinPage !== page || page.editSeq !== seq) return;
    if (!loaded) {
      page.error = "Couldn't draw this skin.";
      render();
      return;
    }
    const live = page.live;
    if (!live || live.scene.columns.length !== loaded.scene.columns.length || live.scene.width !== loaded.scene.width) return startSkinLive(page, loaded);
    Object.assign(live, loaded);
    skinLayers(live);
    if (SKIN_FREEZE) skinFrozen(live);
  }, 120);
}

const skinColor = (c) => `rgba(${c[0]}, ${c[1]}, ${c[2]}, ${c[3] / 255})`;
const skinHex = (c) => `#${c.slice(0, 3).map((x) => Math.round(x).toString(16).padStart(2, "0")).join("")}`;

const skinArtColours = new WeakMap();
function skinArtColour(img) {
  if (!img) return null;
  if (skinArtColours.has(img)) return skinArtColours.get(img);
  let out = null;
  try {
    const c = document.createElement("canvas");
    c.width = 16;
    c.height = 64;
    const x = c.getContext("2d", { willReadFrequently: true });
    x.drawImage(img, 0, 0, 16, 64);
    const px = x.getImageData(0, 0, 16, 64).data;
    const sum = [0, 0, 0];
    let n = 0;
    for (let i = 0; i < px.length; i += 4) {
      if (px[i + 3] < 128) continue;
      for (let k = 0; k < 3; k++) sum[k] += px[i + k];
      n++;
    }
    if (n) out = skinHex(sum.map((v) => v / n));
  } catch {}
  skinArtColours.set(img, out);
  return out;
}

function skinPartColour(page, col, part) {
  const live = page.live;
  const c = live?.scene.columns[col];
  if (!c) return null;
  if (part === "notes") return c.noteColor ? skinHex(c.noteColor) : null;
  const sprite = part === "bodies" ? c.body?.s : (part === "heads" ? c.head : c.tail)?.[0]?.s;
  return sprite == null ? null : skinArtColour(live.images[sprite]);
}

function skinOps(ctx, ops, images) {
  for (const o of ops) {
    if (o.t === "fill") {
      ctx.globalAlpha = o.a;
      ctx.fillStyle = skinColor(o.c);
      ctx.fillRect(...o.r);
      ctx.globalAlpha = 1;
    } else if (o.t === "clear") {
      // only the pixels wholly inside: one it half clears stays half see-through under the fill
      // that follows, a faint line down a column's edge
      const k = ctx.getTransform().a;
      const [x, y, w, h] = o.r;
      const [x0, x1, y0, y1] = [Math.ceil(x * k) / k, Math.floor((x + w) * k) / k, Math.ceil(y * k) / k, Math.floor((y + h) * k) / k];
      if (x1 > x0 && y1 > y0) ctx.clearRect(x0, y0, x1 - x0, y1 - y0);
    } else if (o.t === "poly") {
      ctx.fillStyle = skinColor(o.c);
      ctx.beginPath();
      o.p.forEach(([x, y], i) => (i ? ctx.lineTo(x, y) : ctx.moveTo(x, y)));
      ctx.closePath();
      ctx.fill();
    } else if (o.t === "rrect") {
      const [x, y, w, h] = o.r;
      const r = Math.min(o.rad, w / 2, h / 2);
      ctx.fillStyle = skinColor(o.c);
      ctx.beginPath();
      ctx.moveTo(x + r, y);
      ctx.arcTo(x + w, y, x + w, y + h, r);
      ctx.arcTo(x + w, y + h, x, y + h, r);
      ctx.arcTo(x, y + h, x, y, r);
      ctx.arcTo(x, y, x + w, y, r);
      ctx.closePath();
      ctx.fill();
    } else {
      const img = images[o.s];
      if (!img) continue;
      ctx.save();
      if (o.clip) {
        ctx.beginPath();
        ctx.rect(...o.clip);
        ctx.clip();
      }
      if (o.flip != null) {
        ctx.translate(0, o.flip);
        ctx.scale(1, -1);
      }
      if (o.add) ctx.globalCompositeOperation = "lighter";
      ctx.drawImage(img, ...o.src, ...o.dst);
      ctx.restore();
    }
  }
}

// stable starts a note's animation when it spawns, at 60 fps whatever the skin says
function skinFrame(frames, time) {
  if (frames.length < 2) return frames[0]?.s != null ? frames[0] : null;
  const n = frames.length;
  const f = frames[((Math.floor(-time / SKIN_FRAME_MS) % n) + n) % n];
  return f.s != null ? f : null;
}

const skinMapY = (s, y) => (s.upscroll ? s.height - y : y);

function skinTap(s, column, y, time, out) {
  const c = s.columns[column];
  const anchor = skinMapY(s, y);
  const f = skinFrame(c.tap, time);
  if (f) {
    const top = s.upscroll ? anchor : anchor - f.h;
    out.push({ t: "img", s: f.s, src: [0, 0, ...s.sprites[f.s].slice(0, 2)], dst: [c.x, top, c.w, f.h], flip: s.upscroll ? top * 2 + f.h : null });
    return;
  }
  const h = Math.max(10, c.w * 0.3);
  out.push({ t: "rrect", r: [c.x + 1, s.upscroll ? anchor : anchor - h, c.w - 2, h], rad: 4, c: c.tapColor });
}

// whole pixels, two half-covered rows over each other show as a seam at every join. A body cut to a
// texture's limit, `rows` of its `full`, has its last row drawn on over the rest, as the game
// repeats the whole image
function skinTiles(top, bottom, rows, scale, full = rows) {
  const tiles = [];
  if (!(scale > 0 && rows > 0 && bottom > top)) return tiles;
  const rest = Math.max(0, full - rows);
  const push = (y, n, srcY, srcRows) => {
    const t = Math.round(y);
    tiles.push({ top: t, height: Math.max(1, Math.round(y + n * scale) - t), srcY, srcRows });
  };
  let y = top;
  while (y < bottom - 0.01 && tiles.length < 4096) {
    const n = Math.min(rows, (bottom - y) / scale);
    push(y, n, 0, n);
    y += n * scale;
    if (rest > 0 && y < bottom - 0.01) {
      const m = Math.min(rest, (bottom - y) / scale);
      push(y, m, rows - 1, 1);
      y += m * scale;
    }
  }
  return tiles;
}

function skinHold(s, column, headY, tailY, time, out) {
  const c = s.columns[column];
  const up = s.upscroll;
  const { x, w } = c;
  const head = skinFrame(c.head, time) ?? skinFrame(c.tap, time);
  const tail = skinFrame(c.tail, time);
  const headEnd = skinMapY(s, headY);
  const tailEnd = skinMapY(s, tailY);
  const headH = head ? head.h : Math.max(10, w * 0.3);
  // a blank tail has no box, so the body runs right to the end line
  const artTop = tail?.top ?? null;
  const tailH = tail && artTop != null ? tail.h : 0;
  const headTop = up ? headEnd : headEnd - headH;
  const tailTop = up ? tailEnd : tailEnd - tailH;
  const headSide = up ? headEnd + headH / 2 : headEnd - headH / 2;
  const tailSide = up ? tailTop + tailH : tailTop;
  let bodyTop = up ? headSide : tailSide;
  let bodyBottom = up ? tailSide : headSide;
  if (s.lazerBodies) {
    const half = (tail ? tail.h : headH) / 2;
    if (up) bodyBottom = tailEnd + half;
    else bodyTop = tailEnd - half;
  }
  let visibleTop = bodyTop;
  let visibleBottom = bodyBottom;
  if (tailH > 0 && artTop != null && !s.lazerBodies) {
    const centre = tailTop + tailH / 2;
    const edge = tailTop + (up ? artTop : 1 - artTop) * tailH;
    if (up) visibleBottom = Math.min(visibleBottom, Math.max(centre, edge + 1));
    else visibleTop = Math.max(visibleTop, Math.min(centre, edge - 1));
  }
  if (visibleBottom > visibleTop) {
    const b = c.body;
    if (b) {
      const fromBottom = b.style === 2 && !s.lazerBodies ? !up : up;
      const flip = fromBottom ? bodyTop + bodyBottom : null;
      const clip = [x, visibleTop, w, visibleBottom - visibleTop];
      if (b.style === 0) {
        out.push({ t: "img", s: b.s, src: [0, 0, b.sw, b.sh], dst: [x, bodyTop, w, bodyBottom - bodyTop], flip, clip });
      } else {
        for (const t of skinTiles(bodyTop, bodyBottom, b.sh, b.row, b.full ?? b.sh)) {
          out.push({ t: "img", s: b.s, src: [0, t.srcY, b.sw, t.srcRows], dst: [x, t.top, w, t.height], flip, clip });
        }
      }
    } else {
      out.push({ t: "fill", r: [x + w * 0.16, visibleTop, w * 0.68, visibleBottom - visibleTop], c: s.bodyColor, a: 0.9 });
    }
  }
  if (tail && tailH > 0) {
    // lazer masks the hold at the head's centre
    const clip = s.lazerBodies ? (up ? [x, headSide, w, s.height - headSide] : [x, 0, w, headSide]) : null;
    out.push({ t: "img", s: tail.s, src: [0, 0, ...s.sprites[tail.s].slice(0, 2)], dst: [x, tailTop, w, tailH], flip: up ? null : tailTop * 2 + tailH, clip });
  }
  if (head) out.push({ t: "img", s: head.s, src: [0, 0, ...s.sprites[head.s].slice(0, 2)], dst: [x, headTop, w, headH], flip: up ? headTop * 2 + headH : null });
  else out.push({ t: "rrect", r: [x + 1, headTop, w - 2, headH], rad: 4, c: c.headColor });
}

// k is screen pixels per scene unit
function skinLayers(live) {
  const { scene: s, images, k } = live;
  const layer = (ops) => {
    const canvas = document.createElement("canvas");
    canvas.width = Math.round(s.width * k);
    canvas.height = Math.round(s.height * k);
    const ctx = canvas.getContext("2d");
    ctx.scale(k, k);
    skinOps(ctx, ops, images);
    return canvas;
  };
  live.layers = { base: layer(s.base), hint: layer(s.hint), line: layer(s.line), over: layer(s.over), top: layer(s.top) };
}

function skinDraw(live, holds, taps, pressed) {
  const { scene: s, images, ctx, layers, k } = live;
  const notes = [];
  for (const h of holds) skinHold(s, h.c, h.head, h.tail, h.t, notes);
  for (const t of taps) skinTap(s, t.c, t.y, t.t, notes);
  const keys = () => s.columns.forEach((c, i) => skinOps(ctx, pressed[i] ? c.keyDown : c.keyUp, images));
  const stack = (layer) => ctx.drawImage(layer, 0, 0, s.width, s.height);
  ctx.setTransform(k, 0, 0, k, 0, 0);
  ctx.clearRect(0, 0, s.width, s.height);
  stack(layers.base);
  s.columns.forEach((c, i) => pressed[i] && skinOps(ctx, c.light, images));
  stack(layers.hint);
  if (s.keysUnderNotes) keys();
  skinOps(ctx, notes, images);
  stack(layers.line);
  if (!s.keysUnderNotes) keys();
  stack(layers.over);
  s.columns.forEach((c, i) => pressed[i] && skinOps(ctx, c.lighting, images));
  stack(layers.top);
}

function skinFrozen(live) {
  const { frozen } = live.scene;
  live.scene.columns.forEach((_, i) => (live.pressed[i] = i === frozen.pressed));
  skinDraw(live, frozen.holds, frozen.taps, live.pressed);
}

function skinRandom(seed) {
  let state = seed >>> 0;
  return () => {
    state = (state + 0x6d2b79f5) >>> 0;
    let t = state;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

// seeded per keymode, so it's the same chart every time
function skinChart(keys, holdsOnly = false, start = 600) {
  const random = skinRandom(keys * 7919 + 7);
  const beat = 60000 / 170;
  const chart = { notes: [], next: start, free: Array(keys).fill(-Infinity), last: [], div: 2, sectionEnd: 0 };
  const pick = (t, n, avoid = []) => {
    const open = [...Array(keys).keys()].filter((c) => chart.free[c] <= t && !avoid.includes(c));
    const out = [];
    while (out.length < n && open.length) out.push(open.splice(Math.floor(random() * open.length), 1)[0]);
    return out;
  };
  chart.until = (to) => {
    while (chart.next < to) {
      const t = chart.next;
      if (t >= chart.sectionEnd) {
        chart.div = random() < 0.3 ? 4 : 2;
        chart.sectionEnd = t + beat * (chart.div === 4 ? 4 : 8);
      }
      const step = beat / chart.div;
      const r = random();
      let columns = [];
      let hold = 0;
      if (holdsOnly) {
        columns = r < 0.3 ? [] : pick(t, random() < 0.3 ? 2 : 1, chart.last);
        hold = beat * (0.5 + Math.floor(random() * 7) / 2);
      } else if (r < 0.08) {
        columns = [];
      } else if (r < 0.22 && chart.div === 2) {
        columns = pick(t, random() < 0.25 ? 2 : 1);
        hold = beat * (1 + Math.floor(random() * 3));
      } else if (r < 0.42) {
        columns = pick(t, keys >= 6 && random() < 0.35 ? 3 : 2, chart.last);
      } else if (r < 0.55 && chart.last.length) {
        columns = chart.last.filter((c) => chart.free[c] <= t);
      } else {
        columns = pick(t, 1, chart.last);
      }
      for (const c of columns) {
        chart.notes.push({ c, t, e: hold ? t + hold : null });
        chart.free[c] = t + (hold ? hold + step : step * 0.9);
      }
      chart.last = columns;
      chart.next = t + step;
    }
  };
  return chart;
}

// a real map from `from` (map ms) to its end, then again from its start, 1.5 s apart.
// `from` plays at `start` on the preview's clock
function skinMapChart(notes, from, start = SKIN_LEAD_MS) {
  const list = notes.map(([c, t, e]) => ({ c, t, e })).sort((a, b) => a.t - b.t);
  const end = Math.max(...list.map((n) => n.e ?? n.t));
  const chart = { notes: [], i: Math.max(0, list.findIndex((n) => n.t >= from)), base: start - from };
  const [first, period, zero] = [list[0].t, end + SKIN_LEAD_MS - list[0].t, start - from];
  Object.assign(chart, { first, end });
  chart.at = (now) => {
    const t = now - zero;
    return t > first ? first + ((t - first) % period) : t;
  };
  chart.until = (to) => {
    for (;;) {
      if (chart.i >= list.length) {
        chart.base += end + SKIN_LEAD_MS - list[0].t;
        chart.i = 0;
      }
      const n = list[chart.i];
      if (n.t + chart.base >= to) return;
      chart.notes.push({ c: n.c, t: n.t + chart.base, e: n.e == null ? null : n.e + chart.base });
      chart.i++;
    }
  };
  return chart;
}

function skinSeek(page, t) {
  if (!page.live || !page.map) return;
  page.map.from = t;
  page.live.chart = skinMapChart(page.map.notes, t, page.live.now);
}

// kept across redraws so a drag isn't cut off
function skinSeekBar(page, done) {
  page.seek ??= {
    input: el("input", {
      type: "range", class: "skin-seek-bar", step: 10, "aria-label": "Where in the map",
      oninput: (ev) => {
        page.seek.dragging = true;
        skinSeek(page, Number(ev.target.value));
        skinSeekShow(page);
      },
      onchange: (ev) => {
        page.seek.dragging = false;
        done(Number(ev.target.value));
      },
    }),
    time: el("span", { class: "skin-seek-time" }),
  };
  page.seek.box ??= el("div", { class: "skin-seek" }, page.seek.input, page.seek.time);
  skinSeekShow(page);
  return page.live?.chart?.at && page.map ? page.seek.box : null;
}

// `at` is a time in the map from elsewhere: the game size window's, while it plays instead
function skinSeekShow(page, at) {
  const chart = page.live?.chart;
  if (!page.seek) return;
  page.seek.box.hidden = !page.map || !chart?.at;
  if (page.seek.box.hidden) return;
  at = Math.min(chart.end, Math.max(chart.first, at ?? (skinAway.open ? skinAway.at : null) ?? chart.at(page.live.now)));
  const { input } = page.seek;
  input.min = chart.first;
  input.max = chart.end;
  if (!page.seek.dragging) input.value = at;
  input.style.setProperty("--at", `${((Number(input.value) - chart.first) / Math.max(1, chart.end - chart.first)) * 100}%`);
  page.seek.time.textContent = `${clock(at / 1000)} / ${clock(chart.end / 1000)}`;
}

// new notes start just above the screen, so the ones already on it play out
function swapSkinChart(page) {
  const live = page.live;
  const s = live.scene;
  const start = live.now + (s.height * 1.5) / (s.hitY / page.scroll);
  const carried = live.chart.notes.filter((n) => n.t < start);
  const chart = page.map ? skinMapChart(page.map.notes, page.map.from, start) : skinChart(page.keys, page.source === "lns", start);
  // nothing new lands on a column until its carried notes have cleared it
  const busy = Array(s.columns.length).fill(-Infinity);
  for (const n of carried) busy[n.c] = Math.max(busy[n.c], (n.e ?? n.t) + 100);
  const until = chart.until;
  chart.notes = carried;
  chart.until = (to) => {
    const from = chart.notes.length;
    until(to);
    const added = chart.notes.splice(from).filter((n) => n.t >= busy[n.c]);
    chart.notes.push(...added);
  };
  live.chart = chart;
}

function startSkinLive(page, loaded) {
  const chart = page.map ? skinMapChart(page.map.notes, page.map.from) : skinChart(page.keys, page.source === "lns");
  const { width, height } = loaded.scene;
  if (page.canvas.width !== width || page.canvas.height !== height) Object.assign(page.canvas, { width, height });
  page.canvas.style.aspectRatio = `${width} / ${height}`;
  const live = { ...loaded, ctx: page.canvas.getContext("2d"), k: 1, pressed: [], chart, now: 0, last: null };
  skinLayers(live);
  page.live = live;
  (page.hitUnits ??= {})[page.keys] = loaded.scene.ownHit;
  if (!SKIN_FREEZE) {
    loadSkinTiming(page);
    loadSkinImages(page);
  }
  if (SKIN_FREEZE) {
    skinFrozen(live);
    return;
  }
  requestAnimationFrame((at) => skinTick(page, live, at));
}

// the game size window says it's open twice a second, with where it is in the map, and when it's
// clicked to pause or seeked. a silence means it's gone
const skinAway = { open: false, timer: 0, waiting: [], at: null };
skinAway.set = (open) => {
  if (open === skinAway.open) return;
  skinAway.open = open;
  if (!open) {
    skinAway.at = null;
    skinAway.waiting.splice(0).forEach((wake) => wake());
  }
};

function skinPlayerNews(news) {
  if (skinPage && news.pause) skinPause(skinPage);
  if (skinPage && news.seek != null) {
    skinSeek(skinPage, news.seek);
    render();
  }
  skinAway.at = news.at ?? news.seek ?? skinAway.at;
  if (skinPage && news.at != null) skinSeekShow(skinPage, news.at);
  clearTimeout(skinAway.timer);
  if (news.open) skinAway.timer = setTimeout(() => skinAway.set(false), 2500);
  skinAway.set(!!news.open);
}

function skinPause(page) {
  page.playing = !page.playing;
  render();
}

function skinTick(page, live, at) {
  if (skinPage !== page || page.live !== live) return;
  const away = skinAway.open;
  if (document.hidden || away) {
    live.last = live.fpsFrom = live.tick = null;
    live.frames = 0;
    const wake = () => requestAnimationFrame((t) => skinTick(page, live, t));
    if (away) {
      const show = (on) => {
        page.canvas.classList.toggle("away", on);
        page.fpsNode?.classList.toggle("away", on);
        if (page.fpsNode) page.fpsNode.textContent = on ? "In its own window" : "";
      };
      show(true);
      skinAway.waiting.push(() => {
        show(false);
        wake();
      });
    } else document.addEventListener("visibilitychange", wake, { once: true });
    return;
  }
  // the page's preview draws at about 60 fps on any screen, on whole refreshes so it stays even
  const refresh = live.tick == null ? 0 : at - live.tick;
  live.tick = at;
  if (live.last != null && at - live.last + refresh / 2 < 1000 / 60) {
    requestAnimationFrame((t) => skinTick(page, live, t));
    return;
  }
  if (page.playing && live.last != null) live.now += Math.min(at - live.last, 100);
  live.last = at;
  live.frames = (live.frames ?? 0) + 1;
  live.fpsFrom ??= at;
  if (at - live.fpsFrom >= 500) {
    if (page.fpsNode) page.fpsNode.textContent = `${Math.round((live.frames * 1000) / (at - live.fpsFrom))} fps`;
    live.frames = 0;
    live.fpsFrom = at;
  }
  skinStep(live, page.scroll);
  skinSeekShow(page);
  requestAnimationFrame((t) => skinTick(page, live, t));
}

function skinStep(live, scroll) {
  const s = live.scene;
  const { now, chart } = live;
  const speed = s.hitY / scroll;
  // far enough ahead for a tail cap or an upscroll note entering past the edge
  chart.until(now + (s.height * 1.5) / speed);
  chart.notes = chart.notes.filter((n) => (n.e ?? n.t + SKIN_TAP_PRESS_MS) >= now);
  const pressed = live.pressed;
  s.columns.forEach((_, i) => (pressed[i] = false));
  const holds = [];
  const taps = [];
  for (const n of chart.notes) {
    if (n.e != null) {
      if (n.t <= now) pressed[n.c] = true;
      // a hold being held sticks to the line while its body drains through it
      holds.push({ c: n.c, head: s.hitY - Math.max(0, n.t - now) * speed, tail: s.hitY - (n.e - now) * speed, t: n.t - now });
    } else if (n.t <= now) {
      pressed[n.c] = true;
    } else {
      taps.push({ c: n.c, y: s.hitY - (n.t - now) * speed, t: n.t - now });
    }
  }
  taps.sort((a, b) => a.y - b.y);
  skinDraw(live, holds, taps, pressed);
}

const SKIN_CUT_STEP = 5;
const SKIN_FADE_STEP = 20;
const SKIN_PALETTE = ["#ffffff", "#b8b8c4", "#ff4d6a", "#ff9a3c", "#ffd84a", "#9be564", "#3ddc97", "#38d6e8", "#4fa8ff", "#6a6cff", "#a970ff", "#ff6ad5"];
// stable's default column layout: 1 and 2 mirrored from the edges in, S in the middle of odd keymodes
const SKIN_CLASSIC = { 1: "#ffffff", 2: "#4fa8ff", S: "#ffd84a" };
const SKIN_PARTS = [["notes", "Notes"], ["heads", "LN Heads"], ["bodies", "LN Bodies"], ["tails", "LN Tails"]];

function skinEdit(page) {
  page.edits ??= {};
  return (page.edits[page.keys] ??= skinFresh());
}

function skinFresh() {
  return {
    cut: null, fade: null, colors: { notes: [], heads: [], bodies: [], tails: [] },
    parts: { notes: true, heads: true, bodies: true, tails: true }, picks: [],
    swap: {}, swapPart: "note", swapCols: [],
    // skin.ini's values, null keeping the skin's
    hit: null, line: null, start: null, score: null, combo: null, colWidth: null, colSpacing: null,
    // how far up the receptors' and StageBottom's art move in their images, in 480ths
    raise: 0, bottom: 0,
    // percent
    comboScale: null,
    // by part, per column: { width: percent, outline: { size, color, outside }, rim: "keep" | "remove" }
    shapes: {},
  };
}

// a note covers the field down to the judgement line in the scroll time, so a time is a length
const skinHit = (page) => page.edits?.[page.keys]?.hit ?? page.hitUnits?.[page.keys];
const skinUnits = (page, ms) => (ms * skinHit(page)) / page.scroll;
const skinMsOf = (page, units) => (units * page.scroll) / skinHit(page);

function skinEditsJson(page) {
  const e = page.edits?.[page.keys];
  if (!e || !page.hitUnits?.[page.keys]) return "";
  const keys = page.keys;
  const out = {};
  if (e.hit != null) out.hitPosition = e.hit;
  if (e.raise) out.keysRaise = e.raise;
  if (e.bottom) out.bottomRaise = e.bottom;
  if (e.line != null) out.judgementLine = e.line;
  if (e.score != null) out.scorePosition = e.score;
  if (e.combo != null) out.comboPosition = e.combo;
  if (e.start != null) out.columnStart = e.start;
  if (e.colWidth != null) out.columnWidth = e.colWidth;
  if (e.colSpacing != null) out.columnSpacing = e.colSpacing;
  if (e.comboScale != null) out.comboScale = e.comboScale / 100;
  const shaped = Object.entries(e.shapes ?? {}).filter(([, cols]) => cols.some(Boolean));
  if (shaped.length) {
    out.shapes = Array.from({ length: keys }, (_, c) => {
      const one = Object.fromEntries(shaped.filter(([, cols]) => cols[c])
        .map(([part, cols]) => [part, { width: cols[c].width == null ? null : cols[c].width / 100, outline: cols[c].outline ?? null, rim: cols[c].rim ?? null }]));
      return Object.keys(one).length ? one : null;
    });
  }
  if (e.cut) out.cut = e.cut.map((ms) => (ms == null ? null : Math.round(skinUnits(page, ms) * 100) / 100));
  if (e.fade) out.fade = { length: Math.round(skinUnits(page, e.fade.ms) * 100) / 100, from: e.fade.from, round: !!e.fade.round };
  const coloured = SKIN_PARTS.map(([p]) => p).filter((p) => e.colors[p]?.some(Boolean));
  if (coloured.length) out.colors = Object.fromEntries(coloured.map((p) => [p, Array.from({ length: keys }, (_, c) => e.colors[p][c] ?? null)]));
  const swapped = Object.entries(e.swap ?? {}).filter(([, cols]) => cols.some(Boolean));
  if (swapped.length) {
    out.swaps = Array.from({ length: keys }, (_, c) => {
      const one = Object.fromEntries(swapped.filter(([, cols]) => cols[c]).map(([part, cols]) => [part, cols[c]]));
      return Object.keys(one).length ? one : null;
    });
  }
  // the accuracy's place is the whole skin's, so it goes with whichever keymode is applied
  if (page.wide?.percentWidth != null) out.percentWidth = page.wide.percentWidth;
  return Object.keys(out).length ? JSON.stringify(out) : "";
}

// the skin's own cut per column in ms at the preview's scroll time, null for no body art
function skinCutNow(page) {
  return page.live.scene.cutNow.map((u) => (u == null ? null : Math.round(skinMsOf(page, u))));
}

async function loadSkinTiming(page) {
  const keys = page.keys;
  if (page.timing?.keys === keys) return;
  page.timing = { keys, data: null };
  const play = page.play ? `&play=${encodeURIComponent(page.play.id)}` : "";
  try {
    page.timing.data = await (await fetch(`${BRIDGE}/skin/timing?keys=${keys}${play}`)).json();
  } catch {}
  if (skinPage === page) render();
}

const skinLate = (ms) => (Math.abs(ms) < 0.5 ? "on time" : `${Math.round(Math.abs(ms))} ms ${ms > 0 ? "late" : "early"}`);

// Late releases mean the visible end reached the line too soon for this player's habit, so the cut
// grows by as much; early ones shrink it, down to none
function skinSuggestion(page) {
  const t = page.timing?.keys === page.keys ? page.timing.data : null;
  if (!t) return null;
  const now = skinCutNow(page);
  if (!now.some((v) => v != null)) return { text: "This skin has no long note body to cut." };
  const where = t.fromPlay ? "in this play" : `over your last ${t.plays} ${page.keys}K ${t.plays === 1 ? "play" : "plays"}`;
  if (!t.n) return { text: t.fromPlay ? "No long notes were held in this play." : `No long notes held in your recent ${page.keys}K plays.` };
  const sure = t.columns.filter((c) => c.n >= 20);
  const spread = sure.length > 1 ? Math.max(...sure.map((c) => c.mean)) - Math.min(...sure.map((c) => c.mean)) : 0;
  const per = spread >= 15 && sure.length === t.columns.length;
  if (!per && Math.abs(t.mean) < 5) return { text: `You let go of long notes ${skinLate(t.mean)} on average ${where}, so the gap suits you.` };
  const cut = now.map((v, c) => (v == null ? null : Math.max(0, Math.round(v + (per ? t.columns[c].mean : t.mean)))));
  const text = per
    ? `Your columns let go at different times ${where}. A gap per column of ${skinRange(cut)} would match.`
    : `You let go ${skinLate(t.mean)} on average ${where}. A gap of ${skinRange(cut)} would match.`;
  const detail = per ? `Per column: ${t.columns.map((c, i) => `${i + 1}: ${skinLate(c.mean)}`).join(", ")}` : null;
  return { text, detail, cut };
}

const skinRange = (cuts) => {
  const set = cuts.filter((v) => v != null);
  const [lo, hi] = [Math.min(...set), Math.max(...set)];
  return lo === hi ? `${lo} ms` : `${lo}–${hi} ms`;
};

const skinCutText = (cuts) => {
  const set = cuts.filter((v) => v != null);
  if (!set.length) return "none";
  return set.every((v) => v === set[0]) ? `${set[0]} ms` : cuts.map((v) => (v == null ? "–" : v)).join(" / ") + " ms";
};

// edits applied before colors went per part kept one per column, on the parts that were on
function skinRestored(state) {
  const e = { ...skinFresh(), ...state };
  if (Array.isArray(e.colors)) e.colors = Object.fromEntries(SKIN_PARTS.map(([p]) => [p, e.parts?.[p] === false ? [] : [...e.colors]]));
  return e;
}

// which columns and parts are picked isn't an edit, going back leaves it as it is
const SKIN_PICKS = ["picks", "parts", "swapPart", "swapCols"];
const SKIN_HISTORY_KEPT = 100;

const skinHistory = (page) => ((page.history ??= {})[page.keys] ??= { undo: [], redo: [] });

function skinChange(page, change) {
  const e = skinEdit(page);
  const before = JSON.stringify(e);
  change(e);
  if (JSON.stringify(e) !== before) {
    const h = skinHistory(page);
    h.undo.push(before);
    if (h.undo.length > SKIN_HISTORY_KEPT) h.undo.shift();
    h.redo = [];
  }
  page.saved = null;
  refreshSkin(page);
  render();
}

function skinUndo(page, back) {
  const h = skinHistory(page);
  const [from, to] = back ? [h.undo, h.redo] : [h.redo, h.undo];
  if (!from.length) return;
  const e = skinEdit(page);
  to.push(JSON.stringify(e));
  const restored = JSON.parse(from.pop());
  for (const k of SKIN_PICKS) restored[k] = e[k];
  page.edits[page.keys] = restored;
  page.saved = null;
  refreshSkin(page);
  render();
}

// a field being typed in keeps its own undo
document.addEventListener("keydown", (ev) => {
  if (!skinPage || !skinPage.live || !(ev.ctrlKey || ev.metaKey) || ev.altKey) return;
  if (ev.target.closest?.("input, textarea")) return;
  const key = ev.key.toLowerCase();
  const back = key === "z" && !ev.shiftKey;
  if (!back && key !== "y" && !(key === "z" && ev.shiftKey)) return;
  ev.preventDefault();
  skinUndo(skinPage, back);
});

// kept across redraws so typing isn't cut off, set on Enter or on leaving it. blank, saying
// "mixed", when there isn't one number
function skinField(page, key, value, min, max, set) {
  const input = ((page.fields ??= {})[key] ??= el("input", {
    class: "skin-input", type: "text", inputmode: "numeric", spellcheck: "false", "aria-label": key,
    onkeydown: (ev) => ev.key === "Enter" && ev.target.blur(),
    onchange: (ev) => input.commit(ev.target.value),
  }));
  input.commit = (raw) => {
    const v = Math.round(Number(raw.trim()));
    if (!raw.trim() || !Number.isFinite(v)) return render();
    set(Math.min(max, Math.max(min, v)));
  };
  input.placeholder = value == null ? "mixed" : "";
  if (hasFocus(input)) {
    // a redraw takes it out of the page, and its focus with it
    const [from, to] = [input.selectionStart, input.selectionEnd];
    requestAnimationFrame(() => {
      if (!input.isConnected || document.activeElement === input) return;
      input.focus();
      input.setSelectionRange(from, to);
    });
  } else input.value = value ?? "";
  return input;
}

function skinStepper(label, text, down, up, onDown, onUp) {
  return el("div", { class: "segs", title: label },
    el("button", { class: "seg", disabled: !down, "aria-label": `Less ${label.toLowerCase()}`, onclick: onDown }, "−"),
    el("span", { class: "skin-scroll" }, text),
    el("button", { class: "seg", disabled: !up, "aria-label": `More ${label.toLowerCase()}`, onclick: onUp }, "+"));
}

function skinLongNotes(page) {
  const e = skinEdit(page);
  const now = skinCutNow(page);
  if (!now.some((v) => v != null)) return part("skin-lns", null, el("p", { class: "caption" }, "This skin has no long note body to change."));
  const cut = e.cut ?? now;
  const px = (ms) => Math.round(skinUnits(page, ms));
  const shown = cut.filter((v) => v != null);
  const same = shown.every((v) => v === shown[0]);
  const step = (d) => skinChange(page, (x) => (x.cut = (x.cut ?? now).map((v) => (v == null ? null : Math.max(0, v + d)))));
  const setCut = (ms) => skinChange(page, (x) => (x.cut = now.map((v) => (v == null ? null : ms))));
  const hint = skinSuggestion(page);
  const fade = e.fade;
  return part("skin-lns", null,
    el("p", { class: "caption" }, `Body gap: how far before the tail a long note's body stops. This skin has ${skinCutText(now)}.`),
    el("div", { class: "skin-tools" },
      el("div", { class: "segs", title: "Body gap" },
        el("button", { class: "seg", disabled: Math.max(...shown) <= 0, "aria-label": "Smaller gap", onclick: () => step(-SKIN_CUT_STEP) }, "−"),
        skinField(page, "Body gap", same ? shown[0] : null, 0, 1000, setCut),
        el("span", { class: "skin-unit" }, same ? `ms · ${px(shown[0])} px` : "ms"),
        el("button", { class: "seg", "aria-label": "Bigger gap", onclick: () => step(SKIN_CUT_STEP) }, "+")),
      e.cut && el("button", { class: "link", onclick: () => skinChange(page, (x) => (x.cut = null)) }, "Keep the skin's")),
    !same && el("p", { class: "caption" }, `Per column: ${skinCutText(cut)}. A number here sets them all.`),
    hint && el("p", { class: "caption", title: hint.detail }, hint.text, " ",
      hint.cut && el("button", { class: "link", onclick: () => skinChange(page, (x) => (x.cut = hint.cut)) }, "Use this")),
    el("div", { class: "skin-tools" },
      segs([["off", "No fade"], ["on", "Fade"]], fade ? "on" : "off", (v) => skinChange(page, (x) => (x.fade = v === "on" ? { ms: 120, from: 0 } : null)), "Fade"),
      fade && el("div", { class: "segs", title: "Fade length" },
        el("button", { class: "seg", disabled: fade.ms <= SKIN_FADE_STEP, "aria-label": "Shorter fade", onclick: () => skinChange(page, (x) => (x.fade.ms -= SKIN_FADE_STEP)) }, "−"),
        skinField(page, "Fade length", fade.ms, SKIN_FADE_STEP, 600, (ms) => skinChange(page, (x) => (x.fade.ms = ms))),
        el("span", { class: "skin-unit" }, "ms"),
        el("button", { class: "seg", disabled: fade.ms >= 600, "aria-label": "Longer fade", onclick: () => skinChange(page, (x) => (x.fade.ms += SKIN_FADE_STEP)) }, "+")),
      fade && skinStepper("Fade start", `from ${Math.round(fade.from * 100)}%`, fade.from > 0, fade.from < 1,
        () => skinChange(page, (x) => (x.fade.from = Math.round((x.fade.from - 0.1) * 10) / 10)), () => skinChange(page, (x) => (x.fade.from = Math.round((x.fade.from + 0.1) * 10) / 10))),
      fade && segs([[false, "Flat end"], [true, "Round end"]], !!fade.round, (v) => skinChange(page, (x) => (x.fade.round = v)), "The shape of the body's end")));
}

const SKIN_WHEEL = 150;

function hsvToHex(h, sat, v) {
  const f = (n) => {
    const k = (n + h / 60) % 6;
    return Math.round(255 * (v - v * sat * Math.max(0, Math.min(k, 4 - k, 1))));
  };
  return `#${[f(5), f(3), f(1)].map((x) => x.toString(16).padStart(2, "0")).join("")}`;
}

function hexToHsv(hex) {
  const m = /^#?([0-9a-f]{6})$/i.exec(hex ?? "");
  if (!m) return null;
  const [r, g, b] = [0, 2, 4].map((i) => parseInt(m[1].slice(i, i + 2), 16) / 255);
  const max = Math.max(r, g, b);
  const d = max - Math.min(r, g, b);
  const h = d === 0 ? 0 : max === r ? ((g - b) / d + 6) % 6 : max === g ? (b - r) / d + 2 : (r - g) / d + 4;
  return { h: h * 60, s: max ? d / max : 0, v: max };
}

// built once per page and kept, a redraw mid-drag would drop the pointer; the preview follows on release
function skinWheel(page) {
  if (page.wheel) return page.wheel;
  const dpr = devicePixelRatio || 1;
  const size = SKIN_WHEEL;
  const wheel = el("canvas", { class: "skin-wheel", width: size * dpr, height: size * dpr, style: `width: ${size}px; height: ${size}px` });
  const bar = el("canvas", { class: "skin-bar", width: size * dpr, height: 14 * dpr, style: `width: ${size}px; height: 14px` });
  const swatch = el("span", { class: "skin-swatch big" });
  const hex = el("input", { class: "skin-hex", maxlength: 7, spellcheck: "false", "aria-label": "Hex color" });
  const state = { h: 0, s: 0, v: 1 };
  const base = (() => {
    const c = document.createElement("canvas");
    c.width = c.height = size * dpr;
    const ctx = c.getContext("2d");
    const img = ctx.createImageData(c.width, c.height);
    const r = c.width / 2;
    for (let y = 0; y < c.height; y++) {
      for (let x = 0; x < c.width; x++) {
        const dx = x + 0.5 - r;
        const dy = y + 0.5 - r;
        const d = Math.hypot(dx, dy) / r;
        if (d > 1) continue;
        const color = hsvToHex((Math.atan2(dy, dx) * 180) / Math.PI + 360, d, 1);
        const i = (y * c.width + x) * 4;
        for (let k = 0; k < 3; k++) img.data[i + k] = parseInt(color.slice(1 + k * 2, 3 + k * 2), 16);
        img.data[i + 3] = Math.round(255 * Math.min(1, (1 - d) * r));
      }
    }
    ctx.putImageData(img, 0, 0);
    return c;
  })();
  const color = () => hsvToHex(state.h, state.s, state.v);
  const draw = () => {
    const w = wheel.getContext("2d");
    w.clearRect(0, 0, wheel.width, wheel.height);
    w.drawImage(base, 0, 0);
    w.fillStyle = `rgba(0, 0, 0, ${1 - state.v})`;
    w.beginPath();
    w.arc(wheel.width / 2, wheel.height / 2, wheel.width / 2, 0, Math.PI * 2);
    w.fill();
    const a = (state.h * Math.PI) / 180;
    const mark = (cx, cy) => {
      w.lineWidth = 2 * dpr;
      w.strokeStyle = "#fff";
      w.beginPath();
      w.arc(cx, cy, 6 * dpr, 0, Math.PI * 2);
      w.stroke();
      w.strokeStyle = "rgba(0, 0, 0, 0.5)";
      w.beginPath();
      w.arc(cx, cy, 8 * dpr, 0, Math.PI * 2);
      w.stroke();
    };
    mark(wheel.width / 2 + Math.cos(a) * state.s * wheel.width / 2, wheel.height / 2 + Math.sin(a) * state.s * wheel.height / 2);
    const b = bar.getContext("2d");
    const g = b.createLinearGradient(0, 0, bar.width, 0);
    g.addColorStop(0, "#000");
    g.addColorStop(1, hsvToHex(state.h, state.s, 1));
    b.clearRect(0, 0, bar.width, bar.height);
    b.fillStyle = g;
    b.fillRect(0, 0, bar.width, bar.height);
    b.fillStyle = "#fff";
    b.fillRect(Math.round(state.v * (bar.width - 3 * dpr)), 0, 3 * dpr, bar.height);
    swatch.style.setProperty("--swatch", color());
    if (!hasFocus(hex)) hex.value = color();
  };
  const drag = (node, move) => {
    node.addEventListener("pointerdown", (e) => {
      node.setPointerCapture(e.pointerId);
      move(e);
      const onMove = (m) => move(m);
      node.addEventListener("pointermove", onMove);
      node.addEventListener("pointerup", () => {
        node.removeEventListener("pointermove", onMove);
        page.wheel.commit?.(color());
      }, { once: true });
    });
  };
  drag(wheel, (e) => {
    const box = wheel.getBoundingClientRect();
    const dx = e.clientX - box.left - box.width / 2;
    const dy = e.clientY - box.top - box.height / 2;
    state.h = ((Math.atan2(dy, dx) * 180) / Math.PI + 360) % 360;
    state.s = Math.min(1, Math.hypot(dx, dy) / (box.width / 2));
    draw();
  });
  drag(bar, (e) => {
    const box = bar.getBoundingClientRect();
    state.v = Math.min(1, Math.max(0, (e.clientX - box.left) / box.width));
    draw();
  });
  hex.addEventListener("change", () => {
    const value = hex.value.trim().startsWith("#") ? hex.value.trim() : `#${hex.value.trim()}`;
    const hsv = hexToHsv(value);
    if (!hsv) return draw();
    Object.assign(state, hsv);
    hex.blur();
    draw();
    page.wheel.commit?.(color());
  });
  const node = el("div", { class: "skin-picker" }, wheel, el("div", { class: "skin-picker-side" }, swatch, hex, bar));
  page.wheel = {
    node,
    hex,
    shown: null,
    // the wheel shows the colour it's about without anything else moving
    show(value) {
      if (value === this.shown) return;
      this.shown = value;
      const hsv = hexToHsv(value);
      if (hsv) Object.assign(state, hsv);
      draw();
    },
  };
  draw();
  return page.wheel;
}

function skinColours(page) {
  const e = skinEdit(page);
  const keys = page.keys;
  const layout = (c) => (keys % 2 === 1 && c === Math.floor(keys / 2) ? "S" : (c < keys / 2 ? c : keys - 1 - c) % 2 === 0 ? "1" : "2");
  const on = SKIN_PARTS.map(([p]) => p).filter((p) => e.parts[p]);
  const preset = (pick) => skinChange(page, (x) => {
    for (const p of on) x.colors[p] = Array.from({ length: keys }, (_, c) => pick(c, x.colors[p]));
  });
  const paint = (color) => skinChange(page, (x) => {
    for (const p of on) {
      if (x.picks.length) for (const c of x.picks) x.colors[p][c] = color;
      else x.colors[p] = Array(keys).fill(color);
    }
  });
  // the parts on show their colors, side by side where they differ
  const shown = (c) => {
    const colors = on.map((p) => e.colors[p][c] ?? null);
    if (colors.every((x) => x === colors[0])) return colors[0];
    return `linear-gradient(90deg, ${colors.map((x, i) => `${x ?? "transparent"} ${(i / colors.length) * 100}% ${((i + 1) / colors.length) * 100}%`).join(", ")})`;
  };
  return part("skin-colours", null,
    el("p", { class: "caption" }, !on.length ? "Pick what the color goes on below."
      : e.picks.length ? `Column ${e.picks.map((c) => c + 1).join(", ")}. Unpick them all for every column.` : "A color goes on every column, or pick columns first."),
    el("div", { class: "skin-swatches", "data-drag": "columns" }, Array.from({ length: keys }, (_, c) => el("button", {
      class: `skin-swatch${e.picks.includes(c) ? " on" : ""}${shown(c) ? "" : " none"}`,
      style: shown(c) ? `--swatch: ${shown(c)}` : "",
      title: `Column ${c + 1}`,
      "aria-pressed": String(e.picks.includes(c)),
      onclick: () => {
        e.picks = e.picks.includes(c) ? e.picks.filter((x) => x !== c) : [...e.picks, c].sort((a, b) => a - b);
        render();
      },
    }, c + 1))),
    (() => {
      const wheel = skinWheel(page);
      wheel.commit = paint;
      // a redraw moves the field, which drops its focus mid-typing
      if (hasFocus(wheel.hex)) {
        const [from, to] = [wheel.hex.selectionStart, wheel.hex.selectionEnd];
        requestAnimationFrame(() => {
          wheel.hex.focus();
          wheel.hex.setSelectionRange(from, to);
        });
      }
      const first = on[0];
      const col = e.picks[0] ?? Math.max(0, first ? e.colors[first].findIndex(Boolean) : 0);
      wheel.show((first && (e.colors[first][col] ?? skinPartColour(page, col, first))) ?? "#ffffff");
      return wheel.node;
    })(),
    el("div", { class: "skin-swatches palette" },
      SKIN_PALETTE.map((color) => el("button", { class: "skin-swatch", style: `--swatch: ${color}`, title: color, onclick: () => paint(color) })),
      el("button", { class: "skin-swatch none", title: "The skin's own", onclick: () => paint(null) }, "–")),
    el("div", { class: "skin-tools" },
      el("span", { class: "skin-label" }, "Goes on"),
      el("div", { class: "segs", title: "What the color goes on", "data-drag": "parts" }, SKIN_PARTS.map(([key, text]) => el("button", {
        class: `seg${e.parts[key] ? " on" : ""}`,
        "aria-pressed": String(e.parts[key]),
        onclick: () => {
          e.parts[key] = !e.parts[key];
          render();
        },
      }, text)))),
    el("div", { class: "skin-tools" },
      el("div", { class: "segs" },
        el("button", { class: "seg", title: "The right half takes the left half's colors", onclick: () => preset((c, colors) => colors[c < keys / 2 ? c : keys - 1 - c] ?? null) }, "Mirror the left"),
        el("button", { class: "seg", title: "White and blue, gold in the middle", onclick: () => preset((c) => SKIN_CLASSIC[layout(c)]) }, "Classic"),
        el("button", { class: "seg", disabled: !on.some((p) => e.colors[p].some(Boolean)), onclick: () => preset(() => null) }, "Clear all"))));
}

// combo and score digits, which a skin.ini's fonts point at, not its notes or keys
const SKIN_FONT = /^(?:[0-9]|comma|dot|percent|x)$/i;
// images that suit a part besides its own: a head is often the note, a tail cap often the head
const SKIN_NEAR_ROLES = { note: ["head"], head: ["note"], body: [], tail: ["head", "note"], key: ["keyDown"], keyDown: ["key"] };
// in two groups, so the receptors go under the notes in a narrow window
const SKIN_SWAP_NOTES = [["note", "Notes"], ["head", "LN Heads"], ["body", "LN Bodies"], ["tail", "LN Tails"]];
const SKIN_SWAP_KEYS = [["key", "Receptors"], ["keyDown", "Pressed"]];

async function loadSkinImages(page) {
  if (page.images?.stamp === page.stamp) return;
  page.images = { stamp: page.stamp, list: null };
  try {
    page.images.list = (await (await fetch(`${BRIDGE}/skin/images?at=${page.stamp}`)).json()).images ?? [];
  } catch {
    page.images.list = [];
  }
  if (skinPage === page) render();
}

const skinInUse = (page, e, part, c) => e.swap[part]?.[c] ?? page.live.scene.own[c]?.[part] ?? null;
const skinSame = (a, b) => a != null && b != null && a.toLowerCase().replaceAll("/", "\\") === b.toLowerCase().replaceAll("/", "\\");

function skinParts(page) {
  const e = skinEdit(page);
  const keys = page.keys;
  const list = page.images?.list;
  const cols = e.swapCols.length ? e.swapCols : [...Array(keys).keys()];
  const which = e.swapPart;
  const pick = (path) => skinChange(page, (x) => {
    x.swap[which] ??= [];
    for (const c of cols) x.swap[which][c] = path;
  });
  const tile = (img) => el("button", {
    class: `skin-thumb${cols.every((c) => skinSame(skinInUse(page, e, which, c), img.path)) ? " on" : ""}`,
    title: `${img.path}${img.frames > 1 ? `, ${img.frames} frames` : ""}`,
    onclick: () => pick(img.path),
  }, el("img", {
    src: `${BRIDGE}/skin/thumb?path=${encodeURIComponent(img.path)}&at=${page.stamp}`,
    alt: "",
    loading: "lazy",
    onerror: (ev) => {
      ev.target.parentNode.classList.add("empty");
      if (!page.blanks.has(img.path)) {
        page.blanks.add(img.path);
        render();
      }
    },
  }));
  // blank art only means something as a tail, an invisible one, and one of it is enough
  page.blanks ??= new Set();
  let blankShown = false;
  // a font is ten digit frames, -0 to -9, often with -x, -comma, -dot or -percent beside it
  const fonts = new Set((list ?? []).flatMap((img) => {
    const m = /^(.*)-(?:x|comma|dot|percent)$/i.exec(img.path);
    return m ? [m[1].toLowerCase(), img.path.toLowerCase()] : [];
  }));
  const shown = (img) => {
    const font = fonts.has(img.path.toLowerCase()) || img.frames === 10 || SKIN_FONT.test(img.path.split(/[\\/]/).pop());
    if (font && !cols.some((c) => skinSame(skinInUse(page, e, which, c), img.path))) return false;
    if (!page.blanks.has(img.path)) return true;
    if (which !== "tail" || blankShown) return cols.some((c) => skinSame(skinInUse(page, e, which, c), img.path));
    return (blankShown = true);
  };
  const near = SKIN_NEAR_ROLES[which];
  const fits = (img) => img.roles?.some((r) => r === which || near.includes(r))
    || cols.some((c) => skinSame(skinInUse(page, e, which, c), img.path))
    || (which === "tail" && page.blanks.has(img.path));
  const visible = (list ?? []).filter(shown);
  const likely = visible.filter(fits);
  const rest = likely.length ? visible.filter((img) => !fits(img)) : [];
  const grouped = (images) => {
    const folders = [];
    for (const img of images) {
      if (folders.at(-1)?.name !== img.folder) folders.push({ name: img.folder, images: [] });
      folders.at(-1).images.push(img);
    }
    return folders.map((f) => [f.name && el("p", { class: "skin-folder" }, f.name), f.images.map(tile)]);
  };
  const own = cols.every((c) => !e.swap[which]?.[c]);
  return part("skin-parts", null,
    el("div", { class: "skin-tools skin-part-pick" },
      [[SKIN_SWAP_NOTES, "Part"], [SKIN_SWAP_KEYS, "Receptors, and pressed"]].map(([parts, label]) =>
        segs(parts, which, (p) => { e.swapPart = p; page.otherImages = false; render(); }, label))),
    el("p", { class: "caption" }, e.swapCols.length ? `Column ${e.swapCols.map((c) => c + 1).join(", ")}.` : "Every column, or pick columns."),
    el("div", { class: "skin-swatches", "data-drag": "columns" }, Array.from({ length: keys }, (_, c) => el("button", {
      class: `skin-swatch none${e.swapCols.includes(c) ? " on" : ""}`,
      title: `Column ${c + 1}`,
      "aria-pressed": String(e.swapCols.includes(c)),
      onclick: () => {
        e.swapCols = e.swapCols.includes(c) ? e.swapCols.filter((x) => x !== c) : [...e.swapCols, c].sort((a, b) => a - b);
        render();
      },
    }, c + 1))),
    skinShape(page, which, cols),
    !list ? el("p", { class: "caption" }, "Finding the skin's images…") : el("div", { class: "skin-thumbs", onwheel: skinListWheel },
      el("button", { class: `skin-thumb own${own ? " on" : ""}`, title: "The skin's own", onclick: () => pick(null) }, "The skin's own"),
      grouped(likely.length ? likely : visible),
      rest.length > 0 && el("button", { class: "link skin-others", onclick: () => { page.otherImages = !page.otherImages; render(); } },
        `${page.otherImages ? "Hide" : "Show"} other images (${rest.length})`),
      page.otherImages && grouped(rest)));
}

const SKIN_WIDTH_STEP = 5;
const SKIN_OUTLINE_COLORS = ["#ffffff", "#000000", ...SKIN_PALETTE.slice(1)];

function skinShape(page, which, cols) {
  const e = skinEdit(page);
  const shapes = e.shapes?.[which] ?? [];
  const first = shapes[cols[0]] ?? {};
  const same = cols.every((c) => JSON.stringify(shapes[c] ?? {}) === JSON.stringify(first));
  const set = (patch) => skinChange(page, (x) => {
    x.shapes ??= {};
    x.shapes[which] ??= [];
    for (const c of cols) {
      const s = { ...(x.shapes[which][c] ?? {}), ...patch };
      x.shapes[which][c] = s.width == null && !s.outline && !s.rim ? null : s;
    }
  });
  const width = first.width ?? 100;
  const setWidth = (v) => set({ width: v === 100 ? null : Math.min(200, Math.max(20, v)) });
  const outline = first.outline;
  const rim = [...new Set(cols.map((c) => skinInUse(page, e, which, c)).filter(Boolean))]
    .map((path) => skinRim(page, path, which === "body")?.rim).find(Boolean);
  return [
    el("div", { class: "skin-tools skin-part-pick" },
      el("span", { class: "skin-label" }, "Width"),
      el("div", { class: "segs", title: "How wide the art sits in its column" },
        el("button", { class: "seg", disabled: width <= 20, "aria-label": "Narrower", onclick: () => setWidth(width - SKIN_WIDTH_STEP) }, "−"),
        skinField(page, `Width ${which}`, same ? width : null, 20, 200, setWidth),
        el("span", { class: "skin-unit" }, "%"),
        el("button", { class: "seg", disabled: width >= 200, "aria-label": "Wider", onclick: () => setWidth(width + SKIN_WIDTH_STEP) }, "+")),
      cols.some((c) => shapes[c]?.width != null) && el("button", { class: "link", onclick: () => set({ width: null }) }, "Keep the skin's")),
    el("div", { class: "skin-tools skin-part-pick" },
      el("span", { class: "skin-label" }, "Outline"),
      segs([["off", "None"], ["on", "Outline"]], outline ? "on" : "off", (v) => set({ outline: v === "on" ? { size: 2, color: "#ffffff" } : null }), "Outline"),
      outline && el("div", { class: "segs", title: "How thick the outline is, in ColumnWidth's units" },
        el("button", { class: "seg", disabled: outline.size <= 1, "aria-label": "Thinner", onclick: () => set({ outline: { ...outline, size: outline.size - 1 } }) }, "−"),
        skinField(page, `Outline ${which}`, same ? outline.size : null, 1, 20, (v) => set({ outline: { ...outline, size: v } })),
        el("button", { class: "seg", disabled: outline.size >= 20, "aria-label": "Thicker", onclick: () => set({ outline: { ...outline, size: outline.size + 1 } }) }, "+")),
      outline && segs([["in", "Inside"], ["out", "Outside"]], outline.outside ? "out" : "in",
        (v) => set({ outline: { ...outline, outside: v === "out" } }), "Inside the art's edge, or around it in the image's clear space")),
    outline && el("div", { class: "skin-swatches palette" }, SKIN_OUTLINE_COLORS.map((color) => el("button", {
      class: `skin-swatch${outline.color === color ? " on" : ""}`,
      style: `--swatch: ${color}`,
      title: color,
      onclick: () => set({ outline: { ...outline, color } }),
    }))),
    rim && el("div", { class: "skin-tools skin-part-pick" },
      el("span", { class: "skin-label" }, "Its own outline"),
      segs([["", "As it is"], ["keep", "Keep its color"], ["remove", "Remove"]], same ? first.rim ?? "" : null, (v) => set({ rim: v || null }), "Its own outline")),
    rim && el("p", { class: "caption" },
      el("span", { class: "skin-rim-swatch", style: `--swatch: ${rim.color}` }),
      ` This art has a ${rim.px} px outline drawn in. Keep its color leaves it as it is when you color it, Remove paints it over with the color inside.`),
  ];
}

// a body's art repeats along its length, so only its sides count as its edge
function skinRim(page, path, body) {
  const key = `${page.stamp} ${body ? 0 : 1} ${path.toLowerCase()}`;
  page.rims ??= new Map();
  if (!page.rims.has(key)) {
    page.rims.set(key, null);
    fetch(`${BRIDGE}/skin/rim?path=${encodeURIComponent(path)}&at=${page.stamp}&ends=${body ? 0 : 1}`)
      .then((r) => r.json())
      .then((v) => {
        page.rims.set(key, v);
        if (v.rim && skinPage === page) render();
      })
      .catch(() => {});
  }
  return page.rims.get(key);
}

// lazer takes a moment to import the skin and switch to it
async function skinImported(name) {
  for (let i = 0; i < 30; i++) {
    await new Promise((r) => setTimeout(r, 500));
    try {
      if ((await (await fetch(`${BRIDGE}/skin`)).json()).name === name) return true;
    } catch {}
  }
  return false;
}

async function skinWrite(page, path) {
  const keys = page.keys;
  const edits = skinEditsJson(page);
  page.applying = true;
  page.saved = null;
  render();
  let note;
  try {
    const name = path === "apply-new" ? `&name=${encodeURIComponent(page.newName ?? "")}` : "";
    const state = path === "apply" && page.edits?.[keys] ? `&state=${encodeURIComponent(JSON.stringify(page.edits[keys]))}` : "";
    const query = path === "undo" ? "" : `?keys=${keys}&edits=${encodeURIComponent(edits)}${name}${state}`;
    const res = await fetch(`${BRIDGE}/skin/${path}${query}`, { method: "POST" });
    const body = await res.json();
    if (!res.ok) throw new Error(body.error);
    if (path === "apply-new") {
      const switched = await skinImported(body.name);
      if (switched) {
        page.edits = {};
        page.history = {};
        page.wide = null;
        page.newName = null;
      }
      note = switched
        ? { ok: true, text: `Lazer switched to “${body.name}”.` }
        : { ok: false, text: `Sent “${body.name}” to lazer, but it hasn't switched to it. Is lazer open?` };
      if (skinPage !== page) return;
      page.applying = false;
      if (switched) await loadSkin();
      page.saved = note;
      render();
      return;
    }
    if (path === "undo") {
      // two objects: shared, a column's edits and its undo history would land on the same key
      page.edits = {};
      page.history = {};
    }
    // read back off the skin, which keeps the % only as precisely as whole pixels
    if (path === "undo" || path === "apply") page.wide = null;
    const reload = page.info.client !== "lazer"
      ? "In osu!, press Ctrl+Alt+Shift+S to reload the skin."
      : page.info.stored
        ? "In lazer, switch to another skin and back to see it."
        : "Finish editing in lazer's skin editor to bring it in.";
    const done = { apply: "Saved to the skin.", fix: "Saved them as PNGs.", undo: "Put the skin back as it was." }[path];
    note = { ok: true, text: `${done} ${reload}` };
  } catch (e) {
    note = { ok: false, text: e.message || "Couldn't change the skin." };
  }
  if (skinPage !== page) return;
  page.applying = false;
  await loadSkin();
  page.saved = note;
  render();
}

function skinUnreadable(page) {
  const list = page.info.unreadable;
  const names = list.slice(0, 3).join(", ") + (list.length > 3 ? ` and ${list.length - 3} more` : "");
  const what = list.length === 1 ? `${names}. It's a TIFF with a .png name` : `${list.length} images, TIFFs with .png names: ${names}`;
  return el("div", { class: "skin-tools skin-warn" },
    el("p", { class: "caption" }, `Lazer can't read ${what}, so it draws nothing there.`),
    el("button", {
      class: "key go",
      disabled: page.applying || page.loading,
      title: page.info.saves ? "" : "Saved as a new skin with any changes below, lazer switches to it",
      onclick: () => skinWrite(page, page.info.saves ? "fix" : "apply-new"),
    }, page.info.saves ? "Fix" : "Fix as a new skin"));
}

// kept across redraws, which would drop its focus and caret mid-typing
function skinNameField(page) {
  const input = (page.nameField ??= el("input", {
    class: "skin-name", type: "text", spellcheck: "false", maxlength: 120, "aria-label": "The new skin's name",
    oninput: (ev) => (page.newName = ev.target.value),
  }));
  if (page.newName == null) input.value = page.info.newName ?? "";
  input.placeholder = page.info.newName ?? "";
  if (hasFocus(input)) {
    const [from, to] = [input.selectionStart, input.selectionEnd];
    requestAnimationFrame(() => {
      input.focus();
      input.setSelectionRange(from, to);
    });
  }
  return input;
}

const SKIN_HIT_STEP = 1;
const SKIN_HIT_BIG_STEP = 10;
const SKIN_COLUMN_START_MAX = 853;

function skinNumber(page, label, value, min, max, set) {
  const step = (d) => () => set(Math.min(max, Math.max(min, value + d)));
  return el("div", { class: "segs", title: label },
    el("button", { class: "seg", disabled: value <= min, "aria-label": `${label} down by ${SKIN_HIT_BIG_STEP}`, onclick: step(-SKIN_HIT_BIG_STEP) }, `−${SKIN_HIT_BIG_STEP}`),
    el("button", { class: "seg", disabled: value <= min, "aria-label": `${label} down by ${SKIN_HIT_STEP}`, onclick: step(-SKIN_HIT_STEP) }, "−"),
    skinField(page, label, value, min, max, set),
    el("button", { class: "seg", disabled: value >= max, "aria-label": `${label} up by ${SKIN_HIT_STEP}`, onclick: step(SKIN_HIT_STEP) }, "+"),
    el("button", { class: "seg", disabled: value >= max, "aria-label": `${label} up by ${SKIN_HIT_BIG_STEP}`, onclick: step(SKIN_HIT_BIG_STEP) }, `+${SKIN_HIT_BIG_STEP}`));
}

// the ColumnStart that puts the stage in the middle of the screen osu! draws on
function skinCentred(page) {
  const s = page.live.scene;
  const size = page.info?.gameSize;
  const aspect = size?.width ? size.width / size.height : size === "screen" ? screen.width / screen.height : s.width / s.height;
  const first = s.columns[0];
  const last = s.columns[s.columns.length - 1];
  const stage = (last.x + last.w - first.x) / (s.height / 480);
  return Math.round(Math.max(0, (480 * aspect - stage) / 2));
}

function skinRow(name, line, control, own, reset) {
  return el("div", { class: `skin-row${reset ? " changed" : ""}` },
    el("div", { class: "text" },
      el("span", { class: "title" }, name),
      el("span", { class: "line" }, line, reset && [line && " · ", `skin's ${own} `, el("button", { class: "link", onclick: reset }, "Reset")])),
    control);
}

const SKIN_STAGE_KEYS = ["hit", "raise", "bottom", "line", "score", "combo", "comboScale", "start", "colWidth", "colSpacing"];
// 0, not null, leaves these as they are
const SKIN_RAISES = ["raise", "bottom"];

// kept outside the list, as every render makes it again
let skinListScrolled = 0;

// a scroll that runs the list to its end stops there; after a pause the wheel scrolls the page, so
// the list never traps it
function skinListWheel(ev) {
  const list = ev.currentTarget;
  const room = list.scrollHeight - list.clientHeight;
  if (room <= 1 || !ev.deltaY) return;
  const now = performance.now();
  const atEnd = ev.deltaY > 0 ? list.scrollTop >= room - 1 : list.scrollTop <= 0;
  if (!atEnd) {
    skinListScrolled = now;
    return;
  }
  if (now - skinListScrolled < 400) {
    skinListScrolled = now;
    ev.preventDefault();
  }
}

function skinStage(page) {
  const e = skinEdit(page);
  const s = page.live.scene;
  const own = Math.round(s.ownHit);
  const hit = e.hit ?? own;
  const line = e.line ?? s.ownLine;
  const [ownScore, ownCombo] = [Math.round(s.ownScore), Math.round(s.ownCombo)];
  const [score, combo] = [e.score ?? ownScore, e.combo ?? ownCombo];
  const ownStart = Math.round(s.ownColumnStart);
  const start = e.start ?? ownStart;
  const centred = skinCentred(page);
  const [ownWidth, ownSpacing] = [Math.round(s.ownColumnWidth), Math.round(s.ownColumnSpacing)];
  const [colWidth, colSpacing] = [e.colWidth ?? ownWidth, e.colSpacing ?? ownSpacing];
  // the receptors as the HitPosition that lands a note's art on theirs, so lining them up is the two
  // numbers matching
  const ownKeys = s.ownKeyBottom == null ? null : Math.round(s.ownKeyBottom + s.notePad);
  const keysAt = ownKeys == null ? null : ownKeys - (e.raise ?? 0);
  const setHit = (v) => skinChange(page, (x) => (x.hit = v === own ? null : v));
  const setKeys = (v) => skinChange(page, (x) => (x.raise = ownKeys - v));
  const reset = (key, to = null) => () => skinChange(page, (x) => (x[key] = to));
  const changed = SKIN_STAGE_KEYS.some((k) => (SKIN_RAISES.includes(k) ? !!e[k] : e[k] != null));
  const ownBottom = s.ownBottomArt == null || s.upscroll ? null : Math.round(s.ownBottomArt);
  const bottomAt = ownBottom == null ? null : ownBottom - (e.bottom ?? 0);
  const keysShut = keysAt === hit && !e.raise && !page.keysOpen;
  return part("skin-stage-edit", null,
    el("div", { class: "skin-group" },
      el("span", {}, "Judgement"),
      changed && el("button", { class: "link", onclick: () => skinChange(page, (x) => { for (const k of SKIN_STAGE_KEYS) x[k] = SKIN_RAISES.includes(k) ? 0 : null; }) }, "Reset stage")),
    // HitPosition counts down from the top of a 480-tall stage, so a bigger number sits lower
    skinRow("HitPosition", "Higher sits lower", skinNumber(page, "HitPosition", hit, 0, 480, setHit), own, e.hit != null && reset("hit")),
    keysAt != null && skinRow("Receptors",
      keysAt === hit
        ? "Notes land right on them"
        : [`The line is ${hit > keysAt ? "below" : "above"} them · `,
          el("button", { class: "link", onclick: () => setHit(keysAt) }, "Line to them"), " · ",
          el("button", { class: "link", onclick: () => setKeys(hit) }, "Them to the line")],
      keysShut
        ? el("button", { class: "link", onclick: () => { page.keysOpen = true; render(); } }, "Move them")
        : skinNumber(page, "Receptors", keysAt, 0, 480, setKeys),
      ownKeys, !!e.raise && reset("raise", 0)),
    skinRow("Judgement line", "The line notes are hit on",
      segs([[true, "On"], [false, "Off"]], line, (on) => skinChange(page, (x) => (x.line = on === s.ownLine ? null : on)), "Judgement line"),
      s.ownLine ? "on" : "off", e.line != null && reset("line")),
    bottomAt != null && skinRow("StageBottom art", "The middle of its art, higher sits lower",
      skinNumber(page, "StageBottom art", bottomAt, 0, 480, (v) => skinChange(page, (x) => (x.bottom = ownBottom - v))),
      ownBottom, !!e.bottom && reset("bottom", 0)),
    el("div", { class: "skin-group" }, el("span", {}, "Judgements and combo")),
    skinRow("ScorePosition", "Where judgements show, higher sits lower",
      skinNumber(page, "ScorePosition", score, 0, 480, (v) => skinChange(page, (x) => (x.score = v === ownScore ? null : v))),
      ownScore, e.score != null && reset("score")),
    // a combo placed in lazer's HUD editor stays where it put it, ComboPosition or not
    s.ownComboHud
      ? skinRow("ComboPosition", "Lazer shows this skin's combo where its HUD editor put it. Move it there")
      : skinRow("ComboPosition", s.ownComboHidden && e.combo == null ? "This skin hides its combo. A number shows it there" : "Where the combo shows, higher sits lower",
        skinNumber(page, "ComboPosition", combo, 0, 480, (v) => skinChange(page, (x) => (x.combo = v === ownCombo && !s.ownComboHidden ? null : v))),
        s.ownComboHidden ? "hidden" : ownCombo, e.combo != null && reset("combo")),
    // only a combo drawn in digit images can be resized; lazer sizes its combo in its own skin editor
    s.hud?.combo && page.info?.client !== "lazer" && skinRow("Combo size", "Its digits, bigger or smaller",
      [skinNumber(page, "Combo size", e.comboScale ?? 100, 30, 300, (v) => skinChange(page, (x) => (x.comboScale = v === 100 ? null : v))), el("span", { class: "skin-unit" }, "%")],
      "100%", e.comboScale != null && reset("comboScale")),
    skinAccuracy(page),
    el("div", { class: "skin-group" }, el("span", {}, "Columns")),
    skinRow("ColumnStart",
      ["From the left edge", start !== centred && [" · ", el("button", { class: "link", onclick: () => skinChange(page, (x) => (x.start = centred === ownStart ? null : centred)) }, `Centre (${centred})`)]],
      skinNumber(page, "ColumnStart", start, 0, SKIN_COLUMN_START_MAX, (v) => skinChange(page, (x) => (x.start = v === ownStart ? null : v))),
      ownStart, e.start != null && reset("start")),
    skinRow("ColumnWidth", "Every column",
      skinNumber(page, "ColumnWidth", colWidth, 20, 160, (v) => skinChange(page, (x) => (x.colWidth = v === ownWidth ? null : v))),
      ownWidth, e.colWidth != null && reset("colWidth")),
    skinRow("ColumnSpacing", "Between columns",
      skinNumber(page, "ColumnSpacing", colSpacing, 0, 40, (v) => skinChange(page, (x) => (x.colSpacing = v === ownSpacing ? null : v))),
      ownSpacing, e.colSpacing != null && reset("colSpacing")));
}

// Stable right-aligns the accuracy at the top right, at 0.6 of the score's 0.96 on a HUD 768 tall.
// Skins move it with a % wider than its art, which only holds for one window shape. The margin is
// the helper's ACC_MARGIN
const SKIN_ACC_SCALE = 0.6 * 0.96;
const SKIN_ACC_MARGIN = 6;

function skinGameSize(page) {
  const g = page.info?.gameSize;
  if (g?.width && g?.height) return { w: g.width, h: g.height, known: true };
  if (g === "screen") return { w: Math.round(screen.width * devicePixelRatio), h: Math.round(screen.height * devicePixelRatio), known: true };
  return { w: 1920, h: 1080, known: false };
}

// where the accuracy's numbers end, in the game's pixels from the left, for a % `pct` skin px wide
function skinAccuracyEnd(font, size, pct) {
  const s = size.h / 768;
  return Math.round((size.w / s - SKIN_ACC_MARGIN - (pct - font.overlap) * SKIN_ACC_SCALE) * s);
}

function skinPercentFor(font, size, end) {
  const s = size.h / 768;
  return Math.round(((size.w / s - SKIN_ACC_MARGIN - end / s) / SKIN_ACC_SCALE + font.overlap) * 10) / 10;
}

// lazer places its accuracy in its own skin editor
function skinAccuracy(page) {
  if (page.info?.client === "lazer") return null;
  const font = page.live.scene.hud?.accuracy;
  if (!font?.digit || font.percent == null) return null;
  const size = skinGameSize(page);
  const s = size.h / 768;
  const own = skinAccuracyEnd(font, size, font.percent);
  const pct = page.wide?.percentWidth;
  const end = pct == null ? own : skinAccuracyEnd(font, size, pct);
  // "100.00" before the %, digits as wide as a 5 and the overlap between each
  const numbers = Math.round((5 * font.digit + (font.dot ?? font.digit) - 5 * font.overlap) * SKIN_ACC_SCALE * s);
  const most = size.w - Math.round(SKIN_ACC_MARGIN * s);
  const set = (v) => {
    page.wide = { percentWidth: v === own ? null : skinPercentFor(font, size, Math.min(most, Math.max(numbers, v))) };
    render();
  };
  return [
    el("div", { class: "skin-group" }, el("span", {}, "Accuracy, for the whole skin")),
    skinRow("Accuracy position",
      `Where its numbers end, from the left of your ${size.w}×${size.h} game${size.known ? "" : " (open osu! for its real size)"}`,
      [skinNumber(page, "Accuracy position", end, numbers, most, set), el("span", { class: "skin-unit" }, "px")],
      own, pct != null && (() => { page.wide = { percentWidth: null }; render(); })),
  ];
}

const skinApplied = (page) => page.applied?.[page.keys]?.edits ?? "";

// a keymode's applied edits keep the accuracy as it was then, so it's compared with the skin's now
function skinOwnEdits(json) {
  if (!json) return "";
  const { percentWidth, ...rest } = JSON.parse(json);
  return Object.keys(rest).length ? JSON.stringify(rest) : "";
}

function skinEditor(page) {
  const json = skinEditsJson(page);
  const changed = !!skinOwnEdits(json);
  const differs = skinOwnEdits(json) !== skinOwnEdits(skinApplied(page))
    || (page.wide?.percentWidth ?? null) !== (page.applied?._accuracy?.percentWidth ?? null);
  const tab = page.tab ?? "lns";
  const h = skinHistory(page);
  return el("div", { class: "skin-editor" },
    el("div", { class: "skin-tools skin-tabs" },
      segs([["lns", "Long notes"], ["parts", "Parts"], ["colours", "Colors"], ["stage", "Stage"]], tab, (t) => { page.tab = t; render(); }, "Edit"),
      el("div", { class: "segs skin-undo" },
        el("button", { class: "seg", disabled: !h.undo.length, title: "Undo (Ctrl+Z)", onclick: () => skinUndo(page, true) }, "Undo"),
        el("button", { class: "seg", disabled: !h.redo.length, title: "Redo (Ctrl+Y)", onclick: () => skinUndo(page, false) }, "Redo"),
        el("button", {
          class: "seg", disabled: !changed, title: `Every change to ${page.keys}K back to the skin's own. Undo brings them back`,
          // the picked columns stay picked
          onclick: () => skinChange(page, (x) => Object.assign(x, skinFresh(), Object.fromEntries(SKIN_PICKS.map((k) => [k, x[k]])))),
        }, "Reset all"))),
    tab === "lns" ? skinLongNotes(page) : tab === "parts" ? skinParts(page) : tab === "stage" ? skinStage(page) : skinColours(page),
    page.info?.stored && !page.info.saves
      ? [el("div", { class: "skin-tools apply" },
        skinNameField(page),
        el("button", { class: "key go", disabled: !changed || page.applying || page.loading, onclick: () => skinWrite(page, "apply-new") }, "Save as a new skin")),
      el("p", { class: "caption" }, "Lazer imports it and switches to it. To change this skin itself, use File, Edit externally in lazer's skin editor.")]
      : [el("div", { class: "skin-tools apply" },
        el("button", { class: "key go", disabled: !differs || page.applying || page.loading, onclick: () => skinWrite(page, "apply") }, changed || !skinOwnEdits(skinApplied(page)) ? "Apply to skin" : `Put ${page.keys}K back`),
        page.info?.edited && el("button", { class: "link", disabled: page.applying, onclick: () => skinWrite(page, "undo") }, "Undo Hashi's changes"),
        page.info?.stored && !page.asNew && el("button", { class: "link", disabled: page.applying, onclick: () => { page.asNew = true; render(); } }, "Save as a new skin instead")),
      page.info?.stored && page.asNew && el("div", { class: "skin-tools apply" },
        skinNameField(page),
        el("button", { class: "key", disabled: !changed || page.applying || page.loading, onclick: () => skinWrite(page, "apply-new") }, "Save as a new skin")),
      page.info?.edited && el("p", { class: "caption" }, "These edits start from your skin as it was, so applying them replaces what Hashi applied before."),
      page.info?.stored && el("p", { class: "caption" }, "Saved straight into lazer's copy of the skin. Lazer shows it once you switch to another skin and back.")],
    page.saved && el("p", { class: `status ${page.saved.ok ? "good" : "bad"}` }, page.saved.text));
}
