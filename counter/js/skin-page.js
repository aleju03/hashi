// The skin page around the preview in skin.js: picking keys, the skin's source and watching it change.

let skinPage = null;
const SKIN_KEYS = [4, 5, 6, 7, 8, 9, 10].map((k) => [k, `${k}K`]);
const SKIN_WHY = {
  lazer: "Hashi couldn't read lazer's skin. In lazer, open the skin editor, then File, Edit externally, and Hashi shows it while that's open.",
  builtin: "Lazer's built-in skins aren't shown here, only ones you've imported.",
  missing: "Hashi can't find this skin's folder in osu!'s Skins.",
  unknown: "Open osu! with tosu running to see your skin here.",
};

// from: a play to watch (id), where in its map to start (map ms), and the analysis to go back to
function openSkin(from = null) {
  const back = from && analysis;
  settingsOpen = goalsOpen = notesOpen = false;
  profilePage = null;
  analysis = maker = null;
  dropTrainer();
  board = null;
  clearTimeout(boardTimer);
  const keys = Math.round(snapshot.plays[0]?.play.keys ?? 4);
  skinPage = {
    source: from ? "play" : "mix", play: from, back, map: null,
    keys: keys >= 4 && keys <= 10 ? keys : 4, info: null, stamp: Date.now(), loading: true, error: null,
    // kept across renders so the running preview isn't torn down by every update
    canvas: el("canvas", { class: "skin-shot", width: 1280, height: 720 }),
    fpsNode: el("span", { class: "skin-fps" }),
    live: null, playing: true, scroll: stored(SKIN_SCROLL, 480),
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
  }
}

function closeSkin() {
  const back = skinPage?.back;
  skinPage = null;
  if (back) analysis = back;
  render();
}

const skinSwappable = (p) => p.live && !p.loading && !SKIN_FREEZE && p.live.scene.columns.length === p.keys;

async function skinSource(p, source) {
  p.want = source;
  if (source === "mix" || source === "lns") {
    p.source = source;
    p.map = p.error = p.noMap = null;
    if (!skinSwappable(p)) return showSkin(p);
    swapSkinChart(p);
    return render();
  }
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

async function loadSkin() {
  const page = skinPage;
  page.stamp = Date.now();
  page.loading = true;
  page.error = page.noMap = null;
  render();
  try {
    page.info = await (await fetch(`${BRIDGE}/skin`)).json();
  } catch {
    page.error = "Couldn't reach Hashi's helper.";
  }
  if (skinPage !== page) return;
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
  p.scroll = Math.min(SKIN_SCROLL_MAX, Math.max(SKIN_SCROLL_MIN, p.scroll + by));
  store(SKIN_SCROLL, p.scroll);
  // cuts and fades are set in ms, so their length on the skin follows the scroll time
  if (skinEditsJson(p)) refreshSkin(p);
  render();
}

function skinView() {
  const p = skinPage;
  const info = p.info;
  p.canvas.classList.toggle("loading", p.loading);
  p.canvas.setAttribute("aria-label", info?.found ? `${info.name} in ${p.keys}K` : "");
  return el("div", { class: "skin-page" },
    el("div", { class: "head" },
      el("p", { class: "say" }, "Your skin"),
      el("button", { class: "link", onclick: closeSkin }, p.back ? "Back to the analysis" : "Back to plays")),
    info?.found && el("p", { class: "caption" }, info.name, info.client === "lazer" && " (lazer)"),
    info && !info.found && el("p", { class: "caption" }, SKIN_WHY[info.why] ?? SKIN_WHY.unknown),
    info?.found && info.unreadable?.length > 0 && !SKIN_FREEZE && skinUnreadable(p),
    p.error && el("p", { class: "status bad" }, p.error),
    // the preview and its controls, kept in view beside the editor on a wide window
    info?.found && el("div", { class: "skin-stage" },
    el("div", { class: "skin-tools" },
      segs(SKIN_KEYS, p.keys, (keys) => {
        p.keys = keys;
        if (p.source !== "lns") p.source = "mix";
        p.map = p.noMap = null;
        p.want = p.source;
        showSkin(p);
      }, "Keys"),
      el("button", { class: "link", disabled: p.loading, onclick: loadSkin }, "Load again")),
    !SKIN_FREEZE && el("div", { class: "skin-tools" },
      segs([["mix", "Generated"], ["lns", "Long notes"], ["selected", "Selected map"], ...(p.play ? [["play", "This play"]] : [])], p.source, (s) => skinSource(p, s), "Notes"),
      p.noMap ? el("span", { class: "skin-map bad" }, p.noMap) : p.map && el("span", { class: "skin-map" },`${p.map.title ?? "Unknown"}${p.map.version ? ` [${p.map.version}]` : ""}`)),
    !SKIN_FREEZE && el("div", { class: "skin-tools" },
      el("div", { class: "segs" },
        el("button", { class: "seg", onclick: () => { p.playing = !p.playing; render(); } }, p.playing ? "Pause" : "Play")),
      el("div", { class: "segs", title: "How long a note takes from the top to the judgement line" },
        el("button", { class: "seg", disabled: p.scroll <= SKIN_SCROLL_MIN, "aria-label": "Faster", onclick: () => skinScroll(p, -SKIN_SCROLL_STEP) }, "−"),
        el("span", { class: "skin-scroll" }, `${p.scroll} ms`),
        el("button", { class: "seg", disabled: p.scroll >= SKIN_SCROLL_MAX, "aria-label": "Slower", onclick: () => skinScroll(p, SKIN_SCROLL_STEP) }, "+"))),
    el("div", { class: "skin-shot-box" }, p.canvas, p.fpsNode)),
    info?.found && p.live && !SKIN_FREEZE && skinEditor(p));
}
