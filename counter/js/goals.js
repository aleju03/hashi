// Goals: what each one measures, its tries and its target.

const GOAL_NAMES = { clean: "Real acc run", practice: "Practice goal" };
const GOAL_UNITS = { clean: "clean", practice: "at full speed" };

function goalTrack(g, width = 460) {
  const values = g.values ?? [g.start, g.previous, g.last].filter((v) => v != null);
  // a goal already beaten on its first try still needs some track to the left of the flag
  const lo = Math.min(g.target - 1, g.start ?? g.target - 5, ...values);
  const left = 10;
  const right = width - 30;
  // a try that reached the target lands past the post, further the more it beat it by
  const x = (v) => v >= g.target
    ? right + 10 + Math.min(1, (v - g.target) / Math.max(0.5, 100 - g.target)) * 8
    : left + Math.max(0, (v - lo) / (g.target - lo)) * (right - left);
  const y = 28;
  const root = svg("svg", {
    class: `chart goal-track${g.done ? " done" : ""}`,
    viewBox: `0 0 ${width} 60`,
    width: "100%",
    role: "img",
    "aria-label": g.best != null ? `Best ${accText(g.best)}% of ${g.target}%` : `Goal ${g.target}%`,
  });
  // thin boxes, not lines: the ink filter works off the shape's box, a flat line's has no height
  root.append(svg("rect", { class: "goal-rail", x: left, y: y - 1.5, width: right - left, height: 3, rx: 1.5 }));
  if (g.best != null) root.append(svg("rect", { class: "goal-made", x: left, y: y - 2, width: Math.max(4, x(g.best) - left), height: 4, rx: 2 }));
  root.append(svg("rect", { class: "goal-pole", x: right - 1, y: y - 20, width: 2, height: 29, rx: 1 }));
  root.append(svg("path", { class: "goal-flag", d: `M${right + 1} ${y - 20}l15 5.5-15 5.5z` }));
  values.forEach((v, i) => {
    const age = values.length - 1 - i;
    const best = v === g.best && i === values.lastIndexOf(g.best);
    root.append(svg("circle", { class: `goal-try${best ? " best" : ""}`, cx: x(v), cy: y, r: best ? 5.5 : 3.5, style: `opacity: ${best ? 1 : Math.max(0.3, 1 - age * 0.18)}` }));
  });
  if (g.last != null && !g.done && values.length > 1) {
    root.append(svg("text", { class: "goal-num", x: x(g.last), y: y - 12, "text-anchor": "middle" }, "last"));
  }
  if (g.start != null && x(g.start) < right - 50) root.append(svg("text", { class: "goal-num", x: x(g.start), y: 56, "text-anchor": x(g.start) < left + 30 ? "start" : "middle" }, `${accText(g.start)}%`));
  root.append(svg("text", { class: "goal-num target", x: right, y: 56, "text-anchor": "middle" }, `${g.target}%`));
  return root;
}

function goalLine(g) {
  const unit = GOAL_UNITS[g.kind] ?? "";
  if (g.done) return `${pct(g.best)} ${unit}. Reached in ${g.tries} ${g.tries === 1 ? "try" : "tries"}.`;
  if (!g.tries) return `${g.target}% ${unit} to reach. Play it to start.`;
  const change = g.previous != null ? ` (${deltaText(g.last - g.previous)})` : "";
  return `Best ${pct(g.best)} ${unit}, last ${pct(g.last)}${change}, ${g.tries} ${g.tries === 1 ? "try" : "tries"}. ${accText(Math.max(0, g.target - g.best))}% to go.`;
}

async function dismissGoal(g) {
  await action(`goals/${encodeURIComponent(g.id)}/dismiss`);
  // the analysis keeps its own copy of the goal
  if (analysis?.data?.goal?.id === g.id) {
    analysis.data.goal = null;
    analysis.node = null;
    render();
  }
}

let editingGoal = null;

async function saveGoalTarget(g, value) {
  if (!(value >= 1 && value <= 100)) return;
  await action(`goals/${encodeURIComponent(g.id)}/target/${value}`);
  editingGoal = null;
  if (analysis?.data?.goal?.id === g.id) {
    analysis.data.goal = (snapshot.goals ?? []).find((x) => x.id === g.id) ?? analysis.data.goal;
    analysis.node = null;
  }
  render();
}

function goalEditor(g) {
  const input = el("input", { class: "goal-input", type: "number", min: 1, max: 100, step: 0.1, value: g.target, "aria-label": "New goal" });
  const save = () => saveGoalTarget(g, Math.round(Number(input.value) * 10) / 10);
  input.addEventListener("keydown", (e) => {
    if (e.key === "Enter") save();
    if (e.key === "Escape") {
      editingGoal = null;
      render();
    }
  });
  requestAnimationFrame(() => input.select());
  return el("div", { class: "goal-edit" },
    el("span", {}, "Goal"),
    input,
    el("span", {}, `% ${GOAL_UNITS[g.kind] ?? ""}`),
    el("button", { class: "link", onclick: save }, "Save"),
    el("button", { class: "link", onclick: () => { editingGoal = null; render(); } }, "Cancel"));
}

function goalEntry(g, open = true) {
  const latest = open && snapshot.plays.find((p) => p.id === g.latestPlay && p.analysis);
  const title = latest
    ? el("button", { class: "goal-title", title: "Open the latest try", onclick: () => openAnalysis(latest) }, g.title)
    : el("span", { class: "goal-title" }, g.title);
  const name = el("h3", {}, title);
  name.insertAdjacentHTML("beforeend", `<svg class="squiggle" viewBox="0 0 64 8" width="64" height="8" aria-hidden="true"><path d="M2 5q7-5 14 0t14 0t14 0t14 0"/></svg>`);
  return el("section", { class: `goal${g.done ? " done" : ""}` },
    el("div", { class: "goal-top" },
      el("span", { class: "goal-kind" }, GOAL_NAMES[g.kind] ?? "Goal"),
      g.done && el("span", { class: "stamp done goal-stamp", style: "--tilt: -5deg" }, "Reached"),
      el("button", { class: "link goal-x", onclick: () => { editingGoal = g.id; render(); } }, "Change goal"),
      el("button", { class: "link goal-x", onclick: () => dismissGoal(g) }, g.done ? "Clear" : "Stop tracking")),
    name,
    el("p", { class: "goal-version" }, g.version),
    goalTrack(g),
    editingGoal === g.id ? goalEditor(g) : el("p", { class: "goal-line" }, goalLine(g)));
}

function goalCard(g) {
  return el("div", { class: "goals in-analysis" }, goalEntry(g, false));
}

let goalsOpen = false;
const GOALS_SEEN = "hashi-goals-seen";

const goalSeenKey = (g) => `${g.id} ${g.doneAt}`;

function goalsUnseen() {
  let seen = [];
  try { seen = JSON.parse(localStorage.getItem(GOALS_SEEN)) ?? []; } catch {}
  return (snapshot.goals ?? []).some((g) => g.done && !seen.includes(goalSeenKey(g)));
}

function toggleGoals() {
  goalsOpen = !goalsOpen;
  if (goalsOpen) {
    settingsOpen = notesOpen = false;
    analysis = maker = null;
    dropTrainer();
    skinPage = profilePage = null;
    board = null;
    clearTimeout(boardTimer);
    try { localStorage.setItem(GOALS_SEEN, JSON.stringify((snapshot.goals ?? []).filter((g) => g.done).map(goalSeenKey))); } catch {}
  }
  render();
}

function goalsView() {
  const goals = snapshot.goals ?? [];
  return el("div", { class: "goals-page" },
    el("div", { class: "head" },
      el("p", { class: "say" }, "Goals"),
      el("button", { class: "link", onclick: toggleGoals }, "Back to plays")),
    el("p", { class: "caption" }, goals.length
      ? "Every play of the map counts as a try. Click a map to open its latest try."
      : "No goals right now. Hashi sets one when a play presses keys with no notes, and for each practice map you make."),
    goals.length > 0 && el("div", { class: "goals" }, goals.map((g) => goalEntry(g))));
}
