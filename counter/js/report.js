// a letter the other way: written here and sent off in a bottle along the shore
let reportOpen = false;
const REPORT_DRAFT = "hashi-report-draft";
const REPORT_MIN = 10;
const BOTTLE_DRIFT_MS = 7000;
let report = null;

const CLIP_GLYPH = `<path d="M9.5 4.5 5 9q-1.6 1.6-3.2 0t0-3.2L7.6.2q1.1-1.1 2.2 0t0 2.2L4.4 7.8q-.5.5-1 0t0-1L8 2.2"/>`;
const TICK_GLYPH = `<path class="box" d="M1.5 2q4.5-.6 10 0 .6 5 0 10-5 .5-10 0-.6-5 0-10z"/><path class="tick" d="M3.6 7.4l2.4 2.6 5-7.8"/>`;

function openReport() {
  reportOpen = true;
  report = { text: stored(REPORT_DRAFT, ""), log: true, preview: undefined, peek: false, sending: false, error: null, sentAt: 0, area: null };
  settingsOpen = notesOpen = goalsOpen = false;
  analysis = maker = null;
  dropTrainer();
  dropCutter();
  skinPage = profilePage = null;
  board = null;
  clearTimeout(boardTimer);
  render();
  loadLogPreview();
}

function closeReport() {
  reportOpen = false;
  render();
}

async function loadLogPreview() {
  const mine = report;
  try {
    const res = await fetch(`${BRIDGE}/report/log`, { method: "POST" });
    mine.preview = res.ok ? await res.json() : null;
  } catch {
    mine.preview = null;
  }
  if (report === mine) render();
}

const enough = () => report.text.trim().length >= REPORT_MIN;

async function sendReport() {
  if (report.sending || !enough()) return;
  const mine = report;
  mine.sending = true;
  mine.error = null;
  render();
  try {
    const res = await fetch(`${BRIDGE}/report`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ text: mine.text, log: mine.log && !!mine.preview }),
    });
    const answer = await res.json().catch(() => ({}));
    if (!res.ok) throw new Error(answer.error ?? "Couldn't send the report.");
    store(REPORT_DRAFT, "");
    mine.answer = answer;
    mine.sentAt = Date.now();
    if (!STILL.matches) setTimeout(() => report === mine && render(), BOTTLE_DRIFT_MS + 50);
  } catch (e) {
    mine.error = e instanceof TypeError ? "Lost Hashi's helper. Try again in a moment." : e.message;
  } finally {
    mine.sending = false;
    if (report === mine) render();
  }
}

function bytesText(n) {
  return n < 1024 ? `${n} B` : n < 1024 * 1024 ? `${Math.round(n / 1024)} KB` : `${(n / 1024 / 1024).toFixed(1)} MB`;
}

function inkIcon(cls, paths) {
  const node = el("span", { class: cls });
  node.innerHTML = `<svg viewBox="0 0 14 14" width="14" height="14" aria-hidden="true">${paths}</svg>`;
  return node;
}

function logTag() {
  const p = report.preview;
  if (!p) return null;
  return el("div", { class: "log-tag" },
    el("button", {
      class: `attach${report.log ? " on" : ""}`,
      "aria-pressed": String(report.log),
      disabled: report.sending,
      onclick: () => { report.log = !report.log; render(); },
    }, inkIcon("check", TICK_GLYPH), inkIcon("clip", CLIP_GLYPH), el("span", {}, "Hashi's log"), el("small", {}, bytesText(p.bytes))),
    el("button", { class: "link", onclick: () => { report.peek = !report.peek; render(); } }, report.peek ? "Hide it" : "See it"));
}

function logPeek() {
  const p = report.preview;
  if (!report.peek || !p) return null;
  const box = el("pre", {}, p.tail || "Empty.");
  requestAnimationFrame(() => (box.scrollTop = box.scrollHeight));
  return el("div", { class: "log-peek" }, box);
}

function grow(area) {
  area.style.height = "auto";
  area.style.height = `${area.scrollHeight}px`;
}

// kept across redraws: a new one each time would drop the caret, and Ctrl+A would take the page
function writing() {
  if (!report.area) {
    const area = el("textarea", {
      class: "write",
      rows: 6,
      maxlength: 4000,
      placeholder: "What happened?",
      "aria-label": "What went wrong",
      oninput: () => {
        report.text = area.value;
        store(REPORT_DRAFT, report.text);
        grow(area);
        document.querySelector(".report .send")?.toggleAttribute("disabled", !enough());
      },
    });
    area.value = report.text;
    report.area = area;
  }
  report.area.disabled = report.sending;
  requestAnimationFrame(() => grow(report.area));
  return report.area;
}

const LADYBUG = `<g class="legs"><path d="M12 17l-7-4M11 23H3M12 29l-6 5"/><path d="M32 17l7-4M33 23h8M32 29l6 5"/></g>
  <path class="feelers" d="M20 8q-2-5-6-5M24 8q2-5 6-5"/>
  <path class="head" d="M16.5 13q0-7 5.5-7t5.5 7z"/>
  <path class="shell" d="M22 11c7 0 11 6 11 14s-5 12-11 12-11-4-11-12 4-14 11-14z"/>
  <path d="M22 12v24"/>
  <circle class="spot" cx="17" cy="21" r="2"/><circle class="spot" cx="27.5" cy="19.5" r="1.8"/><circle class="spot" cx="26.5" cy="29" r="2.2"/><circle class="spot" cx="17.5" cy="30" r="1.5"/>`;

function ladybug() {
  const node = el("span", { class: "ladybug" });
  node.innerHTML = `<svg viewBox="0 0 44 40" width="44" height="40" aria-hidden="true"><g style="${loop(900)}">${LADYBUG}</g></svg>`;
  return node;
}

function emptyBottle() {
  const node = el("span", { class: "bottle-doodle" });
  node.innerHTML = `<svg viewBox="0 0 92 40" width="92" height="40" aria-hidden="true"><g class="ink">
    <path class="sand" d="M2 36q22-4 44-1t44-1"/>
    <path class="glass" d="M14 12h32q6 0 9 5h10v10H55q-3 5-9 5H14q-8 0-8-10t8-10z"/>
    <path class="glint" d="M13 15.5q5-1.5 12-1.5"/>
    <rect class="cork" x="74" y="24" width="8" height="8" rx="2" transform="rotate(24 78 28)"/>
  </g></svg>`;
  return node;
}

function reportPage() {
  const date = new Date().toLocaleDateString("en-US", DATE);
  const name = snapshot.connection.username;
  return [
    el("article", { class: "letter outgoing" },
      ladybug(),
      el("div", { class: "top" }, el("p", { class: "say" }, "Something went wrong"), el("span", {}, date)),
      writing(),
      el("div", { class: "foot" }, logTag(), name && el("span", { class: "sign" }, "— ", name))),
    logPeek(),
    report.error && el("p", { class: "status bad" }, report.error),
    el("div", { class: "send-row" },
      el("button", { class: "key go send", disabled: report.sending || !enough(), onclick: sendReport }, report.sending ? "Sending…" : "Send"),
      el("span", { class: "why" }, name
        ? "Replies show up on Mania Tracker."
        : "Not connected to Mania Tracker, so it goes without your name."),
      emptyBottle()),
  ];
}

function reportSent() {
  const { repliesUrl, logLost } = report.answer ?? {};
  return el("div", { class: "sent" },
    el("p", { class: "say" }, "Sent."),
    el("p", { class: "lead" }, repliesUrl
      ? ["Thanks. The reply shows up on ", el("a", { class: "link", href: repliesUrl, target: "_blank", rel: "noreferrer" }, "Mania Tracker"), "."]
      : "Thanks, it's on its way."),
    logLost && el("p", { class: "lead" }, "The log didn't make it, only what you wrote."),
    el("p", { class: "again" },
      el("button", { class: "link", onclick: openReport }, "Write another"),
      " · ",
      el("button", { class: "link", onclick: closeReport }, "Back to plays")));
}

function reportView() {
  return el("div", { class: "report" },
    el("div", { class: "head" },
      el("p", { class: "say" }, "Report a problem"),
      el("button", { class: "link", onclick: closeReport }, "Back to plays")),
    report.sentAt ? reportSent() : reportPage());
}

function sentBottle() {
  if (!reportOpen || !report?.sentAt || STILL.matches) return null;
  const since = Date.now() - report.sentAt;
  if (since > BOTTLE_DRIFT_MS) return null;
  const node = el("span", { class: "bottle away", style: `animation-delay: -${since}ms; animation-duration: ${BOTTLE_DRIFT_MS}ms` });
  node.innerHTML = `<svg viewBox="0 0 104 60" width="104" height="60" aria-hidden="true"><g class="ink">
    <g class="float" style="${loop(BOB_BOTTLE_MS)}"><g transform="rotate(-14 50 36)">
      <path class="glass" d="M26 26h32q6 0 9 5h10v10H67q-3 5-9 5H26q-8 0-8-10t8-10z"/>
      <rect class="paper" x="25" y="31" width="32" height="10" rx="2"/>
      <path class="ribbon" d="M41 31v10"/>
      <path class="glint" d="M25 29.5q5-1.5 12-1.5"/>
      <rect class="cork" x="77" y="32" width="8" height="8" rx="2"/>
    </g></g>
  </g></svg>`;
  return node;
}
