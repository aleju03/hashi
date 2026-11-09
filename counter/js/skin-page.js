let skinPage = null;
const SKIN_KEYS = [4, 5, 6, 7, 8, 9, 10].map((k) => [k, `${k}K`]);
// the notes picked last, generated, long notes or the selected map; a play's own only for that play
const SKIN_SOURCE = "hashi-skin-source";
const SKIN_WHY = {
  lazer: "Couldn't read lazer's skin. In lazer, open the skin editor, then File, Edit externally, and it shows here while that's open.",
  builtin: "Lazer's built-in skins aren't shown here, only ones you've imported.",
  missing: "Can't find this skin's folder in osu!'s Skins.",
  unknown: "Open osu! with tosu running to see your skin here.",
};

// from: a play to watch (id), where in its map to start (map ms), and the analysis to go back to
function openSkin(from = null) {
  const back = from && analysis;
  settingsOpen = goalsOpen = notesOpen = reportOpen = false;
  profilePage = null;
  analysis = maker = null;
  dropTrainer();
  board = null;
  clearTimeout(boardTimer);
  const keys = Math.round((from ? from.keys : null) ?? snapshot.tosu.map?.keys ?? snapshot.plays[0]?.play.keys ?? 4);
  skinPage = {
    source: from ? "play" : ["mix", "lns", "selected"].find((s) => s === stored(SKIN_SOURCE, "mix")) ?? "mix", play: from, back, map: null,
    keys: keys >= 4 && keys <= 10 ? keys : 4, info: null, stamp: Date.now(), loading: true, error: null,
    // kept across renders so the running preview isn't torn down by every update
    canvas: el("canvas", { class: "skin-shot", width: 1280, height: 720, title: "Click to pause or play", onclick: () => skinPause(skinPage) }),
    fpsNode: el("span", { class: "skin-fps" }),
    live: null, playing: true, speed: stored(SKIN_SPEED, 24),
    get scroll() {
      return skinSpeedMs(this.speed, skinHit(this));
    },
  };
  render();
  loadSkin();
  watchSkin(skinPage);
}

// Edit externally starting or ending, or another skin picked in the game
async function watchSkin(page) {
  while (skinPage === page) {
    await new Promise((r) => setTimeout(r, 2000));
    if (skinPage !== page || document.hidden || page.loading || page.applying) continue;
    let info;
    try {
      info = await (await fetch(`${BRIDGE}/skin`)).json();
    } catch {
      continue;
    }
    if (skinPage !== page || page.loading || page.applying) continue;
    const was = page.info ?? {};
    if (info.found !== was.found || info.folder !== was.folder || info.why !== was.why) loadSkin();
    else if (JSON.stringify(info.gameSize) !== JSON.stringify(was.gameSize)) {
      page.info.gameSize = info.gameSize;
      if (page.live) refreshSkin(page);
      render();
    }
  }
}

function closeSkin() {
  const back = skinPage?.back;
  skinPage = null;
  if (back) analysis = back;
  render();
}

const skinSwappable = (p) => p.live && !p.loading && !SKIN_FREEZE && p.live.scene.columns.length === p.keys;

async function skinSource(p, source, picked = false) {
  p.want = source;
  if (picked && source !== "play") store(SKIN_SOURCE, source);
  if (source === "mix" || source === "lns") {
    p.source = source;
    p.map = p.error = p.noMap = null;
    if (!skinSwappable(p)) return showSkin(p);
    swapSkinChart(p);
    return render();
  }
  if (source === "selected") p.followed = snapshot.tosu.map ? boardKey(snapshot.tosu.map) : null;
  // a failed read leaves the preview as it was
  try {
    const query = source === "play" ? `?play=${encodeURIComponent(p.play.id)}` : "";
    const res = await fetch(`${BRIDGE}/skin/chart${query}`);
    const body = await res.json();
    if (skinPage !== p || p.want !== source) return;
    if (!res.ok) throw new Error(body.error);
    p.source = source;
    p.error = p.noMap = null;
    p.map = { ...body, from: source === "play" ? p.play.from : body.notes[0][1] };
    p.keys = body.keys;
    if (skinSwappable(p)) {
      swapSkinChart(p);
      return render();
    }
  } catch (e) {
    if (skinPage !== p || p.want !== source) return;
    p.noMap = e.message || "Couldn't read the map.";
    if (p.live && p.source !== source) return render();
    p.source = "mix";
    p.map = null;
  }
  showSkin(p);
}

// runs on every render; `followed` is set before the read, so a map that fails isn't read again
function skinFollow() {
  const p = skinPage;
  const selected = snapshot.tosu.map;
  if (!p || (p.want ?? p.source) !== "selected" || !selected || p.loading || !p.info?.found) return;
  if (boardKey(selected) === p.followed) return;
  skinSource(p, "selected");
}

async function loadSkin() {
  const page = skinPage;
  page.stamp = Date.now();
  page.loading = true;
  page.error = page.noMap = null;
  render();
  try {
    page.info = await (await fetch(`${BRIDGE}/skin`)).json();
    page.applied = page.info?.found ? await fetch(`${BRIDGE}/skin/applied`).then((r) => r.json()).catch(() => ({})) : {};
  } catch {
    page.error = "Couldn't reach Hashi's helper.";
  }
  if (skinPage !== page) return;
  page.edits ??= {};
  for (const [keys, a] of Object.entries(page.applied ?? {})) {
    if (!/^\d+$/.test(keys)) continue;
    if (a?.state && typeof a.state === "object" && !page.edits[keys]) page.edits[keys] = skinRestored(a.state);
  }
  page.wide ??= { percentWidth: page.applied?._accuracy?.percentWidth ?? null };
  if (page.info?.found) return ["mix", "lns"].includes(page.source) ? showSkin(page) : skinSource(page, page.source);
  page.loading = false;
  render();
}

async function showSkin(page) {
  const keys = page.keys;
  page.loading = true;
  render();
  let loaded = null;
  try {
    loaded = await loadSkinScene(page);
  } catch {}
  if (skinPage !== page || page.keys !== keys) return;
  page.loading = false;
  page.error = loaded ? null : "Couldn't draw this skin.";
  if (loaded) startSkinLive(page, loaded);
  render();
}

function skinScroll(p, by) {
  p.speed = Math.min(SKIN_SPEED_MAX, Math.max(SKIN_SPEED_MIN, p.speed + by));
  store(SKIN_SPEED, p.speed);
  // cuts and fades are set in ms, so their length on the skin follows the scroll time
  if (skinEditsJson(p)) refreshSkin(p);
  render();
}

// what the game size window follows, sent when it changes
function skinShare(p) {
  if (!p.info?.gameSize) return;
  const state = JSON.stringify({
    keys: p.keys, at: p.stamp, edits: p.live ? skinEditsJson(p) : "", source: p.source,
    play: p.source === "play" ? p.play?.id : null, from: p.map?.from ?? null, speed: p.speed, playing: p.playing,
  });
  if (state === p.shared) return;
  p.shared = state;
  fetch(`${BRIDGE}/skin/live?state=${encodeURIComponent(state)}`, { method: "POST" }).catch(() => {});
}

function gameSizeText(size) {
  return size === "screen" ? "Open in a full screen window" : `Open in a ${size.width}×${size.height} window`;
}

function skinView() {
  const p = skinPage;
  const info = p.info;
  skinShare(p);
  p.canvas.classList.toggle("loading", p.loading);
  p.canvas.setAttribute("aria-label", info?.found ? `${info.name} in ${p.keys}K` : "");
  return el("div", { class: "skin-page" },
    el("div", { class: "head" },
      el("p", { class: "say" }, "Skin editor"),
      el("button", { class: "link", onclick: closeSkin }, p.back ? "Back to the analysis" : "Back to plays")),
    info?.found && el("p", { class: "caption" }, info.name, info.client === "lazer" && " (lazer)"),
    info && !info.found && el("p", { class: "caption" }, SKIN_WHY[info.why] ?? SKIN_WHY.unknown),
    info?.found && info.unreadable?.length > 0 && !SKIN_FREEZE && skinUnreadable(p),
    p.error && el("p", { class: "status bad" }, p.error),
    info?.found && el("div", { class: "skin-stage" },
    el("div", { class: "skin-tools" },
      segs(SKIN_KEYS, p.keys, (keys) => {
        p.keys = keys;
        if (p.source !== "lns") p.source = "mix";
        p.map = p.noMap = null;
        p.want = p.source;
        showSkin(p);
      }, "Keys"),
      info.gameSize && el("button", {
        class: "link",
        title: "In a window the size osu! draws at, following your changes here",
        onclick: () => fetch(`${BRIDGE}/window/skin`, { method: "POST" }).catch(() => {}),
      }, gameSizeText(info.gameSize)),
      el("button", { class: "link", disabled: p.loading, onclick: loadSkin }, "Load again")),
    !SKIN_FREEZE && el("div", { class: "skin-tools" },
      segs([["mix", "Generated"], ["lns", "Long notes"], ["selected", "Selected map"], ...(p.play ? [["play", "This play"]] : [])], p.source, (s) => skinSource(p, s, true), "Notes"),
      p.noMap ? el("span", { class: "skin-map bad" }, p.noMap) : p.map && el("span", { class: "skin-map" },`${p.map.title ?? "Unknown"}${p.map.version ? ` [${p.map.version}]` : ""}`)),
    !SKIN_FREEZE && el("div", { class: "skin-tools" },
      el("div", { class: "segs" },
        el("button", { class: "seg", onclick: () => skinPause(p) }, p.playing ? "Pause" : "Play")),
      el("div", { class: "segs", title: "Scroll speed" },
        el("button", { class: "seg", disabled: p.speed <= SKIN_SPEED_MIN, "aria-label": "Slower", onclick: () => skinScroll(p, -1) }, "−"),
        el("span", { class: "skin-scroll" }, `Speed ${p.speed}`),
        el("button", { class: "seg", disabled: p.speed >= SKIN_SPEED_MAX, "aria-label": "Faster", onclick: () => skinScroll(p, 1) }, "+"))),
    el("div", { class: "skin-shot-box" }, p.canvas, p.fpsNode),
    // the game size window follows once it's let go
    !SKIN_FREEZE && skinSeekBar(p, () => render())),
    info?.found && p.live && !SKIN_FREEZE && skinEditor(p));
}
