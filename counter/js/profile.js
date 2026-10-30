// Your profile, made from the plays Hashi keeps.

let profilePage = null;

function openProfile() {
  settingsOpen = goalsOpen = notesOpen = false;
  analysis = maker = null;
  dropTrainer();
  skinPage = null;
  board = null;
  clearTimeout(boardTimer);
  profilePage = { data: null, error: null, keys: null, node: null, openedAt: Date.now() };
  render();
  loadProfile();
}

function closeProfile() {
  profilePage = null;
  render();
}

async function loadProfile() {
  const page = profilePage;
  try {
    const res = await fetch(`${BRIDGE}/profile`);
    if (!res.ok) throw new Error();
    page.data = await res.json();
    page.error = null;
  } catch {
    page.error = "Couldn't reach Hashi's helper.";
  }
  page.node = null;
  if (profilePage === page) render();
}

const dayKey = (iso) => new Date(iso).toDateString();
const shortDate = (date) => date.toLocaleDateString("en-US", { month: "short", day: "numeric" });

function playedText(seconds) {
  const minutes = Math.round(seconds / 60);
  return minutes < 90 ? `${minutes} min` : `${(minutes / 60).toFixed(1)} h`;
}

// the charts are drawn for the width they get, so they wait until they're on the page
function fitted(make) {
  const node = el("div", { class: "slot" });
  node.fill = () => {
    const w = Math.round(node.clientWidth);
    if (!w || w === node.drawn) return;
    node.drawn = w;
    node.replaceChildren(...[make(Math.max(260, w))].flat().filter(Boolean));
  };
  return node;
}

// one square per day Hashi keeps plays for, today last
function stampSheet(days, keepDays, tip) {
  const today = new Date();
  today.setHours(0, 0, 0, 0);
  const cells = [];
  for (let i = keepDays - 1; i >= 0; i--) {
    const date = new Date(today);
    date.setDate(today.getDate() - i);
    const day = days.get(date.toDateString());
    const n = day?.plays ?? 0;
    const level = n === 0 ? 0 : n === 1 ? 1 : n <= 3 ? 2 : 3;
    const cell = el("span", {
      class: `day l${level}${i === 0 ? " today" : ""}`,
      style: `--tilt: ${((i * 37) % 13) - 6}deg; --wobble: url(#ink${(i % 3) + 1})`,
    }, date.getDate());
    if (n) {
      const show = (e) => tip.show(e, `${n} ${n === 1 ? "play" : "plays"}`, shortDate(date), day.acc != null && `${pct(day.acc)} on average`);
      cell.addEventListener("pointerenter", show);
      cell.addEventListener("pointermove", show);
      cell.addEventListener("pointerleave", () => tip.hide());
    }
    cells.push(cell);
  }
  const first = new Date(today);
  first.setDate(today.getDate() - keepDays + 1);
  return el("div", { class: "sheet" },
    el("div", { class: "days" }, cells),
    el("div", { class: "sheet-ends" }, el("span", {}, shortDate(first)), el("span", {}, "today")));
}

function idCard(c, d, days, tip) {
  const real = d.plays.filter((p) => !p.practice);
  const seconds = d.plays.reduce((s, p) => s + (p.read?.length ?? 0), 0);
  const notes = d.plays.reduce((s, p) => s + (p.read?.notes ?? 0), 0);
  const first = d.plays.at(-1)?.capturedAt;
  const main = d.modes.reduce((a, b) => (!a || b.plays > a.plays ? b : a), null);
  return el("section", { class: "panel id-card" },
    el("div", { class: "id-top" },
      el("span", { class: "id-face" }, c.avatarUrl ? el("img", { src: c.avatarUrl, alt: "" }) : c.username[0].toUpperCase()),
      el("div", { class: "id-who" },
        el("span", { class: "id-kind" }, "Player card"),
        el("b", { class: "id-name" }, c.username),
        el("span", { class: "id-since" }, first ? `With Hashi since ${shortDate(new Date(first))}` : "No plays yet")),
      main && el("span", { class: "id-keys", style: "--tilt: 9deg" }, `${main.keys}K`)),
    el("ul", { class: "id-facts" },
      [[real.length, real.length === 1 ? "play" : "plays"], [days.size, days.size === 1 ? "day" : "days"], [playedText(seconds), "played"], [notes.toLocaleString("en-US"), "notes"]]
        .map(([v, l]) => el("li", {}, el("b", {}, v), el("span", {}, l)))),
    stampSheet(days, d.keepDays, tip));
}

// the river from the analysis, a day at a time: accuracy above, how much you played as the water
function dayRiver(m, plays, keepDays, width, tip) {
  const today = new Date();
  today.setHours(0, 0, 0, 0);
  const days = [];
  for (let i = keepDays - 1; i >= 0; i--) {
    const date = new Date(today);
    date.setDate(today.getDate() - i);
    days.push({ date, plays: 0, acc: 0, ur: 0 });
  }
  const at = new Map(days.map((x) => [x.date.toDateString(), x]));
  for (const p of plays) {
    const day = at.get(dayKey(p.capturedAt));
    if (!day) continue;
    day.plays++;
    day.acc += p.read.accuracy;
    day.ur += p.read.ur;
  }
  for (const x of days) if (x.plays) Object.assign(x, { acc: x.acc / x.plays, ur: x.ur / x.plays });
  const played = days.filter((x) => x.plays);
  if (played.length < 2) return null;
  const pad = { l: 36, r: 6 };
  const plotW = width - pad.l - pad.r;
  const step = plotW / days.length;
  const cx = (i) => pad.l + (i + 0.5) * step;
  const floor = Math.max(0, Math.floor(Math.min(...played.map((x) => x.acc)) - 1));
  const top = Math.min(100, Math.ceil(Math.max(...played.map((x) => x.acc)) + 0.5));
  const accTop = 10;
  const accBottom = 84;
  const ay = (a) => accTop + (1 - (a - floor) / Math.max(0.5, top - floor)) * (accBottom - accTop);
  const surface = 104;
  const bed = 150;
  const height = 172;
  const root = svg("svg", { class: "chart stream", viewBox: `0 0 ${width} ${height}`, width: "100%", role: "img", "aria-label": `Your ${m.keys}K accuracy and plays each day` });
  for (const v of [floor, top]) {
    root.append(svg("path", { class: "grid", d: `M${pad.l} ${ay(v)}H${width - pad.r}` }));
    root.append(svg("text", { class: "tick-label", x: pad.l - 6, y: ay(v) + 3, "text-anchor": "end" }, `${v}%`));
  }
  const most = Math.max(...days.map((x) => x.plays));
  const wy = (n) => bed - 2 - (n / most) * (bed - 2 - surface);
  const pts = days.map((x, i) => [cx(i), wy(x.plays)]);
  const water = `M${pad.l} ${pts[0][1]}L${curve(pts).slice(1)}L${width - pad.r} ${pts[pts.length - 1][1]}`;
  root.append(svg("path", { class: "flow", d: `${water}V${bed}H${pad.l}Z` }));
  root.append(svg("path", { class: "flow-edge", d: water }));
  root.append(svg("text", { class: "tick-label", x: pad.l + 4, y: surface - 4 }, most === 1 ? "1 play" : `${most} plays`));
  root.append(svg("path", { class: "bed-line", d: `M${pad.l - 6} ${bed + 2}q12-4 24 0${"t24 0".repeat(Math.ceil(plotW / 24))}` }));
  root.append(svg("path", { class: "acc-line", d: curve(days.map((x, i) => x.plays && [cx(i), ay(x.acc)]).filter(Boolean)) }));
  for (const [i, x] of days.entries()) if (x.plays) root.append(svg("circle", { class: "day-dot", cx: cx(i), cy: ay(x.acc), r: 3.5 }));
  root.append(svg("text", { class: "tick-label", x: pad.l, y: height - 2 }, shortDate(days[0].date)));
  root.append(svg("text", { class: "tick-label", x: width - pad.r, y: height - 2, "text-anchor": "end" }, "today"));
  const cross = svg("path", { class: "cross", hidden: "" });
  const dot = svg("circle", { class: "dot", r: 4.5, hidden: "" });
  root.append(cross, dot);
  const hit = svg("rect", { class: "hit", x: pad.l, y: 0, width: plotW, height: bed + 6 });
  hit.addEventListener("pointermove", (e) => {
    const box = root.getBoundingClientRect();
    const px = ((e.clientX - box.left) / box.width) * width;
    const i = Math.max(0, Math.min(days.length - 1, Math.floor((px - pad.l) / step)));
    const x = days[i];
    cross.setAttribute("d", `M${cx(i)} ${accTop - 4}V${bed}`);
    cross.removeAttribute("hidden");
    if (x.plays) {
      dot.setAttribute("cx", cx(i));
      dot.setAttribute("cy", ay(x.acc));
      dot.removeAttribute("hidden");
    } else dot.setAttribute("hidden", "");
    tip.show(e, x.plays ? pct(x.acc) : "No plays", shortDate(x.date), x.plays && `${x.plays} ${x.plays === 1 ? "play" : "plays"}, UR ${Math.round(x.ur)}`);
  });
  hit.addEventListener("pointerleave", () => {
    cross.setAttribute("hidden", "");
    dot.setAttribute("hidden", "");
    tip.hide();
  });
  root.append(hit);
  return root;
}

// the analysis's key lanes, every press of every play in them
function keyStage(m, width, tip) {
  const hits = m.hist.flatMap((col, c) => col.map(([o, n]) => [0, o, 1, c, n]));
  return stageView({ keys: m.keys, columns: m.columns, hands: m.hands, windows: m.windows, hits }, width, tip, 170);
}

function handsNote(m) {
  const { left, right } = m.hands;
  if (!left.hits || !right.hits) return null;
  const gap = right.mean - left.mean;
  if (Math.abs(gap) < 4) return el("p", { class: "caption" }, "Both hands press at about the same time.");
  return el("p", { class: "caption" }, `Your ${gap > 0 ? "right" : "left"} hand presses ${msText(gap)} ms later than your ${gap > 0 ? "left" : "right"}.`);
}

function mostPlayed(plays) {
  const maps = new Map();
  for (const p of plays) {
    const m = maps.get(p.md5) ?? { latest: p, count: 0, best: null };
    m.count++;
    const acc = p.read?.accuracy ?? (p.play.accuracy > 0 ? p.play.accuracy : null);
    if (acc != null && (m.best == null || acc > m.best)) m.best = acc;
    maps.set(p.md5, m);
  }
  return [...maps.values()].sort((a, b) => b.count - a.count || b.latest.capturedAt.localeCompare(a.latest.capturedAt)).slice(0, 5);
}

const accGrade = (a) => (a >= 95 ? ["S", "gold"] : a >= 90 ? ["A", "a"] : a >= 80 ? ["B", "b"] : a >= 70 ? ["C", "c"] : ["D", "d"]);

function profileBody(d) {
  const root = el("div", { class: "profile-body" });
  // the node is kept while the page is open, and coming back from a play puts it back, which would replay the entrance
  setTimeout(() => root.classList.add("settled"), 1800);
  const tip = vizTip(root);
  const c = snapshot.connection;
  const real = d.plays.filter((p) => !p.practice);
  const days = new Map();
  for (const p of real) {
    const k = dayKey(p.capturedAt);
    const day = days.get(k) ?? { plays: 0, sum: 0, read: 0 };
    day.plays++;
    if (p.read) {
      day.sum += p.read.accuracy;
      day.read++;
    }
    day.acc = day.read ? day.sum / day.read : null;
    days.set(k, day);
  }
  root.append(idCard(c, d, days, tip));
  const modes = d.modes;
  if (!modes.length) {
    root.append(el("p", { class: "lead" }, "Finish a mania map with tosu open and its replay is read here."));
    return root;
  }
  const m = modes.find((x) => x.keys === profilePage.keys) ?? modes.reduce((a, b) => (b.plays > a.plays ? b : a));
  const mine = real.filter((p) => p.read && Math.round(p.play.keys) === m.keys);
  const [grade, tone] = accGrade(m.accuracy);
  const facts = [
    [`${signedMs(m.timing.mean)} ms`, `average press, ${leanWord(m.timing.mean, 4)}`],
    [`±${msText(m.timing.sd)} ms`, `spread, UR ${Math.round(m.timing.ur)}`],
    m.release && [`${signedMs(m.release.mean)} ms`, `average release, ${leanWord(m.release.mean, 12)}`],
  ].filter(Boolean);
  const practice = d.plays.length - real.length;
  const maps = mostPlayed(real);
  const charts = [
    fitted((w) => keyStage(m, w, tip)),
    fitted((w) => dayRiver(m, mine, d.keepDays, w, tip)),
  ];
  const [keys, river] = charts;
  root.append(...[
    modes.length > 1 && el("div", { class: "profile-modes" }, segs(modes.map((x) => [x.keys, `${x.keys}K`]), m.keys, (k) => {
      profilePage.keys = k;
      profilePage.node = null;
      render();
    }, "Key count")),
    el("section", { class: "panel profile-score" },
      el("div", { class: "top" },
        el("div", { class: `grade-stamp ${tone}`, style: "--tilt: -4deg" }, grade),
        el("div", { class: "score" },
          el("b", {}, pct(m.accuracy)),
          el("span", {}, `average of ${m.plays} ${m.plays === 1 ? "play" : "plays"}`),
          el("span", {}, `best ${pct(m.best)}`)),
        el("ul", { class: "stats" }, facts.map(([v, l]) => el("li", {}, el("b", {}, v), el("span", {}, l))))),
      m.clean && el("p", { class: "clean-line" },
        `With presses on empty columns counted as misses, ${m.clean.plays === 1 ? "one play comes" : `${m.clean.plays} plays come`} to ${pct(m.clean.acc)}.`)),
    part("area-keys", heading("Your keys"),
      keys,
      handsNote(m),
      m.release && el("p", { class: "caption" },
        `${Math.round(m.release.early * 100)}% of long notes let go early, spread ±${msText(m.release.sd)} ms.`,
        m.release.breaks > 0 ? ` ${m.release.breaks} of ${m.release.holds} let go in the middle.` : "")),
    mine.length >= 2 && part("area-days", heading("Day by day"),
      river,
      el("p", { class: "caption" }, "The pink line is your average accuracy each day, the water is how many plays.")),
    part("area-patterns", heading("Patterns"),
      patternsView(m),
      m.chordSpread != null && el("p", { class: "caption" }, `A chord's keys land ${msText(m.chordSpread)} ms apart on average.`)),
    maps.length > 0 && part("area-maps", heading("Most played"),
      el("ul", { class: "profile-maps" }, maps.map((x, i) => {
        const p = x.latest;
        const diff = diffName(p.play.keys, p.play.version);
        const open = p.analysis ? () => openAnalysis(p) : null;
        return el("li", { class: open ? "opens" : "", onclick: open, title: open ? "Open your latest play" : null },
          cover(p),
          el("div", { class: "text" },
            el("span", { class: "title" }, p.play.title ?? "Unknown"),
            diff && el("span", { class: "artist" }, diff)),
          x.count > 1 && el("span", { class: "times", style: `--tilt: ${i % 2 ? 5 : -6}deg` }, `×${x.count}`),
          el("b", { class: "best" }, x.best != null ? pct(x.best) : "–"));
      }))),
    practice > 0 && el("p", { class: "caption foot" }, `Your ${practice} practice ${practice === 1 ? "play isn't" : "plays aren't"} counted in these numbers.`),
  ].filter(Boolean));
  root.layout = () => charts.forEach((s) => s.fill());
  return root;
}

function profileView() {
  const page = profilePage;
  const c = snapshot.connection;
  const d = page.data;
  let body;
  if (page.error) body = el("p", { class: "status bad" }, page.error);
  else if (!d) body = el("p", { class: "doing" }, el("i", { class: "spinner", style: `animation-delay: -${Date.now() % SPIN_MS}ms` }), "Reading your replays");
  else {
    body = page.node ??= profileBody(d);
    requestAnimationFrame(() => page.node?.layout?.());
  }
  return el("div", { class: "profile-page" },
    el("div", { class: "head" },
      el("p", { class: "say" }, "Profile"),
      el("div", { class: "head-links" },
        el("a", { class: "link", href: `${c.origin}/player/${encodeURIComponent(c.username)}`, target: "_blank", rel: "noreferrer" }, "On Mania Tracker"),
        el("button", { class: "link", onclick: closeProfile }, "Back to plays"))),
    body);
}
