// A play's analysis: loading it, its score card and tips, and how its parts are laid out.

let analysis = null;

const TIPS_SHOWN = 4;

function openAnalysis(p) {
  goalsOpen = false;
  skinPage = null;
  settingsOpen = notesOpen = false;
  board = null;
  clearTimeout(boardTimer);
  analysis = { id: p.id, play: p, data: null, error: null, lane: "presses", allTips: false, openedAt: Date.now(), back: profilePage };
  profilePage = null;
  loadAnalysis();
  render();
}

function closeAnalysis() {
  profilePage = analysis?.back ?? null;
  analysis = maker = null;
  dropTrainer();
  render();
}

async function loadAnalysis() {
  const want = analysis;
  if (!want) return;
  want.loading = true;
  want.thumb = snapshot.thumb;
  try {
    const res = await fetch(`${BRIDGE}/analysis/${encodeURIComponent(want.id)}`);
    const body = await res.json().catch(() => ({}));
    if (analysis !== want) return;
    if (res.ok) {
      want.data = body;
      want.error = null;
    } else {
      want.error = body.error ?? "Couldn't analyze this play.";
    }
  } catch {
    if (analysis === want) want.error = "Lost Hashi's helper.";
  }
  if (analysis !== want) return;
  want.loading = false;
  want.node = null;
  render();
}

// the thumb setting moves a column between hands, so hands and tips change with it
function analysisChanged() {
  if (analysis?.data && !analysis.loading && analysis.thumb !== snapshot.thumb && analysis.data.thumb) loadAnalysis();
}

function vizTip(root) {
  const tip = el("div", { class: "viz-tip", hidden: "" });
  root.append(tip);
  return {
    show(e, value, ...rest) {
      tip.replaceChildren(el("b", {}, value), ...rest.filter(Boolean).map((r) => el("span", {}, r)));
      tip.hidden = false;
      const box = root.getBoundingClientRect();
      const w = tip.offsetWidth;
      tip.style.left = `${Math.max(0, Math.min(box.width - w, e.clientX - box.left - w / 2))}px`;
      tip.style.top = `${e.clientY - box.top - tip.offsetHeight - 14}px`;
    },
    hide() {
      tip.hidden = true;
    },
  };
}

// which window an offset falls in, 1 MAX to 5 50, 6 past them
function windowOf(offset, windows) {
  const i = windows.findIndex((w) => Math.abs(offset) <= w);
  return i < 0 ? 6 : i + 1;
}

// SS means nothing below a Great, greats cost accuracy but not the SS
function gradeOf(d) {
  const silver = d.mods.some((m) => ["HD", "FL", "FI"].includes(m));
  const a = d.accuracy;
  const clean = d.counts.slice(2).every((n) => n === 0);
  const g = clean ? "X" : a > 95 ? "S" : a > 90 ? "A" : a > 80 ? "B" : a > 70 ? "C" : "D";
  return GRADES[silver && (g === "X" || g === "S") ? `${g}H` : g];
}

const GLYPH = {
  good: `<path d="M2.5 7.5l3 3 6-7"/>`,
  work: `<path d="M7 12V2.5M3 6.5l4-4 4 4"/>`,
};

const notePill = (cls = "") => `<svg class="pill ${cls}" viewBox="0 0 20 9" width="20" height="9" aria-hidden="true"><rect x="1" y="1" width="18" height="7" rx="3.5"/></svg>`;

function scorecard(d, tip, since) {
  const [grade, tone] = gradeOf(d);
  const tilt = -3 - (analysis.id.length % 4);
  const jot = (i) => `animation-delay: ${380 + i * 90 - since}ms`;
  const shown = analysis.allTips ? d.tips : d.tips.slice(0, TIPS_SHOWN);
  const facts = [
    [`${signedMs(d.timing.mean)} ms`, `average press, ${leanWord(d.timing.mean, 4)}`],
    [`±${msText(d.timing.sd)} ms`, `spread, UR ${Math.round(d.timing.ur)}`],
    d.release ? [`${signedMs(d.release.mean)} ms`, `average release, ${leanWord(d.release.mean, 12)}`] : [String(d.counts[5]), d.counts[5] === 1 ? "miss" : "misses"],
  ];
  const tally = el("ul", { class: "tally" }, d.names.map((n, i) => {
    const li = el("li", { class: `j${i + 1}` }, el("b", {}, d.counts[i]), el("span", {}, n));
    li.insertAdjacentHTML("afterbegin", notePill());
    return li;
  }));
  const card = el("article", { class: "letter scorecard", style: `animation-delay: -${since}ms` },
    el("div", { class: "top" },
      el("div", { class: `grade-stamp ${tone}`, style: `--tilt: ${tilt}deg` }, grade),
      el("div", { class: "score" },
        el("b", {}, pct(d.accuracy)),
        el("span", {}, Math.abs(d.accuracy - d.judged) >= 0.05 ? `Hashi counts ${pct(d.judged)}` : "accuracy"),
        // with presses on empty columns, the accuracy osu! gives can't show progress: this one can
        d.empty?.shown && el("span", { class: "clean", title: "Accuracy if every press on a column with no note were a miss" },
          `clean ${pct(d.empty.clean)}, goal ${d.empty.goal}%`,
          d.previous?.clean != null && el("em", { class: d.empty.clean >= d.previous.clean ? "up" : "down" }, ` ${deltaText(d.empty.clean - d.previous.clean)}`))),
      el("ul", { class: "stats" }, facts.map(([v, l]) => el("li", {}, el("b", {}, v), el("span", {}, l))))),
    tally,
    d.tips.length > 0 && [
      el("p", { class: "kind" }, "What stands out"),
      el("ul", { class: "changes remarks" }, shown.map((t, i) => {
        const glyph = el("span", { class: "glyph" });
        glyph.innerHTML = `<svg viewBox="0 0 14 14" width="14" height="14" aria-hidden="true">${GLYPH[t.tone] ?? GLYPH.work}</svg>`;
        return el("li", { class: t.tone, style: jot(i) },
          glyph,
          el("div", { class: "text" },
            el("span", { class: "title" }, t.title),
            el("span", { class: "line" }, t.text),
            (t.marked || t.drill) && el("div", { class: "jumps" },
              t.marked && el("button", { class: "link jump", onclick: showPractice }, "Show it on the chart"),
              t.marked && !d.practiceRun && el("button", { class: "link jump", onclick: () => openMaker(d) }, "Make a practice map"),
              t.marked && d.practice && el("button", { class: "link jump", onclick: () => openSkin({ id: analysis.id, keys: d.keys, from: (d.offset + d.practice.from) * 1000 * d.rate }) }, "Watch it in your skin"),
              t.drill && el("button", { class: "link jump", onclick: () => openTrainer(d, d.columns[t.drill.columns[0]].hand, t.drill.columns) }, "Practice it here"))),
          t.gain != null && el("span", {
            class: "gain",
            style: `--tilt: ${(i % 2 ? 3 : -4)}deg`,
            onpointerenter: (e) => tip.show(e, `${(d.judged + t.gain).toFixed(2)}%`, "this play's accuracy with that fixed"),
            onpointerleave: () => tip.hide(),
          }, `+${t.gain.toFixed(2)}%`));
      })),
      d.tips.length > TIPS_SHOWN && el("button", {
        class: "link more",
        onclick: () => {
          analysis.allTips = !analysis.allTips;
          analysis.node = null;
          render();
        },
      }, analysis.allTips ? "Fewer" : `${d.tips.length - TIPS_SHOWN} more`),
    ]);
  return card;
}

function showPractice() {
  const chart = document.querySelector(".area-play");
  chart?.scrollIntoView({ behavior: STILL.matches ? "auto" : "smooth", block: "start" });
  chart?.classList.remove("flash");
  requestAnimationFrame(() => chart?.classList.add("flash"));
}

function guide(text) {
  const node = el("p", { class: "caption guide" }, text, " ", el("button", {
    class: "link got-it",
    onclick: () => {
      node.classList.remove("open");
      node.closest(".analysis-body")?.classList.add("guided");
      if (!snapshot.guideRead) action("guide/read");
    },
  }, "Got it"));
  node.toggle = el("button", { class: "help", title: "What this shows", "aria-label": "What this shows", onclick: () => node.classList.toggle("open") }, "?");
  return node;
}

const KEY_GAP = 0.2;
const FINGER_CYCLE = ["index", "middle", "ring", "pinky", "thumb"];

async function pickFinger(d, column) {
  const next = FINGER_CYCLE[(FINGER_CYCLE.indexOf(d.columns[column].finger) + 1) % FINGER_CYCLE.length];
  await action(`finger/${d.keys}/${column}/${next}`);
  if (profilePage) loadProfile();
  else loadAnalysis();
}

const sameDay = (a, b) => a.toDateString() === b.toDateString();
const whenText = (iso) => (sameDay(new Date(iso), new Date()) ? `at ${time(iso)}` : `on ${new Date(iso).toLocaleDateString("en-US", { month: "short", day: "numeric" })}, ${time(iso)}`);
const deltaText = (x) => `${x >= 0 ? "+" : "−"}${Math.abs(x).toFixed(1)}`;

// wide windows lay the analysis out like a desktop app, see .roomy in the stylesheet
const ROOMY = matchMedia("(min-width: 980px)");

// keeps each chart's last height until it's redrawn, an empty slot for a frame scrolls the page
function slot(make, key) {
  const node = el("div", { class: "slot" });
  const kept = analysis.heights?.[key];
  if (kept) node.style.minHeight = `${kept}px`;
  node.fill = () => {
    const w = Math.round(node.clientWidth);
    if (!w || w === node.drawn) return;
    node.drawn = w;
    node.replaceChildren(...[make(Math.max(260, w))].flat().filter(Boolean));
    node.style.minHeight = "";
    (analysis.heights ??= {})[key] = node.offsetHeight;
  };
  return node;
}

function compareNote(d) {
  const p = d.previous;
  if (!p) return null;
  const rate = (r) => `${+Number(r).toFixed(2)}×`;
  const change = p.rate > d.rate ? "slower" : "faster";
  return el("p", { class: "caption compare" },
    `The small numbers are the change since your play on this map ${whenText(p.capturedAt)}, ${pct(p.accuracy)}.`,
    p.rate !== d.rate && ` That one was at ${rate(p.rate)} and this one at ${rate(d.rate)}. A ${change} rate moves every key, so compare the keys against each other more than the numbers.`);
}

function analysisBody(d) {
  const root = el("div", { class: `analysis-body${analysis.settled ? " settled" : ""}` });
  // the entrance plays once: redraws put this node back in the page, which would restart it
  if (!analysis.settled) setTimeout(() => root.classList.add("settled"), 1800);
  analysis.settled = true;
  const tip = vizTip(root);
  const since = Date.now() - analysis.openedAt;
  const releases = d.release && d.releases.length > 0;
  let plot = scatter(d, tip);
  const frame = el("div", { class: "frame" }, plot);
  // only the plot changes: rebuilding the body empties the charts above it for a frame, and the page jumps
  const lanes = () => segs([["presses", "Presses"], ["releases", "Releases"]], analysis.lane, (lane) => {
    analysis.lane = lane;
    plot = scatter(d, tip);
    frame.replaceChildren(plot);
    plot.draw();
    laneSegs.replaceWith(laneSegs = lanes());
  }, "What the plot shows");
  let laneSegs = releases && lanes();
  const slots = [
    slot((w) => stageView(d, w, tip, ROOMY.matches ? 250 : 200), "keys"),
    slot((w) => histogram(d.hits.filter((h) => h[2] <= 5).map((h) => h[1]), d.windows, { width: w, reach: d.windows[3], label: "How early or late each press was", tip, names: d.names }), "timing"),
    slot((w) => riverView(d, w, tip), "flow"),
    releases && slot((w) => histogram(d.releases.map((r) => r[1]), d.windows.map((x) => x * 1.5), { width: w, reach: d.windows[3] * 1.5, label: "How early or late each long note was let go", tip, names: d.names, plain: d.mode === "stable" }), "holds"),
  ];
  const [keys, timing, flow, holds] = slots;
  if (snapshot.guideRead) root.classList.add("guided");
  const keysGuide = guide(`The wider a row, the more hits landed at that timing: above the line is early, below is late. The colors are the judgements, the same as the counts on the score card. The outlined note is the key's average${d.tapHold != null ? ", and the number under each key is how long you hold a tap" : ""}.${d.keys > 1 ? ` Keys go from green on your best one through yellow and orange to red on your worst, and keys within ${KEY_GAP}% of each other get the same color.` : ""}`);
  const playGuide = guide("The pink line is the accuracy of each few seconds counted on its own, the water is notes per second, and each note on the bottom is a miss.");
  root.append(...[
    d.goal && goalCard(d.goal),
    d.practiceRun && practiceCard(d),
    scorecard(d, tip, since),
    part("area-keys", heading(["Your keys", keysGuide.toggle], d.thumb && segs([["left", "Left thumb"], ["right", "Right thumb"]], snapshot.thumb, (side) => action(`thumb/${side}`), "The hand whose thumb plays the middle column")),
      keys,
      compareNote(d),
      canTrain(d) && el("button", { class: "link trainer-open", onclick: () => openTrainer(d, "left") }, "Practice chords in the chord trainer"),
      keysGuide),
    part("area-play", heading(["Over the play", playGuide.toggle], laneSegs),
      frame,
      flow,
      playGuide),
    el("div", { class: "trio" },
      part("area-timing", heading("Timing"),
        timing,
        el("p", { class: "caption" }, `${Math.round(d.timing.early * 100)}% early, ${Math.round((1 - d.timing.early) * 100)}% late. Median ${signedMs(d.timing.median)} ms.`)),
      releases && part("area-holds", heading("Long notes"),
        holds,
        el("p", { class: "caption" },
          `${Math.round(d.release.early * 100)}% let go early, spread ±${msText(d.release.sd)} ms.`,
          d.release.breaks > 0 ? ` ${d.release.breaks} of ${d.release.holds} let go in the middle.` : "")),
      part("area-patterns", heading("Patterns"), patternsView(d), d.chordSpread != null && el("p", { class: "caption" }, `A chord's keys land ${msText(d.chordSpread)} ms apart on average.`))),
  ].filter(Boolean));
  root.layout = () => {
    for (const s of slots) s && s.fill();
    plot.draw();
  };
  return root;
}

function analysisView() {
  const a = analysis;
  const p = a.play.play;
  const d = a.data;
  const diff = diffName(p.keys, p.version);
  const mods = modList(p.mods);
  const head = el("div", { class: "head" },
    el("p", { class: "say" }, "Analysis"),
    el("button", { class: "link", onclick: closeAnalysis }, a.back ? "Back to profile" : "Back to plays"));
  const about = el("div", { class: "map" },
    a.play.url
      ? el("a", { class: "title", href: a.play.url, target: "_blank", rel: "noreferrer", title: "Open on Mania Tracker" }, p.title ?? "Unknown")
      : el("span", { class: "title" }, p.title ?? "Unknown"),
    p.artist && el("span", { class: "artist" }, p.artist),
    el("div", { class: "facts" },
      diff && el("span", {}, diff),
      mods.length > 0 && el("span", { class: "mods" }, mods.map((m) => modBadge(m, { rate: d?.rate }))),
      el("span", {}, time(a.play.capturedAt))));
  let body;
  if (a.error) {
    body = el("p", { class: "status bad" }, a.error);
  } else if (!d) {
    body = el("p", { class: "doing" }, el("i", { class: "spinner", style: `animation-delay: -${Date.now() % SPIN_MS}ms` }), "Reading the replay");
  } else {
    if (!a.node || a.roomy !== ROOMY.matches) {
      a.node = analysisBody(d);
      a.roomy = ROOMY.matches;
    }
    a.node.classList.toggle("stale", !!a.loading);
    body = a.node;
    requestAnimationFrame(() => a.node?.layout?.());
  }
  return el("div", { class: "analysis" }, head, about, body);
}
