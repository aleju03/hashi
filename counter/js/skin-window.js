// The game size window: the skin preview alone, pixel for pixel at the size osu! draws it, following
// the skin page in the main window.

async function skinWindowMap(st) {
  if (st.source !== "play" && st.source !== "selected") return null;
  const query = st.source === "play" && st.play ? `?play=${encodeURIComponent(st.play)}` : "";
  try {
    const res = await fetch(`${BRIDGE}/skin/chart${query}`);
    const body = await res.json();
    if (!res.ok || body.keys !== st.keys) return null;
    return { ...body, from: st.from ?? body.notes[0][1] };
  } catch {
    return null;
  }
}

function skinWindow() {
  document.documentElement.classList.add("skin-window");
  const [w, h] = (new URLSearchParams(location.search).get("skin-window") ?? "").split("x").map(Number);
  const canvas = el("canvas", { class: "skin-window-shot" });
  const note = el("p", { class: "skin-window-note", hidden: true });
  document.body.append(canvas, note);
  const page = { window: true, canvas, aspect: w && h ? w / h : null, keys: null, stamp: null, editsJson: null, source: null, map: null, scroll: 480, playing: true, seq: 0 };
  skinPage = page;

  // the largest the window holds at the game's shape, in whole screen pixels
  const fit = () => {
    const dpr = devicePixelRatio || 1;
    const [pw, ph] = [Math.round(innerWidth * dpr), Math.round(innerHeight * dpr)];
    const aspect = page.aspect ?? pw / ph;
    let [cw, ch] = [pw, Math.round(pw / aspect)];
    if (ch > ph) [cw, ch] = [Math.round(ph * aspect), ph];
    canvas.width = cw;
    canvas.height = ch;
    canvas.style.width = `${cw / dpr}px`;
    canvas.style.height = `${ch / dpr}px`;
    page.k = ch / (page.live?.scene.height ?? 720);
    if (page.live) {
      page.live.k = page.k;
      skinLayers(page.live);
    }
    const shown = w ? cw / w : 1;
    note.hidden = !w || shown > 0.99;
    note.textContent = `Shown at ${Math.round(shown * 100)}%: ${w}×${h} doesn't fit on this screen.`;
  };
  addEventListener("resize", fit);
  fit();

  const follow = async (st) => {
    if (!st?.keys) return;
    const seq = ++page.seq;
    page.scroll = st.scroll;
    page.playing = st.playing;
    const reload = st.keys !== page.keys || st.at !== page.stamp || st.edits !== page.editsJson;
    // the same map from somewhere else in it is the main window's seek bar
    const seek = page.live && page.map && st.source === page.source && st.play === page.playId && st.from !== page.from;
    if (seek) {
      page.from = st.from;
      skinSeek(page, st.from);
    }
    const notes = !seek && (st.source !== page.source || st.play !== page.playId || st.from !== page.from);
    if (notes) {
      const map = await skinWindowMap(st);
      if (seq !== page.seq) return;
      page.map = map;
      page.source = map || st.source === "lns" ? st.source : "mix";
      page.playId = st.play;
      page.from = st.from;
    }
    Object.assign(page, { keys: st.keys, stamp: st.at, editsJson: st.edits });
    if (!page.live || page.live.scene.columns.length !== st.keys) {
      let loaded = null;
      try {
        loaded = await loadSkinScene(page);
      } catch {}
      if (seq !== page.seq || !loaded) return;
      startSkinLive(page, loaded);
      fit();
      return;
    }
    if (reload) refreshSkin(page);
    if (notes) swapSkinChart(page);
  };

  fetch(`${BRIDGE}/skin/live`).then((r) => r.json()).then(follow).catch(() => {});
  const events = new EventSource(`${BRIDGE}/events`);
  events.addEventListener("skin", (ev) => follow(JSON.parse(ev.data)));
  events.addEventListener("reload", () => location.reload());
  addEventListener("keydown", (e) => e.key === "Escape" && windowAction("skin-close"));

  // says it's on screen every second, the main window's preview rests meanwhile
  const away = new BroadcastChannel(SKIN_AWAY);
  const beat = () => away.postMessage(document.hidden ? "closed" : "open");
  beat();
  setInterval(beat, 1000);
  document.addEventListener("visibilitychange", beat);
  addEventListener("pagehide", () => away.postMessage("closed"));
  // it follows the main window's preview, so a click pauses that one
  canvas.addEventListener("click", () => away.postMessage("pause"));

  // a seek bar over the bottom while the mouse moves, which the main window's follows
  skinSeekBar(page, (t) => {
    // the main window says the same back, which is already done here
    page.from = t;
    away.postMessage({ seek: t });
  });
  document.body.append(page.seek.box);
  let still = 0;
  addEventListener("pointermove", () => {
    document.documentElement.classList.add("moving");
    clearTimeout(still);
    still = setTimeout(() => document.documentElement.classList.remove("moving"), 1500);
  });
  setInterval(() => {
    const chart = page.live?.chart;
    if (page.map && chart?.at && !document.hidden) away.postMessage({ at: chart.at(page.live.now) });
  }, 500);
}
