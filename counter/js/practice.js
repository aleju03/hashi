// Making a practice map from a stretch of a play, and how its repeats went.

let maker = null;
const REPEAT_CHOICES = [1, 2, 3, 4, 6, 8];
const SLOWEST_CHOICES = [[0.7, "70%"], [0.8, "80%"], [0.85, "85%"], [0.9, "90%"], [1, "100%"]];
// longer and it sounds like the notes are about to start when they aren't
const LEAD_CHOICES = [[0.5, "0.5 s"], [1, "1 s"], [2, "2 s"]];
const STRETCH_MIN = 1;
const STRETCH_MAX = 60;
// the helper's padding around each repeat
const REPEAT_EXTRA_S = 2 + 1;

const songClock = (s) => `${Math.floor(s / 60)}:${String(Math.floor(s % 60)).padStart(2, "0")}`;

function parseClock(text) {
  const m = /^\s*(\d+):(\d{1,2}(?:\.\d+)?)\s*$/.exec(text);
  if (m) return Number(m[1]) * 60 + Number(m[2]);
  const s = /^\s*(\d+(?:\.\d+)?)\s*$/.exec(text);
  return s ? Number(s[1]) : null;
}

const repeatSpeeds = (m) => (m.repeats === 1 ? [1] : Array.from({ length: m.repeats }, (_, k) => m.slowest + ((1 - m.slowest) * k) / (m.repeats - 1)));

function openMaker(d) {
  const p = d.practice;
  maker = {
    id: analysis.id,
    title: analysis.play.play.title ?? "this song",
    back: scroller().scrollTop,
    // the tip's stretch counts from the first note, the song's clock from its start
    from: Math.floor(p.from + d.offset),
    to: Math.ceil(p.to + d.offset),
    repeats: 4,
    slowest: 0.85,
    lead: 1,
    goal: Math.floor(p.rest * 10) / 10,
    rest: p.rest,
    worst: p.acc,
    busy: false,
    result: null,
    error: null,
    made: d.practiceMaps ?? [],
  };
  render();
}

function closeMaker() {
  const back = maker?.back ?? 0;
  maker = null;
  render();
  requestAnimationFrame(() => (scroller().scrollTop = back));
}

async function makePractice() {
  const m = maker;
  if (!m || m.busy) return;
  m.busy = true;
  m.error = m.result = null;
  render();
  const q = new URLSearchParams({ from: m.from, to: m.to, repeats: m.repeats, slowest: m.slowest, lead: m.lead, goal: m.goal });
  try {
    const res = await fetch(`${BRIDGE}/practice/${encodeURIComponent(m.id)}?${q}`, { method: "POST" });
    const body = await res.json().catch(() => ({}));
    if (maker !== m) return;
    if (res.ok) {
      m.result = body;
      m.made = [...m.made, { version: body.version }];
    } else {
      m.error = body.error ?? (res.status === 400 ? `A stretch has to be ${STRETCH_MIN} to ${STRETCH_MAX} seconds long.` : "Couldn't make the practice map.");
    }
  } catch {
    if (maker === m) m.error = "Lost Hashi's helper.";
  }
  if (maker !== m) return;
  m.busy = false;
  render();
}

function makerView() {
  const m = maker;
  const clockInput = (key) => {
    const input = el("input", { class: "maker-time", type: "text", value: songClock(m[key]), "aria-label": key === "from" ? "Stretch starts" : "Stretch ends" });
    input.addEventListener("change", () => {
      const v = parseClock(input.value);
      if (v != null) m[key] = v;
      render();
    });
    return input;
  };
  const goal = el("input", { class: "maker-goal", type: "number", min: 50, max: 100, step: 0.1, value: m.goal, "aria-label": "Goal accuracy" });
  goal.addEventListener("change", () => {
    const v = Number(goal.value);
    if (v >= 50 && v <= 100) m.goal = v;
    render();
  });
  const length = m.to - m.from;
  const fits = length >= STRETCH_MIN && length <= STRETCH_MAX;
  const speeds = repeatSpeeds(m);
  const total = speeds.reduce((sum, s) => sum + (length + m.lead + REPEAT_EXTRA_S) / s, 1);
  const field = (label, ...control) => el("div", { class: "maker-field" }, el("span", { class: "maker-label" }, label), el("div", { class: "maker-control" }, control));
  return el("div", { class: "maker" },
    el("div", { class: "head" },
      el("p", { class: "say" }, "Practice map"),
      el("button", { class: "link", onclick: closeMaker }, "Back to the analysis")),
    el("p", { class: "caption" }, `A new difficulty of ${m.title} with only this stretch in it, ${m.repeats === 1 ? "once" : `${m.repeats} times in a row, slower first`}. In the play it got ${pct(m.worst)}, the rest of the map ${pct(m.rest)}.`),
    field("Stretch", clockInput("from"), el("span", { class: "maker-dash" }, "to"), clockInput("to")),
    field("Repeats", segs(REPEAT_CHOICES.map((n) => [n, String(n)]), m.repeats, (n) => {
      m.repeats = n;
      render();
    }, "How many times the stretch comes")),
    m.repeats > 1 && field("First at", segs(SLOWEST_CHOICES, m.slowest, (s) => {
      m.slowest = s;
      render();
    }, "The first repeat's speed, the last one is always the speed you played")),
    field("Music before", segs(LEAD_CHOICES, m.lead, (s) => {
      m.lead = s;
      render();
    }, "How much of the song plays, without notes, before each repeat's first note")),
    field("Goal", goal, el("span", { class: "maker-unit" }, "% on a full-speed repeat")),
    el("p", { class: "caption" },
      fits ? `${speeds.map((s) => `${Math.round(s * 100)}%`).join(", ")} of the speed you played, about ${Math.round(total)} seconds in all.` : `A stretch has to be ${STRETCH_MIN} to ${STRETCH_MAX} seconds long.`,
      " Plays of it stay in Hashi with their own breakdown, they aren't sent to Mania Tracker."),
    el("button", { class: "primary maker-go", disabled: m.busy || !fits, onclick: makePractice }, m.busy ? "Cutting the song…" : "Make it and open in osu!"),
    m.error && el("p", { class: "status bad" }, m.error),
    m.result && el("p", { class: "status good" }, `Sent to osu! as “${m.result.version}”. Find it with ${m.title}'s other difficulties.`),
    m.made.length > 0 && el("p", { class: "caption" }, `Made from this play: ${m.made.map((x) => x.version).join(", ")}.`));
}

function practiceCard(d) {
  const r = d.practiceRun;
  const fullSpeed = (rows) => rows.filter((x) => x.speed >= 0.999 && x.acc != null).map((x) => x.acc);
  const now = fullSpeed(r.repeats);
  const best = now.length ? Math.max(...now) : null;
  const met = r.repeats.some((x) => x.met);
  const tries = (r.attempts ?? []).map((a) => fullSpeed(a.repeats)).filter((x) => x.length).map((x) => Math.max(...x)).reverse();
  return part("area-practice", heading("Practice"),
    el("p", { class: "caption" }, `${songClock(r.from)}–${songClock(r.to)} of the play, ${r.repeats.length === 1 ? "once" : `${r.repeats.length} times`}.${r.goal != null ? ` Goal: ${r.goal}% on a full-speed repeat.` : ""}`),
    el("div", { class: "trainer-table" },
      el("div", { class: "practice-row head" }, ["Repeat", "Speed", "Accuracy", "Misses"].map((h) => el("span", {}, h))),
      r.repeats.map((x) => el("div", { class: `practice-row${x.met ? " met" : ""}` },
        el("span", {}, x.repeat),
        el("span", {}, `${Math.round(x.speed * 100)}%`),
        el("span", {}, pct(x.acc)),
        el("span", {}, x.misses)))),
    r.goal != null && best != null && el("p", { class: `practice-verdict${met ? " good" : ""}` },
      met ? `Goal reached: ${pct(best)} at full speed.` : `${pct(best)} at full speed, ${accText(Math.max(0, r.goal - best))}% to go.`),
    tries.length > 0 && el("p", { class: "caption" }, `Earlier tries at full speed, oldest first: ${tries.map((x) => pct(x)).join(", ")}.`));
}
