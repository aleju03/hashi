// ?overlay: the window as a small card that stays over the game. ?overlay=stream: the same card for an
// OBS browser source, set up by its link, never touching the window, scaled to the source's width
const OVERLAY = new URLSearchParams(location.search).has("overlay");
const STREAM = new URLSearchParams(location.search).get("overlay") === "stream";
const PIP = `<rect x=".5" y="1.5" width="9" height="7" rx="1"/><rect x="5" y="5" width="3" height="2" fill="currentColor" stroke="none"/>`;
const DISC = `<circle cx="5" cy="5" r="4.5"/><circle cx="5" cy="5" r="1.3"/>`;
const GRIP = `<path d="M9.5 4.5l-5 5M9.5 7.5l-2 2" stroke-linecap="round"/>`;
const OV_KEY = "hashi-overlay";
// the card is laid out this wide, and made bigger or smaller as a whole
const OV_WIDTH = 340;
// .ov-standing's height in overlay.css
const OV_ROW_PX = 30;
// below this the card is mostly see-through: nothing blurred behind it, a shadow under the text instead
const OV_THIN_SEE = 45;
const OV_ZOOM_RANGE = [60, 300];
const OV_ROWS_RANGE = [3, 30];
const OV_PLAYING = [["show", "Show"], ["fade", "Fade"], ["hide", "Hide"]];
const OV_ROWS = [[5, "5"], [8, "8"], [10, "10"], [15, "15"]];
// a play stays up front this long after it's done
const OV_FRESH_MS = 3 * 60_000;

// what a stream link can say, and what it means when it doesn't
const STREAM_DEFAULTS = { view: "now", bg: 85, art: 1, playing: "show", rows: 8, live: 1 };

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
    playing: OV_PLAYING.some(([k]) => k === q.get("playing")) ? q.get("playing") : STREAM_DEFAULTS.playing,
    rows: Math.round(num("rows", ...OV_ROWS_RANGE)),
    live: num("live", 0, 1) === 1,
  };
}

const ov = {
  set: STREAM ? streamSettings() : { see: 90, zoom: 100, rows: 8, art: true, status: true, live: true, playing: "fade", view: "now", ...stored(OV_KEY, {}) },
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
  // the leaderboard rows last shown, kept up dimmed while the next map's are loading
  rows: null,
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

function ovSave(change = () => {}) {
  change(ov.set);
  if (!STREAM) store(OV_KEY, ov.set);
  render();
}

const ovPlaying = () => snapshot?.reader.state === "play";

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
  const acc = playAcc(p.play);
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
    p.live && ov.set.live && ovLive(p.live),
    ["fail", "wait"].includes(status.kind) && status.detail && el("p", { class: "why" }, status.detail));
}

function ovLast(p) {
  const status = playStatus(p);
  const acc = playAcc(p.play);
  const mods = modList(p.play.mods);
  return el("div", { class: "ov-last" },
    el("span", { class: "label" }, "Last"),
    el("span", { class: "title", title: `${p.play.artist ?? ""} - ${chartName(p.play)}` }, p.play.title ?? "Unknown"),
    mods.length > 0 && el("span", { class: "mods" }, mods.map((m) => modBadge(m, { rate: p.play.rate }))),
    acc && el("b", {}, acc),
    // whether it was sent is the player's business, not the stream's
    !STREAM && el("span", { class: `dot ${status.kind}`, title: status.label }));
}

const FINGER_SHORT = { thumb: "th", index: "ix", middle: "md", ring: "rg", pinky: "pk" };

function ovLive(live) {
  const hand = (side, title) => el("span", { class: "hand" },
    el("span", { class: "label" }, title),
    el("b", {}, live.hands[side] == null ? "–" : `${accText(live.hands[side])}%`));
  const column = (c, i) => el("span", { class: `col${c.acc == null ? " none" : ""}`, title: `Column ${i + 1}, ${c.hand} ${c.finger}${c.misses ? `, ${c.misses} ${c.misses === 1 ? "miss" : "misses"}` : ""}` },
    el("b", {}, c.acc == null ? "–" : accText(c.acc, 1)),
    // full from 90% up
    el("i", { style: `--fill: ${c.acc == null ? 0 : Math.max(0, Math.min(1, (c.acc - 90) / 10))}` }),
    el("small", {}, FINGER_SHORT[c.finger] ?? ""));
  const groups = [];
  live.columns.forEach((c, i) => {
    if (!groups.length || groups.at(-1)[0].c.hand !== c.hand) groups.push([]);
    groups.at(-1).push({ c, i });
  });
  return el("div", { class: "ov-live" },
    el("div", { class: "hands" }, hand("left", "Left hand"), hand("right", "Right hand")),
    el("div", { class: "cols" }, groups.map((g) => el("span", { class: "group", style: `flex: ${g.length}` }, g.map(({ c, i }) => column(c, i))))));
}

function ovNow(map) {
  const last = snapshot.plays[0];
  const playing = ovPlaying();
  const fresh = !playing && ovFresh(last);
  const live = playing && ov.set.live && snapshot.reader.live;
  return [
    map ? ovMap(map) : !STREAM && el("p", { class: "ov-empty" }, snapshot.reader.osu ? "Select a mania map in osu! to see it here." : "Open osu! to see your map here."),
    live ? ovLive(live) : fresh ? ovResult(last, map) : last && !playing && ovLast(last),
  ];
}

// on stream nobody clicks, so a row there never opens
function ovStanding(e, mine, still = false) {
  const [grade, tone] = GRADES[e.grade] ?? [e.grade, "d"];
  const open = !still && !STREAM && board.open === boardRowKey(e);
  return [el("li", { class: `ov-standing${e.username === snapshot.connection.username ? " me" : ""}${mine ? " mine" : ""}${open ? " open" : ""}`, onclick: !still && !STREAM && ((ev) => toggleBoardRow(e, ev)) },
    el("span", { class: "pos" }, `#${e.position ?? "?"}`),
    grade && el("span", { class: `grade ${tone}` }, grade),
    lookFace("avatar", e.avatarUrl && rowImg(`avatar ${e.userId ?? e.username}`, { src: e.avatarUrl, alt: "" }), e.look?.frame, 20, { key: `ov row ${e.userId}` }),
    el("span", { class: "player", style: e.avatarAccent && `--name: ${e.avatarAccent}` }, styledName(e.username, e.look?.name)),
    el("span", { class: "mods" }, e.mods.filter((m) => m !== "CL").map((m) => modBadge(m, { rate: e.rate }))),
    el("span", { class: "acc" }, typeof e.accuracy === "number" ? `${accText(e.accuracy)}%` : "")),
  open && el("li", { class: "ov-standing-detail" }, scoreDetail(e))];
}

function ovBoard(map) {
  if (!map) return el("p", { class: "ov-empty" }, "Select a mania map in osu! to see its leaderboard.");
  if (!board || boardKey(board) !== boardKey(map)) setBoard(map, true);
  else boardChanged();
  const { data } = board;
  const head = el("div", { class: "ov-board-head" },
    el("span", { class: "title" }, map.title ?? "Unknown"),
    data?.entries?.length > 0 && el("span", { class: "count" }, `${data.total} ${data.total === 1 ? "player" : "players"}`));
  let body;
  if (board.error) body = el("p", { class: "ov-empty" }, board.error);
  // the last board's rows, dimmed, until this one's come: the card keeps its height from map to map
  else if (!data) body = ov.rows ? el("ol", { class: "ov-standings stale", "aria-label": "Loading" }, ov.rows.map((e) => ovStanding(e.e, e.mine, true)))
    : el("ol", { class: "ov-standings", "aria-label": "Loading" }, Array.from({ length: ov.set.rows }, () => el("li", { class: "ov-standing" })));
  else if (data.unsupported || data.unknown) body = el("p", { class: "ov-empty" }, "Mania Tracker has no leaderboard for this map.");
  else if (!data.entries.length) body = el("p", { class: "ov-empty" }, "Nobody has played this map yet.");
  else {
    const mine = data.entries.findIndex((e) => e.username === snapshot.connection.username);
    const room = Math.max(1, ov.set.rows - (data.self && mine < 0 ? 1 : 0));
    const last = Math.max(0, data.entries.length - room);
    board.from = Math.max(0, Math.min(board.from, last));
    // a row per row's height of wheel, so a trackpad's small steps add up
    const wheel = (e) => {
      e.preventDefault();
      if (!e.deltaY) return;
      if (Math.sign(e.deltaY) !== Math.sign(board.wheel)) board.wheel = 0;
      board.wheel += e.deltaY;
      const rows = Math.trunc(board.wheel / OV_ROW_PX);
      board.wheel -= rows * OV_ROW_PX;
      const to = Math.max(0, Math.min(board.from + rows, last));
      if (to === board.from) return;
      board.from = to;
      render();
    };
    ov.rows = [...data.entries.slice(board.from, board.from + room).map((e) => ({ e })), ...(data.self && mine < 0 ? [{ e: data.self, mine: true }] : [])];
    body = el("ol", { class: "ov-standings", onwheel: wheel }, ov.rows.map((r) => ovStanding(r.e, r.mine)));
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
    onchange: () => ovSave(),
  });
  return el("div", { class: "ov-settings" },
    row("Background", see),
    row("Status line", switchButton(s.status, "Status line", () => ovSave((x) => (x.status = !x.status)))),
    row("Map art", switchButton(s.art, "Map art", () => ovSave((x) => (x.art = !x.art)))),
    row("While playing", segs(OV_PLAYING, s.playing, (p) => ovSave((x) => (x.playing = p)), "While playing")),
    row("Live accuracy", switchButton(s.live, "Live accuracy", () => ovSave((x) => (x.live = !x.live)))),
    row("Leaderboard rows", segs(OV_ROWS, s.rows, (r) => ovSave((x) => (x.rows = r)), "Leaderboard rows")),
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
    // the card moves it, as a title bar would, except its leaderboard rows, which open
    onmousedown: (e) => {
      if (STREAM || e.button !== 0 || e.target.closest("button, a, input, .ov-standing, .ov-standing-detail")) return;
      windowAction("drag");
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
  const map = snapshot.reader.map;
  const newest = snapshot.plays[0]?.id;
  const flash = ov.newest !== undefined && newest && newest !== ov.newest;
  ov.newest = newest ?? null;
  const art = ovArt(map);
  const view = ov.open ? "settings" : ov.set.view === "board" && snapshot.connection.username ? "board" : "now";
  const playing = ovPlaying();
  // on stream, nothing at all rather than a card saying there's nothing
  const empty = STREAM && (view === "board" ? !map : !map && !(ovFresh(snapshot.plays[0]) && !playing));
  card.className = `ov view-${view}${playing ? ` playing ${ov.set.playing}` : ""}${ov.set.see < OV_THIN_SEE ? " thin" : ""}${empty ? " empty" : ""}${ov.set.status || ov.open ? "" : " quiet"}${card.classList.contains("flash") ? " flash" : ""}`;
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
      onmousedown: (e) => e.button === 0 && windowAction("resize"),
    }, icon(GRIP))];
  card.replaceChildren(...parts.flat(Infinity).filter((c) => c != null && c !== false));
  return card;
}

// the window is sized to the card at its zoom, as tall as what it shows
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
  const blur = s.see >= OV_THIN_SEE;
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
  if (STREAM) document.documentElement.classList.add("overlay-stream");
  // a redraw mid-drag would drop the slider under the pointer
  if (ov.holding) return;
  artZoom = STREAM ? innerWidth / OV_WIDTH : ov.set.zoom / 100;
  const card = overlayCard();
  if (app.firstChild !== card) app.replaceChildren(card);
  if (STREAM) card.style.zoom = innerWidth / OV_WIDTH;
  else ovFit(card);
}

if (STREAM) addEventListener("resize", () => render());

if (OVERLAY && !STREAM) addEventListener("pointerup", () => ov.holding && ((ov.holding = false), render()));

if (OVERLAY && !STREAM) {
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
        s.rows = Math.max(OV_ROWS_RANGE[0], Math.min(OV_ROWS_RANGE[1], Math.floor((innerHeight - rest) / OV_ROW_PX)));
      }
      s.zoom = Math.max(OV_ZOOM_RANGE[0], Math.min(OV_ZOOM_RANGE[1], Math.round(s.zoom * scale)));
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
    p.view === "board" && row("Rows", null, segs(OV_ROWS, p.rows, (v) => setStream((x) => (x.rows = v)), "Rows"), " sub"),
    row("Background", null, segs([[0, "None"], [50, "See-through"], [85, "Solid"]], p.bg, (v) => setStream((x) => (x.bg = v)), "Background")),
    row("Map art", null, switchButton(!!p.art, "Map art", () => setStream((x) => (x.art = x.art ? 0 : 1)))),
    row("While playing", null, segs(OV_PLAYING, p.playing, (v) => setStream((x) => (x.playing = v)), "While playing")),
    p.view === "now" && row("Live accuracy", "Each hand's and column's accuracy during a play.", switchButton(!!p.live, "Live accuracy", () => setStream((x) => (x.live = x.live ? 0 : 1)))),
    el("div", { class: "setting stream-link" },
      el("div", { class: "text" },
        el("span", { class: "title" }, "Link"),
        el("span", { class: "line" },
          "Add it in OBS as a Browser Source. Its width sets how big the card is, 400 is a good start. It keeps working while the window is in the tray. ",
          el("a", { class: "link", href: link, target: "_blank", rel: "noreferrer" }, "Preview"))),
      el("div", { class: "stream-copy" },
        input,
        el("button", { class: "key go", onclick: () => copyStreamLink(link, input) }, Date.now() - streamCopiedAt < 1500 ? "Copied" : "Copy"))),
  ];
}
