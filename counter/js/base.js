// What every view uses: the helper's address and state, building elements, saving a setting, icons and number formats.

const OFFICIAL_ORIGIN = "https://mania-tracker.com";
const BRIDGE = `http://127.0.0.1:${new URLSearchParams(location.search).get("bridge") || 24071}`;
const app = document.getElementById("app");

let snapshot = null;
let helperDown = false;
let busy = false;
// controls only dim when a save takes a while, a quick one would just flicker them
let slow = false;
let unsaved = null;

function el(tag, attrs = {}, ...children) {
  const node = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (v == null || v === false) continue;
    if (k.startsWith("on")) node.addEventListener(k.slice(2), v);
    else if (k === "class") node.className = v;
    else if (k === "style") node.style.cssText = v;
    else node.setAttribute(k, v === true ? "" : v);
  }
  for (const c of children.flat(Infinity)) {
    if (c == null || c === false) continue;
    node.append(c instanceof Node ? c : document.createTextNode(String(c)));
  }
  return node;
}

// 12/24h from the system locale
function time(iso) {
  const clock = snapshot.clock ?? {};
  let hour12 = clock.hour12;
  if (typeof hour12 !== "boolean") {
    try { hour12 = new Intl.DateTimeFormat(clock.locale ?? undefined, { hour: "numeric" }).resolvedOptions().hour12; } catch {}
  }
  return new Date(iso).toLocaleTimeString("en-US", { hour: hour12 ? "numeric" : "2-digit", minute: "2-digit", hour12: !!hour12 });
}

async function action(name) {
  if (busy) return;
  busy = true;
  unsaved = null;
  lockedHint = null;
  const dim = setTimeout(() => {
    slow = true;
    render();
  }, 300);
  try {
    const res = await fetch(`${BRIDGE}/${name}`, { method: "POST" });
    if (res.ok) snapshot = await res.json();
    else if (res.status === 500) unsaved = (await res.json().catch(() => null))?.error ?? "Couldn't save that.";
  } catch {
    helperDown = true;
  } finally {
    clearTimeout(dim);
    busy = slow = false;
    render();
  }
}

const scroller = () => (document.documentElement.classList.contains("framed") ? app : document.scrollingElement);

function stored(key, fallback) {
  try { return JSON.parse(localStorage.getItem(key)) ?? fallback; } catch { return fallback; }
}

function store(key, value) {
  try { localStorage.setItem(key, JSON.stringify(value)); } catch {}
}

const SVG_NS = "http://www.w3.org/2000/svg";

const msText = (x) => (Math.abs(x) < 9.95 ? Math.abs(x).toFixed(1) : Math.abs(x).toFixed(0));
const signedMs = (x) => (Math.abs(x) < 0.05 ? "0" : `${x > 0 ? "+" : "−"}${msText(x)}`);
const clock = (s) => `${Math.floor(s / 60)}:${String(Math.floor(s % 60)).padStart(2, "0")}`;
// rounded down like osu! does, 99.996% with a Good in it isn't 100%
function accText(x, digits = 2) {
  const f = 10 ** digits;
  return (Math.floor(x * f + 1e-6) / f).toFixed(digits);
}
const pct = (x) => (x == null ? "–" : `${accText(x)}%`);
// the same cutoffs the tips use before calling something early or late
const leanWord = (x, near) => (Math.abs(x) < near ? "on time" : x > 0 ? "late" : "early");

function svg(tag, attrs = {}, ...children) {
  const node = document.createElementNS(SVG_NS, tag);
  for (const [k, v] of Object.entries(attrs)) if (v != null && v !== false) node.setAttribute(k, v);
  for (const c of children.flat()) if (c != null && c !== false) node.append(c instanceof Node ? c : document.createTextNode(String(c)));
  return node;
}

function heading(title, ...extra) {
  const node = el("div", { class: "part-head" }, el("h3", {}, title), extra);
  node.querySelector("h3").insertAdjacentHTML("beforeend", `<svg class="squiggle" viewBox="0 0 64 8" width="64" height="8" aria-hidden="true"><path d="M2 5q7-5 14 0t14 0t14 0t14 0"/></svg>`);
  return node;
}

function segs(options, current, pick, label) {
  return el("div", { class: "segs", title: label },
    options.map(([key, text]) => el("button", {
      class: `seg${current === key ? " on" : ""}`,
      "aria-pressed": String(current === key),
      disabled: slow || analysis?.loading,
      onclick: () => current !== key && pick(key),
    }, text)));
}

function part(area, ...children) {
  return el("section", { class: `part ${area}` }, children);
}

const GEAR = `<path d="M4.10 0.39L5.90 0.39L6.07 1.88L7.16 2.51L8.55 1.92L9.44 3.47L8.24 4.37L8.24 5.63L9.44 6.53L8.55 8.08L7.16 7.49L6.07 8.12L5.90 9.61L4.10 9.61L3.93 8.12L2.84 7.49L1.45 8.08L0.56 6.53L1.76 5.63L1.76 4.37L0.56 3.47L1.45 1.92L2.84 2.51L3.93 1.88z" stroke-linejoin="round"/><circle cx="5" cy="5" r="1.4"/>`;
const MAXIMIZE = `<rect x=".5" y=".5" width="9" height="9"/>`;
const PODIUM = `<path d="M3.5 9.5v-5h3v5M.5 9.5v-3h3M6.5 9.5v-4h3v4M0 9.5h10M5 1l.5 1h1l-.8.7.3 1-1-.6-1 .6.3-1-.8-.7h1z" stroke-linejoin="round"/>`;
const TARGET = `<circle cx="5" cy="5" r="4.5"/><circle cx="5" cy="5" r="2.5"/><circle cx="5" cy="5" r=".6" fill="currentColor"/>`;
const TRAY = `<path d="M1 1l6 6M7 3v4H3M.5 9.5h9"/>`;
const CHART = `<path d="M1 9.5h8M2.5 7.5v-2M5 7.5V1.5M7.5 7.5v-4" stroke-linecap="round"/>`;
const PIN = `<path class="head" d="M3 .5h4V4l1.5 1.5h-7L3 4z" stroke-linejoin="round"/><path d="M5 5.5v4"/>`;

function windowAction(name) {
  fetch(`${BRIDGE}/window/${name}`, { method: "POST" }).catch(() => {});
}

function icon(paths) {
  const node = el("span", { class: "icon" });
  node.innerHTML = `<svg viewBox="0 0 10 10" width="10" height="10" fill="none" stroke="currentColor" stroke-width="1">${paths}</svg>`;
  return node;
}
