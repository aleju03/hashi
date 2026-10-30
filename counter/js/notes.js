// What's new, from changelog.json.

// what's new, from changelog.json, newest first. after an update a bottle washes up on the shore with it
let notes = [];
let notesOpen = false;
let notesAt = 0;
let uncorkAt = 0;
const UNCORK_MS = 420;
const BOB_BOTTLE_MS = 2800;
const NOTE_KINDS = [["new", "New"], ["fixed", "Fixed"], ["changed", "Changed"]];
const NOTE_GLYPHS = {
  new: `<path class="fill" d="M7 .8q1 5.2 6.2 6.2Q8 8 7 13.2 6 8 .8 7 6 6 7 .8z"/>`,
  fixed: `<g transform="rotate(-45 7 7)"><rect x="1" y="4.4" width="12" height="5.2" rx="2.6"/><path d="M5.4 4.4v5.2M8.6 4.4v5.2"/></g>`,
  changed: `<path d="M2 11q.4-6.6 8-6.3M7.6 2l2.6 2.7-2.6 2.7"/>`,
};

async function loadNotes() {
  try {
    const res = await fetch("changelog.json");
    if (res.ok) notes = await res.json();
  } catch {}
  render();
}

const thisVersion = () => snapshot.version.split(/[-+]/)[0];

function newer(a, b) {
  const [x, y] = [a, b].map((v) => v.split(".").map(Number));
  const at = x.findIndex((n, i) => n !== y[i]);
  return at >= 0 && x[at] > y[at];
}

// the changelog can already have the next version written down
function shownNotes() {
  return notes.filter((n) => !newer(n.version, thisVersion()));
}

function bottleVersion() {
  if (!snapshot.unreadNotes || settingsOpen || notesOpen) return null;
  return notes.find((n) => n.version === thisVersion())?.version;
}

function openNotes() {
  notesOpen = true;
  notesAt = Date.now();
  uncorkAt = 0;
  settingsOpen = false;
  analysis = maker = null;
  dropTrainer();
  skinPage = profilePage = null;
  board = null;
  clearTimeout(boardTimer);
  lockedHint = null;
  if (snapshot.unreadNotes) action("notes/read");
  render();
}

function closeNotes() {
  notesOpen = false;
  render();
}

function uncork() {
  if (uncorkAt) return;
  if (STILL.matches) return openNotes();
  uncorkAt = Date.now();
  render();
  setTimeout(openNotes, UNCORK_MS);
}

function bottle(version) {
  const popped = uncorkAt ? ` style="animation-delay: -${Date.now() - uncorkAt}ms"` : "";
  const node = el("button", { class: `bottle${uncorkAt ? " uncorked" : ""}`, title: `What's new in Hashi ${version}`, onclick: uncork });
  node.innerHTML = `<svg viewBox="0 0 104 60" width="104" height="60" aria-hidden="true"><g class="ink">
    <g class="ripples" style="${loop(BOB_BOTTLE_MS)}"><path d="M2 51q4-3 8 0t8 0"/><path d="M82 51q4-3 8 0t8 0"/></g>
    <g class="float" style="${loop(BOB_BOTTLE_MS)}"><g transform="rotate(-14 50 36)">
      <path class="glass" d="M26 26h32q6 0 9 5h10v10H67q-3 5-9 5H26q-8 0-8-10t8-10z"/>
      <rect class="paper" x="25" y="31" width="32" height="10" rx="2"/>
      <path class="ribbon" d="M41 31v10"/>
      <path class="glint" d="M25 29.5q5-1.5 12-1.5"/>
      <rect class="cork" x="77" y="32" width="8" height="8" rx="2"${popped}/>
    </g></g>
    <path class="glimmer" d="M91 14l3-5M96 21l5-1M85 10l-1-5" style="${loop(1800)}"/>
  </g></svg>`;
  node.append(el("span", { class: "bottle-say" }, "What's new in ", el("b", {}, version)));
  return node;
}

function noteDate(n, form) {
  return n.date ? new Date(`${n.date}T12:00:00`).toLocaleDateString("en-US", form) : null;
}

function change(c, style) {
  const glyph = el("span", { class: "glyph" });
  glyph.innerHTML = `<svg viewBox="0 0 14 14" width="14" height="14" aria-hidden="true">${NOTE_GLYPHS[c.kind] ?? NOTE_GLYPHS.changed}</svg>`;
  return el("li", { class: c.kind, style }, glyph, el("span", {}, c.text));
}

const DATE = { month: "short", day: "numeric", year: "numeric" };

function letter(n) {
  const since = Date.now() - notesAt;
  let line = 0;
  const groups = NOTE_KINDS.map(([kind, label]) => {
    const items = n.changes.filter((c) => c.kind === kind);
    return items.length > 0 && [
      el("p", { class: "kind" }, label),
      el("ul", { class: "changes" }, items.map((c) => change(c, `animation-delay: ${400 + line++ * 90 - since}ms`))),
    ];
  });
  const date = noteDate(n, DATE);
  return el("article", { class: "letter", style: `animation-delay: -${since}ms` },
    el("div", { class: "top" }, el("p", { class: "say" }, `Hashi ${n.version}`), date && el("span", {}, date)),
    groups);
}

function pastNotes(n) {
  const date = noteDate(n, DATE);
  return el("section", { class: "past" },
    el("div", { class: "when" }, el("b", {}, n.version), date && el("span", {}, date)),
    el("ul", { class: "changes" }, n.changes.map((c) => change(c))));
}

function notesView() {
  const [latest, ...earlier] = shownNotes();
  return el("div", { class: "notes" },
    el("div", { class: "head" },
      el("p", { class: "say" }, "What's new"),
      el("button", { class: "link", onclick: closeNotes }, "Back to plays")),
    latest ? letter(latest) : el("p", { class: "lead" }, "Nothing written down yet."),
    earlier.length > 0 && [el("p", { class: "group" }, "Before that"), earlier.map(pastNotes)]);
}
