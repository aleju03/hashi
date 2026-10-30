// The chord trainer: its metronome, the keys it listens to and what it says about your chords.

let trainer = null;
// wider than this apart, two presses are two notes, not a chord
const CHORD_MS = 100;
// keyboards poll every millisecond, a chord this close is as together as it gets
const TOGETHER_MS = 3;
// the lead the analysis calls a roll
const ROLL_MS = 7;
// longer than this since the last press, a chord comes after a pause, not after a note
const PAUSE_MS = 1000;
const TRAINER_KEYS = "hashi-trainer-keys";
const TRAINER_TEMPO = "hashi-trainer-tempo";
const SHOWN_CHORDS = 40;
const STATS_CHORDS = 30;
const RAMP_TICKS = 16;
const RAMP_BPM = 10;
const DIVISIONS = [[1, "1/1"], [2, "1/2"], [4, "1/4"]];
const UPCOMING = 8;
const NO_SOUND = {
  gstreamer: "No sound. Hashi plays it through GStreamer, and its good plugins aren't installed: gst-plugins-good, or gstreamer1.0-plugins-good on Ubuntu and Debian. Install them, then restart Hashi.",
  appimage: "No sound. The AppImage can't play it on this system. The .deb or .rpm can, with GStreamer's good plugins installed.",
};

function trainerPair(d, hand, columns) {
  const played = (d.pairs ?? []).filter((p) => p.hand === hand);
  const asked = columns && played.find((p) => p.columns[0] === columns[0] && p.columns[1] === columns[1]);
  if (asked) return asked;
  if (played.length) return [...played].sort((a, b) => Math.abs(b.mean) - Math.abs(a.mean))[0];
  for (let c = 0; c + 1 < d.columns.length; c++) {
    if (d.columns[c].hand === hand && d.columns[c + 1].hand === hand) return { columns: [c, c + 1], hand, mean: null, sd: null, chords: 0 };
  }
  return null;
}

const canTrain = (d) => !!(trainerPair(d, "left") || trainerPair(d, "right"));

function openTrainer(d, hand, columns) {
  dropTrainer();
  const tempo = stored(TRAINER_TEMPO, {});
  trainer = {
    d,
    back: scroller().scrollTop,
    mode: tempo.mode === "patterns" ? "patterns" : "chords",
    metro: { bpm: tempo.bpm ?? 120, div: tempo.div ?? 2, ramp: !!tempo.ramp, running: false, queue: [] },
  };
  setTrainerHand(trainerPair(d, hand) ? hand : hand === "left" ? "right" : "left", columns);
  render();
}

function setTrainerHand(hand, columns) {
  stopMetronome();
  const t = trainer;
  const pair = trainerPair(t.d, hand, columns);
  const [a, b] = pair.columns;
  const slot = `${t.d.keys}K ${a}-${b}`;
  const keys = stored(TRAINER_KEYS, {})[slot] ?? [null, null];
  Object.assign(t, {
    hand,
    slot,
    columns: [a, b],
    fingers: [a, b].map((c) => `${t.d.columns[c].hand} ${t.d.columns[c].finger}`),
    play: pair.mean != null ? pair : null,
    codes: keys.map((k) => k?.code ?? null),
    labels: keys.map((k) => k?.label ?? null),
    binding: keys[0] ? (keys[1] ? null : 1) : 0,
    held: new Set(),
    pending: null,
    // { gap, tempo: "160 1/2" | null, after: "chord" | 0 | 1 | null }
    chords: [],
    lastKind: null,
    lastAt: null,
  });
}

function dropTrainer() {
  stopMetronome();
  trainer?.metro.ctx?.close();
  trainer = null;
}

function closeTrainer() {
  const back = trainer?.back ?? 0;
  dropTrainer();
  render();
  requestAnimationFrame(() => (scroller().scrollTop = back));
}

function saveTrainerKeys() {
  const saved = stored(TRAINER_KEYS, {});
  saved[trainer.slot] = trainer.codes.map((code, i) => code && { code, label: trainer.labels[i] });
  store(TRAINER_KEYS, saved);
}

function saveTempo() {
  const m = trainer.metro;
  store(TRAINER_TEMPO, { bpm: m.bpm, div: m.div, ramp: m.ramp, mode: trainer.mode });
}

const tempoName = (m) => `${m.bpm} ${DIVISIONS.find(([d]) => d === m.div)[1]}`;

// ticks are put on the audio clock ahead of time, a timer alone drifts
function startMetronome() {
  const m = trainer.metro;
  if (m.running) return;
  const Audio = window.AudioContext ?? window.webkitAudioContext;
  if (!Audio) return;
  m.ctx ??= new Audio();
  m.ctx.resume();
  m.running = true;
  m.tick = 0;
  m.atTempo = 0;
  m.queue = [];
  m.recent = [];
  m.next = m.ctx.currentTime + 0.3;
  m.timer = setInterval(scheduleTicks, 25);
  m.silent = false;
  scheduleTicks();
  requestAnimationFrame(followTicks);
  trainer.refresh?.();
  // webkitgtk without an audio sink leaves the context interrupted, its clock never moves
  const started = m.ctx.currentTime;
  setTimeout(() => {
    if (trainer?.metro !== m || !m.running || m.ctx.currentTime - started > 0.2) return;
    stopMetronome();
    m.silent = true;
    trainer.refresh?.();
  }, 1000);
}

function stopMetronome() {
  const m = trainer?.metro;
  if (!m?.running) return;
  clearInterval(m.timer);
  m.running = false;
  m.queue = [];
  trainer.refresh?.();
}

// mostly chords, never three single notes in a row, like a jumpjack
function nextNote(m) {
  const alone = m.recent.slice(-2).filter((x) => x !== "chord").length;
  const note = alone >= 2 || Math.random() < 0.5 ? "chord" : Math.random() < 0.5 ? 0 : 1;
  m.recent = [...m.recent.slice(-3), note];
  return note;
}

function scheduleTicks() {
  const t = trainer;
  const m = t?.metro;
  if (!m?.running) return;
  // the upcoming notes are shown ahead of their ticks, so they're picked further ahead than the clicks
  while (m.next < m.ctx.currentTime + 0.12 + (t.mode === "patterns" ? (UPCOMING * 60) / m.bpm / m.div : 0)) {
    const at = m.next;
    m.queue.push({ at, note: t.mode === "patterns" ? nextNote(m) : "chord", beat: m.tick % m.div === 0, clicked: false });
    m.next += 60 / m.bpm / m.div;
    m.tick += 1;
    m.atTempo += 1;
    if (m.ramp && m.atTempo >= RAMP_TICKS && m.tick % m.div === 0) {
      m.bpm = Math.min(400, m.bpm + RAMP_BPM);
      m.atTempo = 0;
      saveTempo();
      t.refresh?.();
    }
  }
  for (const q of m.queue) {
    if (q.clicked || q.at > m.ctx.currentTime + 0.12) continue;
    q.clicked = true;
    const beat = q.beat;
    const osc = m.ctx.createOscillator();
    const gain = m.ctx.createGain();
    osc.frequency.value = beat ? 1500 : 1000;
    gain.gain.setValueAtTime(beat ? 0.5 : 0.25, q.at);
    gain.gain.exponentialRampToValueAtTime(0.001, q.at + 0.04);
    osc.connect(gain).connect(m.ctx.destination);
    osc.start(q.at);
    osc.stop(q.at + 0.05);
  }
}

function followTicks() {
  const t = trainer;
  const m = t?.metro;
  if (!m?.running) return;
  const half = 30 / m.bpm / m.div;
  const before = m.queue.length;
  while (m.queue.length && m.queue[0].at < m.ctx.currentTime - half) m.queue.shift();
  if (m.queue.length !== before) t.drawUpcoming?.();
  requestAnimationFrame(followTicks);
}

const keyLabel = (e) => (e.key.length === 1 && e.key !== " " ? e.key.toUpperCase() : e.code.replace(/^Key|^Digit/, ""));

// the times are the keyboard events', not when this code got to them
function trainerKey(e, down) {
  const t = trainer;
  if (!t || settingsOpen || notesOpen || board) return;
  if (e.target instanceof HTMLInputElement) return;
  if (down && t.binding != null) {
    if (e.code === "Escape") return;
    e.preventDefault();
    const i = t.binding;
    t.codes[i] = e.code;
    t.labels[i] = keyLabel(e);
    if (t.codes[1 - i] === e.code) t.codes[1 - i] = t.labels[1 - i] = null;
    t.binding = t.codes[1 - i] ? null : 1 - i;
    saveTrainerKeys();
    t.refresh?.();
    return;
  }
  const i = t.codes.indexOf(e.code);
  if (i < 0) return;
  e.preventDefault();
  if (!down) {
    t.held.delete(i);
    t.refresh?.();
    return;
  }
  if (e.repeat || t.held.has(i)) return;
  t.held.add(i);
  const p = t.pending;
  if (p && p.key !== i && e.timeStamp - p.at <= CHORD_MS) {
    // like the analysis: the first column's press minus the second's, above zero the second led
    const gap = i === 1 ? p.at - e.timeStamp : e.timeStamp - p.at;
    t.chords.push({ gap, tempo: t.metro.running ? tempoName(t.metro) : null, after: p.after });
    t.lastKind = "chord";
    t.pending = null;
  } else {
    if (p) t.lastKind = p.key;
    const fresh = t.lastAt != null && e.timeStamp - t.lastAt < PAUSE_MS;
    t.pending = { key: i, at: e.timeStamp, after: fresh ? t.lastKind : null };
  }
  t.lastAt = e.timeStamp;
  t.refresh?.();
}
document.addEventListener("keydown", (e) => trainerKey(e, true));
document.addEventListener("keyup", (e) => trainerKey(e, false));

function leadText(gap, fingers) {
  if (Math.abs(gap) < TOGETHER_MS) return "together";
  return `${gap > 0 ? fingers[1] : fingers[0]} first by ${msText(gap)} ms`;
}

function chordStats(gaps) {
  const m = gaps.reduce((a, b) => a + b, 0) / gaps.length;
  const sd = Math.sqrt(gaps.reduce((a, b) => a + (b - m) ** 2, 0) / gaps.length);
  const together = gaps.filter((g) => Math.abs(g) < TOGETHER_MS).length / gaps.length;
  return { m, sd, together };
}

function metronomeBar(t) {
  const m = t.metro;
  const setBpm = (bpm) => {
    m.bpm = Math.max(30, Math.min(400, Math.round(bpm) || m.bpm));
    m.atTempo = 0;
    saveTempo();
    t.refresh();
  };
  const bpm = el("input", { class: "trainer-bpm", type: "number", min: 30, max: 400, step: 5, value: m.bpm, "aria-label": "BPM" });
  bpm.addEventListener("change", () => {
    bpm.blur();
    setBpm(Number(bpm.value));
  });
  return el("div", { class: "trainer-metro" },
    el("button", { class: `trainer-go${m.running ? " on" : ""}`, onclick: () => (m.running ? stopMetronome() : startMetronome()) }, m.running ? "Stop" : "Start"),
    el("div", { class: "trainer-tempo" },
      el("button", { class: "seg", title: `${RAMP_BPM} BPM slower`, onclick: () => setBpm(m.bpm - RAMP_BPM) }, "−"),
      bpm,
      el("span", {}, "BPM"),
      el("button", { class: "seg", title: `${RAMP_BPM} BPM faster`, onclick: () => setBpm(m.bpm + RAMP_BPM) }, "+")),
    segs(DIVISIONS, m.div, (div) => {
      m.div = div;
      m.atTempo = 0;
      saveTempo();
      t.refresh();
    }, "Chords per beat"),
    segs([[false, "Steady"], [true, "Speed up"]], m.ramp, (ramp) => {
      m.ramp = ramp;
      m.atTempo = 0;
      saveTempo();
      t.refresh();
    }, `Speed up by ${RAMP_BPM} BPM every ${RAMP_TICKS} ticks`),
    segs([["chords", "Chords"], ["patterns", "Patterns"]], t.mode, (mode) => {
      t.mode = mode;
      m.queue = m.queue.filter((q) => q.at <= m.ctx?.currentTime + 0.12);
      if (m.running) m.next = (m.queue.at(-1)?.at ?? m.ctx.currentTime) + 60 / m.bpm / m.div;
      saveTempo();
      t.refresh();
    }, "Patterns mix in a key alone now and then, like a jumpjack"),
    m.silent && el("p", { class: "trainer-silent" }, NO_SOUND[snapshot.soundHelp] ?? "No sound. Check that your computer has a working audio output."));
}

const noteText = (t, note) => (note === "chord" ? `${t.labels[0] ?? "?"}${t.labels[1] ?? "?"}` : t.labels[note] ?? "?");

function rowsTable(head, rows, note) {
  return el("div", { class: "trainer-table" },
    el("div", { class: "trainer-row head" }, head.map((h) => el("span", {}, h))),
    rows.map((r) => el("div", { class: `trainer-row${r.mark ? " breaks" : ""}`, title: r.mark ?? null }, r.cells.map((c) => el("span", {}, c)))),
    note && el("p", { class: "caption" }, note));
}

const leadCell = (t, m) => (Math.abs(m) < TOGETHER_MS ? "0" : `${msText(m)} ${(m > 0 ? t.fingers[1] : t.fingers[0]).split(" ")[1]}`);
const statCells = (t, s, n) => [n, leadCell(t, s.m), `±${msText(s.sd)}`, `${Math.round(s.together * 100)}%`];

function tempoTable(t) {
  const by = new Map();
  for (const c of t.chords) if (c.tempo) by.set(c.tempo, [...(by.get(c.tempo) ?? []), c.gap]);
  const rows = [...by].filter(([, gaps]) => gaps.length >= 4)
    .map(([tempo, gaps]) => ({ tempo, n: gaps.length, s: chordStats(gaps), rate: Number(tempo.split(" ")[0]) * Number(tempo.split("/")[1]) }))
    .sort((a, b) => a.rate - b.rate);
  if (!rows.length) return null;
  const breaks = rows.find((r) => Math.abs(r.s.m) >= ROLL_MS);
  return rowsTable(["Tempo", "Chords", "Lead", "Spread", "Together"],
    rows.map((r) => ({ cells: [r.tempo, ...statCells(t, r.s, r.n)], mark: r === breaks ? "Your roll starts here" : null })),
    breaks
      ? `From ${breaks.tempo} on, one finger leads by ${ROLL_MS} ms or more. Practice just under it, then nudge it up.`
      : `No tempo here leads by ${ROLL_MS} ms or more yet. Go faster to find where it starts.`);
}

// in a jumpjack a finger is often still coming off its key alone
function contextTable(t) {
  const kinds = [["chord", "After a chord"], [0, `After ${t.fingers[0].split(" ")[1]} alone`], [1, `After ${t.fingers[1].split(" ")[1]} alone`]];
  const rows = kinds.map(([kind, name]) => {
    const gaps = t.chords.filter((c) => c.after === kind).map((c) => c.gap);
    return gaps.length >= 4 ? { name, n: gaps.length, s: chordStats(gaps) } : null;
  }).filter(Boolean);
  if (rows.length < 2) return null;
  const worst = rows.reduce((a, b) => (b.s.sd > a.s.sd ? b : a));
  const best = rows.reduce((a, b) => (b.s.sd < a.s.sd ? b : a));
  const clear = worst.s.sd >= best.s.sd * 1.5 && worst.s.sd - best.s.sd >= 3;
  return rowsTable(["Before it", "Chords", "Lead", "Spread", "Together"],
    rows.map((r) => ({ cells: [r.name, ...statCells(t, r.s, r.n)], mark: clear && r === worst ? "Your chords come apart here" : null })),
    clear
      ? `Your chords come apart most ${worst.name.toLowerCase()}: ±${msText(worst.s.sd)} ms, against ±${msText(best.s.sd)} ms ${best.name.toLowerCase()}.`
      : "What comes before a chord doesn't change it much here.");
}

function trainerView() {
  const t = trainer;
  const caps = [0, 1].map((i) => el("button", {
    class: "trainer-cap",
    title: "Click, then press the key you play this column with",
    onclick: () => {
      t.binding = i;
      t.refresh();
    },
  }));
  const hands = el("div");
  const last = el("p", { class: "trainer-last" });
  const upcoming = el("div", { class: "trainer-upcoming" });
  const strip = el("div", { class: "trainer-strip" });
  const stats = el("p", { class: "caption" });
  const prompt = el("p", { class: "caption" });
  const metro = el("div");
  const tables = el("div");
  t.drawUpcoming = () => {
    const m = t.metro;
    upcoming.hidden = !(m.running && t.mode === "patterns");
    if (upcoming.hidden) return;
    upcoming.replaceChildren(...m.queue.slice(0, UPCOMING).map((q, i) => el("span", { class: `trainer-note${i === 0 ? " now" : ""}${q.note === "chord" ? " chord" : ""}` }, noteText(t, q.note))));
  };
  t.refresh = () => {
    hands.replaceChildren(segs([["left", "Left hand"], ["right", "Right hand"]].filter(([h]) => trainerPair(t.d, h)), t.hand, (hand) => {
      setTrainerHand(hand);
      t.refresh();
    }, "The hand to practice"));
    caps.forEach((cap, i) => {
      cap.classList.toggle("on", t.held.has(i));
      cap.classList.toggle("binding", t.binding === i);
      cap.replaceChildren(
        el("b", {}, t.binding === i ? "press a key" : t.labels[i] ?? "set key"),
        el("span", {}, `column ${t.columns[i] + 1}, ${t.fingers[i]}`));
    });
    const patterns = t.metro.running && t.mode === "patterns";
    prompt.textContent = t.binding != null
      ? `Press the key you play column ${t.columns[t.binding] + 1} with.`
      : patterns
        ? "Press what the first box says on each tick: both keys as one chord, or the one key alone."
        : t.metro.running
          ? "Press both keys on each tick, as one chord."
          : "Press both keys together, the way you would in a chord. Adjust your hand between chords and watch the gap, or start the metronome to see when speed brings the roll back.";
    const lastChord = t.chords.at(-1);
    last.replaceChildren(lastChord == null ? "No chords yet" : leadText(lastChord.gap, t.fingers));
    last.classList.toggle("good", lastChord != null && Math.abs(lastChord.gap) < TOGETHER_MS);
    t.drawUpcoming();
    strip.replaceChildren(trainerStrip(t));
    const play = t.play ? ` In the play it was ${leadText(t.play.mean, t.fingers)} on average, ±${msText(t.play.sd)} ms.` : "";
    const recent = t.chords.slice(-STATS_CHORDS).map((c) => c.gap);
    if (recent.length < 5) {
      stats.textContent = `A hand pressing as one stays within a couple of ms.${play}`;
    } else {
      const s = chordStats(recent);
      stats.textContent = `Your last ${recent.length} chords: ${leadText(s.m, t.fingers)} on average, ±${msText(s.sd)} ms, ${Math.round(s.together * 100)}% together.${play}`;
    }
    // keep the BPM field's focus and half-typed value across redraws
    const typing = document.activeElement?.classList.contains("trainer-bpm") && metro.contains(document.activeElement);
    if (!typing) metro.replaceChildren(metronomeBar(t));
    else metro.querySelector(".trainer-go")?.replaceWith(metronomeBar(t).querySelector(".trainer-go"));
    tables.replaceChildren(...[contextTable(t), tempoTable(t)].filter(Boolean));
  };
  t.refresh();
  return el("div", { class: "trainer" },
    el("div", { class: "head" },
      el("p", { class: "say" }, "Chord trainer"),
      el("button", { class: "link", onclick: closeTrainer }, "Back to the analysis")),
    hands,
    prompt,
    el("div", { class: "trainer-caps" }, caps),
    upcoming,
    last,
    strip,
    stats,
    metro,
    tables,
    el("button", { class: "link", onclick: () => { t.chords = []; t.refresh(); } }, "Start over"));
}

function trainerStrip(t) {
  const width = 480;
  const height = 74;
  const reach = 40;
  const x = (g) => width / 2 + (Math.max(-reach, Math.min(reach, g)) / reach) * (width / 2 - 14);
  const root = svg("svg", { class: "chart", viewBox: `0 0 ${width} ${height}`, width: "100%", role: "img", "aria-label": "How far apart each chord's keys went down" });
  root.append(svg("rect", { class: "trainer-band", x: x(-TOGETHER_MS), y: 14, width: x(TOGETHER_MS) - x(-TOGETHER_MS), height: 32, rx: 3 }));
  root.append(svg("line", { class: "trainer-axis", x1: 6, x2: width - 6, y1: 30, y2: 30 }));
  for (const g of [-reach, -20, 0, 20, reach]) {
    root.append(svg("text", { class: "tick-label", x: x(g), y: 66, "text-anchor": "middle" }, g === 0 ? "0" : `${Math.abs(g)}${Math.abs(g) === reach ? "+" : ""}`));
  }
  root.append(svg("text", { class: "edge-label", x: 6, y: 8 }, `${t.fingers[0]} first`));
  root.append(svg("text", { class: "edge-label", x: width - 6, y: 8, "text-anchor": "end" }, `${t.fingers[1]} first`));
  const shown = t.chords.slice(-SHOWN_CHORDS);
  shown.forEach(({ gap }, i) => {
    const age = shown.length - 1 - i;
    root.append(svg("circle", {
      class: `trainer-dot${Math.abs(gap) < TOGETHER_MS ? " together" : ""}`,
      cx: x(gap),
      cy: 30,
      r: age === 0 ? 6 : 4,
      style: `opacity: ${Math.max(0.15, 1 - age / SHOWN_CHORDS)}`,
    }));
  });
  return root;
}
