// ?overlay: the window as a small card that stays over the game. ?overlay=stream: the same card for an
// OBS browser source, set up by its link, never touching the window, scaled to the source's width
const OVERLAY = new URLSearchParams(location.search).has("overlay");
const STREAM = new URLSearchParams(location.search).get("overlay") === "stream";
// a wayland layer: the compositor can't move or resize it, the page does through the app
const LAYER = new URLSearchParams(location.search).has("layer");
const PIP = `<rect x=".5" y="1.5" width="9" height="7" rx="1"/><rect x="5" y="5" width="3" height="2" fill="currentColor" stroke="none"/>`;
const DISC = `<circle cx="5" cy="5" r="4.5"/><circle cx="5" cy="5" r="1.3"/>`;
const GRIP = `<path d="M9.5 4.5l-5 5M9.5 7.5l-2 2" stroke-linecap="round"/>`;
const OV_KEY = "hashi-overlay";
// the card is laid out this wide, and made bigger or smaller as a whole
const OV_WIDTH = 340;
const OV_ROW_PX = 30;
// a play stays up front this long after it's done
const OV_FRESH_MS = 3 * 60_000;

// what a stream link can say, and what it means when it doesn't
const STREAM_DEFAULTS = { view: "now", bg: 85, art: 1, playing: "show", rows: 8 };

function streamSettings() {
  const q = new URLSearchParams(location.search);
  const num = (k, lo, hi) => {
    const n = Number(q.get(k));
    return q.has(k) && Number.isFinite(n) ? Math.max(lo, Math.min(hi, n)) : STREAM_DEFAULTS[k];
  };
  return {
    view: q.get("view") === "board" ? "board" : "now",
    see: num("bg", 0, 100),
    art: num("art", 0, 1) === 1,
    playing: ["show", "fade", "hide"].includes(q.get("playing")) ? q.get("playing") : STREAM_DEFAULTS.playing,
    rows: Math.round(num("rows", 3, 30)),
    zoom: 100,
  };
}

const ov = {
  set: STREAM ? streamSettings() : { see: 90, zoom: 100, rows: 8, art: true, status: true, playing: "fade", view: "now", ...stored(OV_KEY, {}) },
  open: false,
  holding: false,
  sent: { size: "", zoom: 0, through: null, blur: null },
  // the window changing size because it was asked to, not because it was dragged
  askedAt: 0,
  // a drag is under way, the window's size isn't asked for until it settles
  dragging: false,
  newest: undefined,
  art: { md5: null, src: null },
  card: null,
  seen: new Map(),
};

// the card is drawn again on every change, so something that animates in carries when it first
// showed and picks up from there
function since(key) {
  if (!ov.seen.has(key)) ov.seen.set(key, Date.now());
  return `animation-delay: -${Date.now() - ov.seen.get(key)}ms`;
}

function overlayButton() {
  return el("button", { class: "overlay-go", title: "Overlay mode", onclick: () => windowAction("overlay/on") }, icon(PIP));
}

function ovSave(change) {
  change(ov.set);
  if (!STREAM) store(OV_KEY, ov.set);
  render();
}

const ovPlaying = () => snapshot?.tosu.state === "play";

// the selected map's background, swapped in once it's loaded so the card never shows it half drawn
function ovArt(map) {
  if (!ov.set.art || !map) return null;
  if (ov.art.md5 !== map.md5) {
    const md5 = map.md5;
    ov.art.md5 = md5;
    const img = new Image();
    img.onload = () => ov.art.md5 === md5 && ((ov.art.src = img.src), render());
    img.onerror = () => ov.art.md5 === md5 && ((ov.art.src = null), render());
    img.src = `${BRIDGE}/cover/selected?md5=${md5}`;
  }
  return ov.art.src;
}

function ovMap(map) {
  pickedMap();
  const d = picked?.data ?? picked?.last;
  const showDans = snapshot.showDans !== false;
  const dans = d && showDans ? [d.dan, d.other].filter(Boolean) : [];
  return el("div", { class: `ov-map${picked?.data ? "" : " stale"}` },
    el("div", { class: "what" },
      map.id
        ? el("a", { class: "title", href: `${snapshot.connection.origin}/maps?map=${map.id}`, target: "_blank", rel: "noreferrer", title: "Open on Mania Tracker" }, map.title ?? "Unknown")
        : el("span", { class: "title" }, map.title ?? "Unknown"),
      el("span", { class: "by" }, [map.artist, diffName(map.keys, map.version)].filter(Boolean).join(" · "))),
    el("div", { class: "numbers" },
      el("span", { class: "msd" },
        !d ? el("b", { class: "wait" }) : d.msd != null && el("b", {}, d.msd.toFixed(2)),
        el("small", {}, `${ratingName(map.keys)}${d && d.rate !== 1 ? ` ${d.rate}x` : ""}`)),
      dans.length > 0 && el("span", { class: "dans" }, dans.map((x) => danBadge(x, d.keys)))));
}

const ovFresh = (p) => p && (playStatus(p).kind === "spin" || Date.now() - new Date(p.capturedAt) < OV_FRESH_MS);

// the play just passed, up front: how it went, then where it landed once Mania Tracker has it
function ovResult(p, map) {
  const status = playStatus(p);
  const acc = typeof p.play.accuracy === "number" && p.play.accuracy > 0 ? `${accText(p.play.accuracy)}%` : null;
  const mods = modList(p.play.mods);
  const r = p.rating;
  const elsewhere = !map || map.md5 !== p.md5;
  return el("div", { class: `ov-result ${status.kind}`, style: since(`result ${p.id}`) },
    el("div", { class: "line" },
      el("span", { class: "label" }, "Just played"),
      stamp(status, p.id)),
    elsewhere && el("span", { class: "title", title: `${p.play.artist ?? ""} - ${chartName(p.play)}` }, chartName(p.play)),
    el("div", { class: "how" },
      acc && el("b", { class: "acc" }, acc),
      mods.length > 0 && el("span", { class: "mods" }, mods.map((m) => modBadge(m, { rate: p.play.rate })))),
    (p.rank || typeof r?.msd === "number" || r?.dan) && el("div", { class: "where", style: since(`where ${p.id}`) },
      p.rank && el("span", { class: `rank${p.rank.best ? " best" : ""}`, title: p.rank.best ? "A new best on this map" : "Not a new best" },
        `#${p.rank.position}`, p.rank.total && el("small", {}, ` of ${p.rank.total}`)),
      typeof r?.msd === "number" && el("span", { class: "msd" }, el("b", {}, r.msd.toFixed(2)), ` ${ratingName(p.play.keys)}`),
      r?.dan && snapshot.showDans !== false && danText(r.dan)),
    ["fail", "wait"].includes(status.kind) && status.detail && el("p", { class: "why" }, status.detail));
}

function ovLast(p) {
  const status = playStatus(p);
  const acc = typeof p.play.accuracy === "number" && p.play.accuracy > 0 ? `${accText(p.play.accuracy)}%` : null;
  const mods = modList(p.play.mods);
  return el("div", { class: "ov-last" },
    el("span", { class: "label" }, "Last"),
    el("span", { class: "title", title: `${p.play.artist ?? ""} - ${chartName(p.play)}` }, p.play.title ?? "Unknown"),
    mods.length > 0 && el("span", { class: "mods" }, mods.map((m) => modBadge(m, { rate: p.play.rate }))),
    acc && el("b", {}, acc),
    el("span", { class: `dot ${status.kind}`, title: status.label }));
}

function ovNow(map) {
  const last = snapshot.plays[0];
  const fresh = !ovPlaying() && ovFresh(last);
  return [
    map ? ovMap(map) : !STREAM && el("p", { class: "ov-empty" }, snapshot.tosu.osu ? "Select a mania map in osu!" : "Open osu! to see your map here."),
    fresh ? ovResult(last, map) : last && !ovPlaying() && ovLast(last),
  ];
}

function ovStanding(e, mine) {
  const [grade, tone] = GRADES[e.grade] ?? [e.grade, "d"];
  return el("li", { class: `ov-standing${e.username === snapshot.connection.username ? " me" : ""}${mine ? " mine" : ""}` },
    el("span", { class: "pos" }, `#${e.position ?? "?"}`),
    grade && el("span", { class: `grade ${tone}` }, grade),
    el("span", { class: "avatar" }, e.avatarUrl && rowImg(`avatar ${e.userId ?? e.username}`, { src: e.avatarUrl, alt: "" })),
    el("span", { class: "player" }, e.username),
    el("span", { class: "mods" }, e.mods.filter((m) => m !== "CL").map((m) => modBadge(m, { rate: e.rate }))),
    el("span", { class: "acc" }, typeof e.accuracy === "number" ? `${accText(e.accuracy)}%` : ""));
}

function ovBoard(map) {
  if (!map) return el("p", { class: "ov-empty" }, "Select a mania map in osu! to see its leaderboard.");
  if (!board || boardKey(board) !== boardKey(map)) openBoard(map, true);
  else boardChanged();
  const { data } = board;
  const head = el("div", { class: "ov-board-head" },
    el("span", { class: "title" }, map.title ?? "Unknown"),
    data?.entries?.length > 0 && el("span", { class: "count" }, `${data.total} ${data.total === 1 ? "player" : "players"}`));
  let body;
  if (board.error) body = el("p", { class: "ov-empty" }, board.error);
  else if (!data) body = el("p", { class: "doing" }, el("i", { class: "spinner", style: `animation-delay: -${Date.now() % SPIN_MS}ms` }), "Loading");
  else if (data.unsupported || data.unknown) body = el("p", { class: "ov-empty" }, "Mania Tracker has no leaderboard for this map.");
  else if (!data.entries.length) body = el("p", { class: "ov-empty" }, "Nobody has played this map yet.");
  else {
    const mine = data.entries.findIndex((e) => e.username === snapshot.connection.username);
    const room = Math.max(1, ov.set.rows - (data.self && mine < 0 ? 1 : 0));
    const last = Math.max(0, data.entries.length - room);
    board.from = Math.max(0, Math.min(board.from, last));
    const wheel = (e) => {
      e.preventDefault();
      board.from = Math.max(0, Math.min(board.from + Math.sign(e.deltaY), last));
      render();
    };
    body = el("ol", { class: "ov-standings", onwheel: wheel },
      data.entries.slice(board.from, board.from + room).map((e) => ovStanding(e)),
      data.self && mine < 0 && ovStanding(data.self, true));
  }
  return [head, body];
}

function ovSettings() {
  const s = ov.set;
  const row = (title, control) => el("div", { class: "ov-setting" }, el("span", {}, title), control);
  const see = el("input", {
    type: "range", class: "skin-seek-bar", min: 0, max: 100, step: 5, value: s.see, "aria-label": "Background",
    style: `--at: ${s.see}%`,
    onpointerdown: () => (ov.holding = true),
    oninput: (e) => {
      s.see = Number(e.target.value);
      e.target.style.setProperty("--at", `${s.see}%`);
      document.querySelector(".ov")?.style.setProperty("--see", s.see / 100);
    },
    onchange: () => ovSave(() => {}),
  });
  return el("div", { class: "ov-settings" },
    row("Background", see),
    row("Status line", el("button", {
      class: `switch${s.status ? " on" : ""}`, role: "switch", "aria-checked": String(s.status), "aria-label": "Status line",
      onclick: () => ovSave((x) => (x.status = !x.status)),
    }, el("i"))),
    row("Map art", el("button", {
      class: `switch${s.art ? " on" : ""}`, role: "switch", "aria-checked": String(s.art), "aria-label": "Map art",
      onclick: () => ovSave((x) => (x.art = !x.art)),
    }, el("i"))),
    row("While playing", segs([["show", "Show"], ["fade", "Fade"], ["hide", "Hide"]], s.playing, (p) => ovSave((x) => (x.playing = p)), "While playing")),
    row("Leaderboard rows", segs([[5, "5"], [8, "8"], [10, "10"], [15, "15"]], s.rows, (r) => ovSave((x) => (x.rows = r)), "Leaderboard rows")),
    el("p", { class: "ov-hint" }, "Drag the corner to make it bigger or smaller."),
    el("button", { class: "link", onclick: () => ((ov.open = false), render()) }, "Done"));
}

function ovBar() {
  const account = !!snapshot.connection.username;
  const tab = (view, title, art) => el("button", {
    class: ov.set.view === view && !ov.open ? "on" : "",
    title,
    "aria-pressed": String(ov.set.view === view && !ov.open),
    onclick: () => ((ov.open = false), ovSave((x) => (x.view = view))),
  }, icon(art));
  return el("div", { class: "bar" },
    now(),
    el("div", { class: "tools" },
      account && tab("now", "Map and plays", DISC),
      account && tab("board", "Leaderboard", PODIUM),
      el("button", { class: ov.open ? "on" : "", title: "Overlay settings", "aria-pressed": String(ov.open), onclick: () => ((ov.open = !ov.open), render()) }, icon(GEAR)),
      snapshot.tray && el("button", { title: "Hide to tray", onclick: () => windowAction("hide") }, icon(TRAY)),
      el("button", { title: "Back to the window", onclick: () => windowAction("overlay/off") }, icon(MAXIMIZE))));
}

// kept, not drawn again, so it can fade as a map starts and flash when a play comes in
function ovShell() {
  if (ov.card) return ov.card;
  ov.card = el("div", {
    class: "ov",
    // the whole card moves it, as a title bar would
    onmousedown: (e) => {
      if (STREAM || e.button !== 0 || e.target.closest("button, a, input")) return;
      if (LAYER) layerDrag(e, "move");
      else windowAction("drag");
    },
  });
  return ov.card;
}

function overlayCard() {
  const card = ovShell();
  if (!snapshot) {
    card.className = "ov";
    card.replaceChildren(el("p", { class: "status warn" }, helperDown ? "Lost Hashi's helper. Reconnecting." : ""));
    return card;
  }
  const map = snapshot.tosu.map;
  const newest = snapshot.plays[0]?.id;
  const flash = ov.newest !== undefined && newest && newest !== ov.newest;
  ov.newest = newest ?? null;
  const art = ovArt(map);
  const view = ov.open ? "settings" : ov.set.view === "board" && snapshot.connection.username ? "board" : "now";
  const playing = ovPlaying();
  // on stream, nothing at all rather than a card saying there's nothing
  const empty = STREAM && (view === "board" ? !map : !map && !(ovFresh(snapshot.plays[0]) && !playing));
  card.className = `ov view-${view}${playing ? ` playing ${ov.set.playing}` : ""}${ov.set.see < 45 ? " thin" : ""}${empty ? " empty" : ""}${ov.set.status || ov.open ? "" : " quiet"}${card.classList.contains("flash") ? " flash" : ""}`;
  card.style.setProperty("--see", ov.set.see / 100);
  if (art) card.style.setProperty("--art", `url("${art}")`);
  else card.style.removeProperty("--art");
  if (flash) {
    card.classList.remove("flash");
    void card.offsetWidth;
    card.classList.add("flash");
  }
  const parts = [
    art && el("div", { class: "ov-art" }),
    // on stream it's for the viewers: none of the controls or the notes meant for the player
    !STREAM && [ovBar(), readHint(), statusLine(snapshot.connection)],
    view === "settings" ? ovSettings() : view === "board" ? ovBoard(map) : ovNow(map),
    !STREAM && el("button", {
      class: "grip",
      title: "Resize",
      "aria-label": "Resize",
      onmousedown: (e) => {
        if (e.button !== 0) return;
        if (LAYER) layerDrag(e, "scale");
        else windowAction("resize");
      },
    }, icon(GRIP))];
  card.replaceChildren(...parts.flat(Infinity).filter((c) => c != null && c !== false));
  return card;
}

// the window takes the card's size at its scale: as tall as what it shows, the leaderboard as many
// rows as it was dragged to
function ovFit(card) {
  const s = ov.set;
  if (ov.dragging) return;
  if (ov.sent.zoom !== s.zoom) {
    ov.sent.zoom = s.zoom;
    ov.askedAt = Date.now();
    windowAction(`overlay-zoom/${s.zoom}`);
  }
  const h = Math.ceil(card.getBoundingClientRect().height);
  const size = `${OV_WIDTH}x${h}`;
  if (h > 0 && size !== ov.sent.size) {
    ov.sent.size = size;
    ov.askedAt = Date.now();
    windowAction(`overlay-size/${size}`);
  }
  const blur = s.see >= 45;
  if (blur !== ov.sent.blur) {
    ov.sent.blur = blur;
    windowAction(`overlay-blur/${blur ? "on" : "off"}`);
  }
  const through = ovPlaying();
  if (through !== ov.sent.through) {
    ov.sent.through = through;
    windowAction(`overlay-through/${through ? "on" : "off"}`);
  }
}

function drawOverlay() {
  document.documentElement.classList.add("alone", "overlay");
  if (STREAM) document.documentElement.classList.add("stream");
  // a redraw mid-drag would drop the slider under the pointer
  if (ov.holding) return;
  const card = overlayCard();
  if (app.firstChild !== card) app.replaceChildren(card);
  if (STREAM) card.style.zoom = innerWidth / OV_WIDTH;
  else ovFit(card);
}

if (STREAM) addEventListener("resize", () => render());

// a layer, moved or scaled by the pointer. its own pixels shift under the pointer once the app has
// moved it, so each step asks for where the pointer is now against where it was held, and the next
// waits until that's been done
function layerDrag(e, how) {
  e.preventDefault();
  const held = [e.clientX, e.clientY];
  const card = ov.card.getBoundingClientRect();
  const grip = card.right - e.clientX;
  let busy = false;
  const step = (ev) => {
    if (busy) return;
    const zoom = ov.set.zoom / 100;
    let ask;
    if (how === "move") {
      const [dx, dy] = [Math.round((ev.clientX - held[0]) * zoom), Math.round((ev.clientY - held[1]) * zoom)];
      if (!dx && !dy) return;
      ask = `overlay-move/${dx},${dy}`;
    } else {
      const next = Math.max(60, Math.min(300, Math.round(((ev.clientX + grip) * zoom / OV_WIDTH) * 100)));
      if (next === ov.set.zoom) return;
      ov.set.zoom = ov.sent.zoom = next;
      ask = `overlay-zoom/${next}`;
    }
    busy = true;
    fetch(`${BRIDGE}/window/${ask}`, { method: "POST" }).catch(() => {}).finally(() =>
      requestAnimationFrame(() => requestAnimationFrame(() => (busy = false))));
  };
  const done = () => {
    removeEventListener("pointermove", step);
    removeEventListener("pointerup", done);
    if (how === "scale") ovSave(() => {});
  };
  addEventListener("pointermove", step);
  addEventListener("pointerup", done);
}

if (OVERLAY && !STREAM) addEventListener("pointerup", () => ov.holding && ((ov.holding = false), render()));

if (OVERLAY && !STREAM && !LAYER) {
  // dragged bigger or smaller, from the corner or by the window manager: the width sets the scale
  // and, on the leaderboard, the height how many rows fit. the page's own pixels are the window's
  // over its zoom, so a window the size it was asked for is OV_WIDTH wide here
  let settle = 0;
  addEventListener("resize", () => {
    const asked = Number(ov.sent.size.split("x")[1]);
    const same = Math.abs(innerWidth - OV_WIDTH) < 2 && Math.abs(innerHeight - asked) < 2;
    if (!ov.dragging && (Date.now() - ov.askedAt < 800 || same)) return;
    ov.dragging = true;
    clearTimeout(settle);
    settle = setTimeout(() => {
      const s = ov.set;
      const scale = innerWidth / OV_WIDTH;
      const card = ov.card;
      const list = card?.querySelector(".ov-standings");
      if (list && card.classList.contains("view-board")) {
        const rest = card.getBoundingClientRect().height - list.offsetHeight;
        s.rows = Math.max(3, Math.min(30, Math.floor((innerHeight - rest) / OV_ROW_PX)));
      }
      s.zoom = Math.max(60, Math.min(300, Math.round(s.zoom * scale)));
      store(OV_KEY, s);
      ov.dragging = false;
      ov.sent.size = "";
      render();
    }, 300);
  });
}

// fresh plays go back to the small row after a while, with nothing else changing
if (OVERLAY) setInterval(() => snapshot && !ov.holding && ovFresh(snapshot.plays[0]) && render(), 30_000);

// Settings' stream overlay: the card's link for an OBS browser source, put together from what's picked
const STREAM_KEY = "hashi-stream";
const streamPick = { ...STREAM_DEFAULTS, ...stored(STREAM_KEY, {}) };
let streamCopiedAt = 0;

function streamLink() {
  const q = new URLSearchParams({ overlay: "stream" });
  for (const [k, v] of Object.entries(streamPick)) {
    if (k in STREAM_DEFAULTS && v !== STREAM_DEFAULTS[k] && !(k === "rows" && streamPick.view !== "board")) q.set(k, v);
  }
  return `${BRIDGE}/mania-bridge/?${q}`;
}

function setStream(change) {
  change(streamPick);
  store(STREAM_KEY, streamPick);
  render();
}

async function copyStreamLink(link, input) {
  try {
    await navigator.clipboard.writeText(link);
  } catch {
    input.select();
    document.execCommand("copy");
  }
  streamCopiedAt = Date.now();
  render();
  setTimeout(render, 1600);
}

function streamSection() {
  const p = streamPick;
  const row = (title, line, control, cls = "") => el("div", { class: `setting${cls}` },
    el("div", { class: "text" }, el("span", { class: "title" }, title), line && el("span", { class: "line" }, line)),
    control);
  const link = streamLink();
  const input = el("input", { class: "stream-url", readonly: true, value: link, "aria-label": "Stream overlay link", onfocus: (e) => e.target.select() });
  return [
    el("p", { class: "group" }, "Stream overlay"),
    row("Shows", null, segs([["now", "Map and plays"], ["board", "Leaderboard"]], p.view, (v) => setStream((x) => (x.view = v)), "What it shows")),
    p.view === "board" && row("Rows", null, segs([[5, "5"], [8, "8"], [10, "10"], [15, "15"]], p.rows, (v) => setStream((x) => (x.rows = v)), "Rows"), " sub"),
    row("Background", null, segs([[0, "None"], [50, "See-through"], [85, "Solid"]], p.bg, (v) => setStream((x) => (x.bg = v)), "Background")),
    row("Map art", null, el("button", {
      class: `switch${p.art ? " on" : ""}`, role: "switch", "aria-checked": String(!!p.art), "aria-label": "Map art",
      onclick: () => setStream((x) => (x.art = x.art ? 0 : 1)),
    }, el("i"))),
    row("While playing", null, segs([["show", "Show"], ["fade", "Fade"], ["hide", "Hide"]], p.playing, (v) => setStream((x) => (x.playing = v)), "While playing")),
    el("div", { class: "setting stream-link" },
      el("div", { class: "text" },
        el("span", { class: "title" }, "Link"),
        el("span", { class: "line" },
          "Add it in OBS as a Browser Source. Its width sets how big the card is, 400 is a good start. It keeps working with Hashi in the tray. ",
          el("a", { class: "link", href: link, target: "_blank", rel: "noreferrer" }, "Preview"))),
      el("div", { class: "stream-copy" },
        input,
        el("button", { class: "key go", onclick: () => copyStreamLink(link, input) }, Date.now() - streamCopiedAt < 1500 ? "Copied" : "Copy"))),
  ];
}
