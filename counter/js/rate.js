let rater = null;
// the helper's SLOWEST and FASTEST
const RATE_MIN = 0.5;
const RATE_MAX = 2;
const RATE_STEP = 0.05;
const GAUGE = `<path d="M1 8a4 4 0 1 1 8 0"/><path d="M5 8 7 5" stroke-linecap="round"/>`;

const rateText = (r) => String(Number(r.toFixed(2)));
const rateClamp = (r) => Math.min(RATE_MAX, Math.max(RATE_MIN, Math.round(r * 100) / 100));
const rateMod = (r, keep) => (r > 1 ? (keep ? "DT" : "NC") : (keep ? "HT" : "DC"));
// what the helper adds to the difficulty's name
const rateSuffix = (r) => (r === 1 ? "" : ` ${rateText(r)}x`);
const newRater = () => ({
  map: null, loading: true, error: null, busy: false, made: [], own: [], makeError: null,
  rate: 1,
  keep: true,
  asked: snapshot.tosu.map?.md5,
});

function openRater() {
  settingsOpen = goalsOpen = notesOpen = reportOpen = false;
  analysis = maker = null;
  dropTrainer();
  dropCutter();
  skinPage = profilePage = null;
  board = null;
  clearTimeout(boardTimer);
  const r = (rater = newRater());
  render();
  loadRater(r);
}

function closeRater() {
  rater = null;
  render();
}

// a map change loads into a new page behind the shown one
async function loadRater(r) {
  const next = r.loading ? r : newRater();
  const asked = (r.asked = next.asked);
  try {
    const res = await fetch(`${BRIDGE}/rate/map`);
    const map = await res.json();
    if (!res.ok) throw new Error(map.error);
    next.map = map;
  } catch (e) {
    next.error = e.message || "Couldn't reach Hashi's helper.";
  }
  next.loading = false;
  if (rater !== r || r.asked !== asked) return;
  if (r.busy) return void (r.asked = null);
  rater = next;
  render();
}

// osu! opens a map made here by itself, the page stays on the one it came from
function raterFollow() {
  const r = rater;
  const md5 = snapshot.tosu.map?.md5;
  if (!r || r.loading || r.busy || !md5 || md5 === r.asked || r.own.includes(md5)) return;
  loadRater(r);
}

function setRate(r, value) {
  if (!Number.isFinite(value)) return render();
  r.rate = rateClamp(value);
  r.makeError = null;
  r.changed?.();
  render();
}

async function makeRate(r) {
  if (r.busy || !r.map || r.rate === 1) return;
  r.busy = true;
  r.makeError = null;
  render();
  try {
    const q = new URLSearchParams({ rate: r.rate, pitch: r.keep ? "keep" : "change" });
    const res = await fetch(`${BRIDGE}/rate/${r.map.md5}?${q}`, { method: "POST" });
    const body = await res.json().catch(() => ({}));
    if (rater !== r) return;
    if (res.ok) {
      r.made = [...r.made.filter((v) => v !== body.version), body.version];
      r.own.push(body.md5);
    } else r.makeError = body.error ?? "Couldn't make the map.";
  } catch {
    if (rater === r) r.makeError = "Lost Hashi's helper.";
  }
  if (rater !== r) return;
  r.busy = false;
  render();
}

// kept across redraws, which would throw away what's typed
function rateField(r) {
  r.field ??= el("input", {
    class: "rate-input", type: "text", inputmode: "decimal", spellcheck: "false", "aria-label": "Rate",
    onchange: () => setRate(r, Number(r.field.value.replace(",", ".").replace(/x$/i, ""))),
    onkeydown: (ev) => {
      if (ev.key !== "ArrowUp" && ev.key !== "ArrowDown") return;
      ev.preventDefault();
      setRate(r, r.rate + (ev.key === "ArrowUp" ? 0.01 : -0.01));
    },
  });
  if (!hasFocus(r.field)) r.field.value = rateText(r.rate);
  return r.field;
}

function rateStep(r, by) {
  // to the next whole step, so 1.23 goes to 1.25 and 1.2
  const steps = r.rate / RATE_STEP;
  const next = by > 0 ? Math.floor(steps + 1e-6) + 1 : Math.ceil(steps - 1e-6) - 1;
  setRate(r, next * RATE_STEP);
}

// r is the rate page or the cut page, both keep rate, keep and field
function rateStepper(r) {
  return el("div", { class: "segs" },
    el("button", { class: "seg", disabled: r.rate <= RATE_MIN, "aria-label": "Slower", onclick: () => rateStep(r, -1) }, "−"),
    rateField(r),
    el("button", { class: "seg", disabled: r.rate >= RATE_MAX, "aria-label": "Faster", onclick: () => rateStep(r, 1) }, "+"));
}

function pitchSegs(r) {
  const up = r.rate > 1;
  return segs([[true, up ? "DT" : "HT"], [false, up ? "NC" : "DC"]], r.keep, (keep) => {
    r.keep = keep;
    r.changed?.();
    render();
  }, up ? "DT keeps the song's pitch, NC raises it" : "HT keeps the song's pitch, DC lowers it");
}

// 1x points straight up, an octave either way is DIAL_TURN degrees
const DIAL_W = 400;
const DIAL_H = 242;
const DIAL_X = 200;
const DIAL_Y = 164;
const DIAL_R = 130;
const DIAL_TURN = 100;
const DIAL_NAMED = [0.5, 0.75, 1, 1.5, 2];
const TORTOISE_MS = 1800;
const HARE_MS = 600;

const dialTurn = (rate) => Math.log2(rate) * DIAL_TURN;
function dialAt(turn, radius) {
  const a = turn * Math.PI / 180;
  return `${(DIAL_X + radius * Math.sin(a)).toFixed(1)} ${(DIAL_Y - radius * Math.cos(a)).toFixed(1)}`;
}
function dialArc(from, to, radius) {
  return `M${dialAt(from, radius)}A${radius} ${radius} 0 ${Math.abs(to - from) > 180 ? 1 : 0} ${to > from ? 1 : 0} ${dialAt(to, radius)}`;
}

const TORTOISE = `<g class="tortoise" transform="translate(8 206)">
  <g class="legs"><path d="M15 21v7h6v-7"/><path d="M34 21v7h6v-7"/></g>
  <path d="M10 21l-6 3"/>
  <path class="head" d="M44 19q3-8 9-7q5 1 4 6q-1 4-7 4h-6z"/>
  <path class="shell" d="M8 22q1-17 20-17t20 17z"/>
  <path d="M21 8l-3 14M35 8l3 14M13 15h30"/>
  <circle class="eye" cx="53" cy="16" r="1"/>
  <path class="ground" d="M0 30q7-2 14 0t14 0t14 0t14 0"/>
</g>`;
const HARE = `<g class="hare" transform="translate(336 206)">
  <path class="whoosh" d="M0 8h6M1 14h4M0 20h5"/>
  <g class="leap">
    <path class="coat" d="M15 18q-4 5-10 8q8 1 15-4zM34 18q5 6 13 6q-7 3-16-2z"/>
    <circle class="coat" cx="11.5" cy="13" r="3.2"/>
    <path d="M43 6q-3-9-11-10q0 8 7 12"/>
    <path class="coat" d="M12 16q1-10 14-10q10 0 14 7q1 9-10 10h-9q-9 0-9-7z"/>
    <path class="head" d="M38 12q0-7 7-7q8 1 8 6q0 5-7 5q-7 0-8-4z"/>
    <path class="head" d="M46 5q-2-9-10-11q-1 8 6 12"/>
    <circle class="eye" cx="48.5" cy="9.5" r="1"/>
  </g>
  <path class="ground" d="M0 30q7-2 14 0t14 0t14 0t14 0"/>
</g>`;
// kept across redraws so a drag keeps its needle
function rateDial(r) {
  if (!r.dial) {
    const ticks = Array.from({ length: 16 }, (_, i) => (5 + i) / 10)
      .map((x) => `M${dialAt(dialTurn(x), DIAL_R + 9)}L${dialAt(dialTurn(x), DIAL_R + (DIAL_NAMED.includes(x) ? 17 : 13))}`).join("")
      + `M${dialAt(dialTurn(0.75), DIAL_R + 9)}L${dialAt(dialTurn(0.75), DIAL_R + 17)}`;
    const names = DIAL_NAMED.map((x) => {
      const [nx, ny] = dialAt(dialTurn(x), DIAL_R + 29).split(" ");
      return `<text class="dial-name" x="${nx}" y="${Number(ny) + 4}" text-anchor="middle">${rateText(x)}x</text>`;
    }).join("");
    const node = svg("svg", { class: "rate-dial", viewBox: `0 0 ${DIAL_W} ${DIAL_H}`, role: "slider", tabindex: 0, "aria-label": "Rate", "aria-valuemin": RATE_MIN, "aria-valuemax": RATE_MAX });
    node.innerHTML = `<g class="dial-inked"><path class="dial-rail" d="${dialArc(-DIAL_TURN, DIAL_TURN, DIAL_R)}"/><path class="dial-ticks" d="${ticks}"/></g>
      <path class="dial-fill"/>
      ${names}
      <g class="dial-needle"><g class="dial-hand"><path d="M${DIAL_X - 4.5} ${DIAL_Y}L${DIAL_X} ${DIAL_Y - DIAL_R + 16}L${DIAL_X + 4.5} ${DIAL_Y}z"/></g><circle cx="${DIAL_X}" cy="${DIAL_Y}" r="8.5"/></g>
      <g class="dial-doodles" style="${loop(TORTOISE_MS)}">${TORTOISE}</g>
      <g class="dial-doodles" style="${loop(HARE_MS)}">${HARE}</g>
      <text class="dial-read" x="${DIAL_X}" y="224" text-anchor="middle"><tspan class="dial-rate"/><tspan class="dial-mod" dx="8"/></text>
      <path class="dial-hit" d="${dialArc(-DIAL_TURN, DIAL_TURN, DIAL_R + 40)}L${DIAL_X} ${DIAL_Y}z"/>`;
    const q = (sel) => node.querySelector(sel);
    r.dial = { node, fill: q(".dial-fill"), hand: q(".dial-hand"), rate: q(".dial-rate"), mod: q(".dial-mod") };
    rateDialEvents(r, node);
  }
  const d = r.dial;
  const turn = dialTurn(r.rate);
  d.node.setAttribute("class", `rate-dial${r.rate > 1 ? " fast" : r.rate < 1 ? " slow" : ""}`);
  d.node.setAttribute("aria-valuenow", r.rate);
  d.node.setAttribute("aria-valuetext", `${rateText(r.rate)}x`);
  d.fill.setAttribute("d", r.rate === 1 ? "" : dialArc(0, turn, DIAL_R));
  d.hand.setAttribute("transform", `rotate(${turn.toFixed(2)} ${DIAL_X} ${DIAL_Y})`);
  d.rate.textContent = `${rateText(r.rate)}x`;
  d.mod.textContent = r.rate === 1 ? "" : rateMod(r.rate, r.keep);
  return d.node;
}

function rateDialEvents(r, node) {
  const rateAt = (ev) => {
    const box = node.getBoundingClientRect();
    const k = DIAL_W / box.width;
    const turn = Math.atan2((ev.clientX - box.left) * k - DIAL_X, DIAL_Y - (ev.clientY - box.top) * k) * 180 / Math.PI;
    return 2 ** (Math.max(-DIAL_TURN, Math.min(DIAL_TURN, turn)) / DIAL_TURN);
  };
  node.querySelector(".dial-hit").addEventListener("pointerdown", (ev) => {
    if (ev.button !== 0) return;
    ev.preventDefault();
    // on the window: every step redraws the page, which would drop a capture
    const move = (e) => {
      if (rater !== r || !e.buttons) return up();
      const next = rateClamp(Math.round(rateAt(e) / RATE_STEP) * RATE_STEP);
      if (next !== r.rate) setRate(r, next);
    };
    const up = () => ["pointermove", "pointerup", "pointercancel"].forEach((name) => removeEventListener(name, name === "pointermove" ? move : up));
    addEventListener("pointermove", move);
    addEventListener("pointerup", up);
    addEventListener("pointercancel", up);
    move(ev);
  });
  node.addEventListener("keydown", (ev) => {
    const by = { ArrowRight: 1, ArrowUp: 1, ArrowLeft: -1, ArrowDown: -1 }[ev.key];
    if (!by) return;
    ev.preventDefault();
    rateStep(r, by);
  });
}

function raterView() {
  const r = rater;
  const m = r.map;
  const up = r.rate > 1;
  const version = `${m?.version ?? ""}${rateSuffix(r.rate)}`.trim();
  const field = (label, ...control) => el("div", { class: "maker-field" }, el("span", { class: "maker-label" }, label), el("div", { class: "maker-control" }, control));
  const fact = (label, before, after) => el("div", { class: "rate-fact" },
    el("span", { class: "rate-fact-label" }, label),
    el("span", { class: "rate-was" }, before),
    el("span", { class: "rate-arrow", "aria-hidden": "true" }, "→"),
    el("b", {}, after));
  const bpm = (x) => String(Math.round(x * 10) / 10);
  return el("div", { class: "rater" },
    el("div", { class: "head" },
      el("p", { class: "say" }, "Change the rate"),
      el("button", { class: "link back", onclick: closeRater }, "Back to plays")),
    m && el("div", { class: "map" },
      el("span", { class: "title" }, m.title ?? "Unknown"),
      m.artist && el("span", { class: "artist" }, m.artist),
      el("div", { class: "facts" },
        el("span", {}, diffName(m.keys, m.version)),
        el("span", {}, `${m.notes} notes`))),
    r.loading && el("p", { class: "doing" }, el("i", { class: "spinner", style: `animation-delay: -${Date.now() % SPIN_MS}ms` }), "Reading the map"),
    r.error && el("p", { class: "status bad" }, r.error),
    m && [
      el("div", { class: "rate-change" },
        rateDial(r),
        el("div", { class: "rate-facts" },
          fact("BPM", bpm(m.bpm), bpm(m.bpm * r.rate)),
          fact("Length", clock(m.length / 1000), clock(m.length / 1000 / r.rate)))),
      el("div", { class: "rate-controls" },
        field("Rate", rateStepper(r), r.rate !== 1 && pitchSegs(r)),
        el("p", { class: "caption" },
          r.rate === 1 ? "Pick a rate other than 1x." : `A new difficulty, “${version}”, with the whole map and its song at this rate.`),
        el("button", { class: "primary maker-go", disabled: r.busy || r.rate === 1, onclick: () => makeRate(r) },
          r.busy ? (up ? "Speeding up the song…" : "Slowing down the song…") : "Make it and open in osu!"),
        r.makeError && el("p", { class: "status bad" }, r.makeError),
        r.made.length > 0 && el("p", { class: "status good" }, `Sent to osu! as “${r.made[r.made.length - 1]}”.`),
        r.made.length > 1 && el("p", { class: "caption" }, `Also made: ${r.made.slice(0, -1).join(", ")}.`)),
    ]);
}
