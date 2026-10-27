const OFFICIAL_ORIGIN = "https://mania-tracker.com";
const BRIDGE = `http://127.0.0.1:${new URLSearchParams(location.search).get("bridge") || 24071}`;
const app = document.getElementById("app");

let snapshot = null;
let helperDown = false;
let busy = false;
// controls only dim when a save takes a while, a quick one would just flicker them
let slow = false;
let unsaved = null;

// kind: spin, done, wait, fail, off
function playStatus(p) {
  const r = p.receipt;
  const retryAt = p.retryAt && `Trying again at ${time(new Date(p.retryAt).toISOString())}`;
  if (p.phase === "practice") return { kind: "done", label: "Practice", detail: "Kept in Hashi, not sent" };
  if (p.onHold) return { kind: "wait", label: "On hold", detail: "Not sent until you send it." };
  if (!snapshot.sendPlays && p.phase === "send") return { kind: "wait", label: "Not sending", detail: "Sending plays is off." };
  if (p.phase === "waiting_replay") return { kind: "spin", label: "Waiting for replay" };
  if (p.phase === "unbound") return { kind: "off", label: "Not sent", detail: "Wasn't connected when you played" };
  if (p.phase === "held") return { kind: "wait", label: "On hold", detail: p.error?.message };
  if (p.phase === "failed") return { kind: "fail", label: "Not sent", detail: p.error?.message };
  if (p.error?.code === "retrying") return { kind: "wait", label: "Retrying", detail: retryAt };
  if (p.error?.code === "gone") return { kind: "fail", label: "Not submitted", detail: p.error.message };
  switch (r?.state) {
    case undefined:
    case "awaiting_assets": return { kind: "spin", label: "Uploading" };
    case "queued": return { kind: "spin", label: "Submitting" };
    case "validating":
    case "analyzing": return { kind: "spin", label: "Processing" };
    case "deferred": return { kind: "spin", label: "Processing", detail: "Taking a little longer than usual" };
    case "accepted": return { kind: "done", label: "Submitted" };
    case "rejected":
      if (r.error?.code === "duplicate_play") return { kind: "done", label: "Already submitted" };
      return { kind: "fail", label: "Not submitted", detail: r.error?.message };
    case "expired": return { kind: "fail", label: "Not submitted", detail: "The upload took too long" };
    case "deleted": return { kind: "off", label: "Removed" };
    default: return { kind: "spin", label: "Processing" };
  }
}

const SPIN_MS = 900;
const covers = new Map();

// only stamp plays that change while on screen, not everything at startup
const stamped = new Map();
const stampAt = new Map();

function stamp(status, id) {
  const before = stamped.get(id);
  stamped.set(id, status.label);
  if (status.kind === "spin") {
    return el("span", { class: "doing" },
      el("i", { class: "spinner", style: `animation-delay: -${Date.now() % SPIN_MS}ms` }), status.label);
  }
  if (before !== undefined && before !== status.label) stampAt.set(id, Date.now());
  const tilt = -2 - [...id].reduce((sum, ch) => sum + ch.charCodeAt(0), 0) % 6;
  return el("span", {
    class: `stamp ${status.kind}`,
    style: `--tilt: ${tilt}deg; animation-delay: -${Date.now() - (stampAt.get(id) ?? 0)}ms`,
  }, status.label);
}

// icons and colors from the site's ModBadge
const MOD_FILES = {
  HD: "hidden", HR: "hard-rock", DT: "double-time", FL: "flashlight", EZ: "easy", NF: "no-fail",
  HT: "half-time", NC: "nightcore", SD: "sudden-death", PF: "perfect", FI: "fade-in", RD: "random",
  MR: "mirror", SV2: "score-v2", TD: "touch-device", CN: "cinema", CO: "cover", DS: "dual-stages",
  AT: "autoplay", DC: "daycore", NR: "no-release", AC: "accuracy-challenge", IN: "invert", HO: "hold-off",
  CS: "constant-speed", CL: "classic", DA: "difficulty-adjust", WU: "wind-up", WD: "wind-down",
  MU: "muted", AS: "adaptive-speed",
  "1K": "one-key", "2K": "two-keys", "3K": "three-keys", "4K": "four-keys", "5K": "five-keys",
  "6K": "six-keys", "7K": "seven-keys", "8K": "eight-keys", "9K": "nine-keys", "10K": "ten-keys",
};
const MOD_COLORS = {
  red: ["HD", "HR", "DT", "FL", "NC", "SD", "PF", "FI", "CO", "AC"],
  lime: ["EZ", "NF", "HT", "DC", "NR"],
  purple: ["MR", "RD", "DS", "IN", "HO", "CS", "CL", "DA", "1K", "2K", "3K", "4K", "5K", "6K", "7K", "8K", "9K", "10K"],
  blue: ["CN", "AT", "RX", "AP"],
  pink: ["WU", "WD", "MU", "AS"],
  yellow: ["TD", "SV2"],
};
const COLOR = { red: "#ff6666", lime: "#b2ff66", purple: "#8c66ff", blue: "#66ccff", pink: "#ff66ab", yellow: "#ffcc22" };
const modColor = (mod) => COLOR[Object.keys(MOD_COLORS).find((c) => MOD_COLORS[c].includes(mod))] ?? COLOR.red;

// tosu gives "HDNC", "4KMR" etc
function modList(name) {
  const mods = name?.match(/10K|[1-9]K|SV2|[A-Z]{2}/g) ?? [];
  return mods.filter((m) => !(m === "DT" && mods.includes("NC")) && !(m === "SD" && mods.includes("PF")));
}

// the speed lazer's speed mods play at unless it's set to a custom one, which goes on the badge like lazer does
const DEFAULT_RATES = { DT: 1.5, NC: 1.5, HT: 0.75, DC: 0.75 };

// an object, not a bare rate, so .map(modBadge) can't pass the index as one
function modBadge(mod, { rate } = {}) {
  const file = MOD_FILES[mod];
  const custom = mod in DEFAULT_RATES && typeof rate === "number" && Math.abs(rate - DEFAULT_RATES[mod]) > 0.001;
  const tail = custom && `${rate.toFixed(2)}×`;
  const badge = el("span", { class: "mod", title: tail ? null : mod, style: `--mod: ${modColor(mod)}` },
    file
      ? el("i", { style: `-webkit-mask-image: url("images/mods/mod-${file}.svg"); mask-image: url("images/mods/mod-${file}.svg")` })
      : el("b", {}, mod));
  if (!tail) return badge;
  return el("span", { class: "mod-rated", title: `${mod} ${tail}`, style: `--mod: ${modColor(mod)}` }, badge, el("span", { class: "tail" }, tail));
}

function cover(p) {
  let node = covers.get(p.id);
  // the cover can turn up later, once the chart is found
  if (!node || (p.cover && !node.dataset.cover)) {
    node = el("div", { class: "cover" });
    if (p.cover) {
      node.dataset.cover = "yes";
      const img = new Image();
      img.onload = () => {
        node.style.setProperty("--cover", `url("${img.src}")`);
        node.classList.add("loaded");
      };
      img.src = `${BRIDGE}/cover/${encodeURIComponent(p.id)}`;
    }
    covers.set(p.id, node);
  }
  return node;
}

const OSU_STATES = {
  menu: "Main menu",
  selectPlay: "Song select",
  play: "Playing",
  resultScreen: "Results",
  edit: "Editor",
  selectEdit: "Editor",
  selectMulti: "Multiplayer",
  lobby: "Multiplayer",
  matchSetup: "Multiplayer",
  selectDrawings: "Song select",
  onlineSelection: "osu!direct",
};

const TOSU_HINT = "Start tosu to submit plays. Anything you play without it isn't sent.";

function gameName(tosu) {
  return tosu.client === "lazer" ? "osu!lazer" : "osu!";
}

function osuLabel(tosu) {
  return OSU_STATES[tosu.state] ?? String(tosu.state ?? "").replace(/([a-z])([A-Z])/g, "$1 $2").replace(/^./, (c) => c.toUpperCase());
}

function tosuDetail(tosu) {
  return { starting: "Starting", failed: "Couldn't start", missing: "Not found", "with-osu": "Opens with osu!" }[tosu.launch] ?? "Not running";
}

function blockedHint() {
  const { blocked } = snapshot.tosu;
  return blocked && el("div", { class: "blocked" },
    el("p", {}, "Linux blocks tosu from reading the game. Run this once, then restart Hashi:"),
    el("code", { title: "Click to copy", onclick: (e) => {
      navigator.clipboard?.writeText(blocked.fix);
      e.currentTarget.classList.add("copied");
    } }, blocked.fix));
}

function getTosu() {
  const g = snapshot.getTosu;
  if (!g) return null;
  const get = () => action("get-tosu");
  if (g.step === "downloading" || g.step === "unpacking") {
    return el("div", { class: "pinned" },
      el("p", {}, g.step === "unpacking"
        ? "Unpacking tosu…"
        : ["Downloading tosu", g.percent != null ? ` · ${g.percent}%` : "", "…"]),
      el("i", { class: "spinner", style: `animation-delay: -${Date.now() % SPIN_MS}ms` }));
  }
  if (g.step === "failed") {
    return el("div", { class: "pinned failed" },
      el("p", {}, "Couldn't get tosu. ", el("span", { class: "why" }, g.error)),
      el("button", { class: "key go", disabled: slow, onclick: get }, "Try again"));
  }
  return el("div", { class: "pinned" },
    el("p", {}, "Couldn't find tosu on this computer."),
    el("button", { class: "key go", disabled: slow, onclick: get }, "Get tosu"));
}

function tosuHint() {
  const { tosu } = snapshot;
  if (tosu.blocked) return blockedHint();
  if (snapshot.getTosu) return getTosu();
  if (tosu.connected || tosu.launch === "starting" || tosu.launch === "with-osu") return null;
  return el("p", { class: "status warn" }, TOSU_HINT);
}

// loops are offset from the clock so a re-render doesn't restart them
function loop(ms, at = 0) {
  return `animation-duration: ${ms}ms; animation-delay: -${(Date.now() + at * ms) % ms}ms`;
}

const LEAF_MS = 700;
const BOB_MS = 2400;
let leafPose = "up";
let leafAt = 0;

function leaf(pose) {
  if (pose !== leafPose) {
    leafPose = pose;
    leafAt = Date.now();
  }
  const moved = Math.min(Date.now() - leafAt, LEAF_MS);
  const bob = moved < LEAF_MS ? LEAF_MS - moved : -((Date.now() - leafAt - LEAF_MS) % BOB_MS);
  return `class="leaf ${pose}" style="animation-delay: -${moved}ms, ${bob}ms"`;
}

const STILL = matchMedia("(prefers-reduced-motion: reduce)");
const CROSS_MS = 1350;
let crossNoteMs = 1200;
let reach = 300;
let wasConnecting = false;
let crossAt = 0;
let crossTimer = null;

function cross() {
  crossAt = Date.now();
  clearTimeout(crossTimer);
  crossTimer = setTimeout(iris, CROSS_MS);
}

// plain fills under a clip-path, an ink filter over the whole window is too slow to animate
async function iris() {
  const sign = document.querySelector(".sign.landing circle")?.getBoundingClientRect();
  if (!crossAt || !sign) return;
  const from = { x: sign.x + sign.width / 2, y: sign.y + sign.height / 2, r: sign.width / 2 };
  const cover = el("div", { class: "iris" }, el("i"), el("i"));
  document.body.append(cover);
  const move = (a, b, timing) => Promise.all([...cover.children].map((layer, i) => {
    const clip = ({ x, y, r }) => `circle(${Math.max(0, r + (i ? -1.5 : 1.5))}px at ${x}px ${y}px)`;
    return layer.animate({ clipPath: [clip(a), clip(b)] }, { ...timing, fill: "forwards" }).finished;
  }));
  const full = { ...from, r: Math.hypot(Math.max(from.x, innerWidth - from.x), Math.max(from.y, innerHeight - from.y)) + 8 };
  await move(from, full, { duration: 380, easing: "cubic-bezier(0.5, 0, 0.9, 0.4)" });
  crossAt = 0;
  render();
  const face = document.querySelector(".me .face")?.getBoundingClientRect();
  const to = face ? { x: face.x + face.width / 2, y: face.y + face.height / 2, r: face.width / 2 } : { ...from, r: 0 };
  await move(full, to, { duration: 420, easing: "cubic-bezier(0.3, 0.6, 0.2, 1)" });
  await cover.animate({ opacity: [1, 0] }, { duration: 160, fill: "forwards" }).finished;
  cover.remove();
}

const NOTE_MS = 3600;

function bridge(rows, pose) {
  const crossed = Date.now() - crossAt;
  const landing = pose === "flat" && crossAt;
  const notes = pose !== "flat" && rows.osu.state === "ok" ? [0, 1, 2].map((i) => loop(NOTE_MS, i / 3)) : [];
  const overs = pose === "flat"
    ? [0, 1, 2].map((i) => `animation-duration: ${crossNoteMs}ms; animation-delay: ${150 + i * crossNoteMs / 3 - crossed}ms`)
    : [];
  // first note reaches the sign 72% into its loop
  const lit = 150 + crossNoteMs * 0.72;
  const water = `M${-reach} 246q10-3 20 0${"t20 0".repeat(reach / 10 + 19)}V300H${-reach}z`;
  // filled outside the wobble: webkitgtk's filters shift dark colors a little, and the river under it isn't filtered
  const drift = loop(4000);
  const node = el("div", { class: "scene" }, Object.entries(rows).map(([at, r]) =>
    el("div", { class: `tag at-${at} is-${r.state}` },
      el("b", {}, r.name),
      r.detail && el("span", { class: "detail" }, r.detail),
      r.sub && el("span", { class: "sub" }, r.sub))));
  node.insertAdjacentHTML("afterbegin", `<svg viewBox="0 44 400 224" aria-hidden="true"><g class="ink">
    <path class="land" d="M${-reach} 200H54q10 1 12 14l8 60H${-reach}z"/>
    <path class="land" d="M${400 + reach} 200H346q-10 1-12 14l-8 60H${400 + reach}z"/>
    <path class="line" d="M69 233q40-31 83-7M331 233q-40-31-83-7"/>
    <rect class="solid" x="152" y="205" width="16" height="70"/>
    <rect class="solid" x="232" y="205" width="16" height="70"/>
    <g ${leaf(pose)}><rect class="solid" x="166" y="199" width="72" height="8" rx="2"/><path class="line" d="M184 199v8M202 199v8M220 199v8"/></g>
    <rect class="solid" x="46" y="200" width="122" height="7" rx="2"/>
    <rect class="solid" x="232" y="200" width="122" height="7" rx="2"/>
    <path class="solid" d="M229 200v-25l11-10 11 10v25z"/>
    <path class="line" d="M240 181v8"/>
    <g class="light is-${rows.tosu.state}">
      <path class="line" d="M130 200v-31"/>
      <rect class="solid" x="121" y="134" width="18" height="36" rx="8"/>
      <circle class="lamp stop" cx="130" cy="145" r="4.5" style="${loop(1000)}"/>
      <circle class="lamp go" cx="130" cy="159" r="4.5"/>
    </g>
    ${notes.map((style) => `<g class="note" style="${style}"><rect x="-11" y="-7" width="22" height="7" rx="1.5"/></g>`).join("")}
    ${overs.map((style) => `<g class="note over" style="${style}"><rect x="-11" y="-7" width="22" height="7" rx="1.5"/></g>`).join("")}
    <g class="sign is-${rows.osu.state}">
      <path class="line" d="M30 186v14"/>
      <circle cx="30" cy="164" r="22"/>
      <text x="30" y="168.5" text-anchor="middle">osu!</text>
    </g>
    <g class="sign is-${rows.site.state}${landing ? " landing" : ""}" style="--lit: ${lit}ms; --waited: -${crossed}ms">
      <path class="line" d="M370 186v14"/>
      <circle cx="370" cy="164" r="22"/>
      <path class="glyph" d="M356 159h12v-8l14 13-14 13v-8h-12z" style="${pose === "flat" ? "" : loop(1000)}"/>
    </g>
    ${landing ? `<path class="line spark" d="M-14-4l-7-8M0-9v-11M14-4l7-8" style="animation-delay: ${lit - crossed}ms"/>` : ""}
  </g>
    <path class="water" d="${water}" style="${drift}"/>
    <g class="ink">
    <path class="water edge" d="${water}" style="${drift}"/>
    <path class="ripple" d="M88 259q5-3 10 0t10 0M286 262q5-3 10 0t10 0M12 263q5-3 10 0"/>
    ${notes.map((style) => `<path class="line splash" d="M-8-2l-5-7M0-4v-9M8-2l5-7" style="${style}"/>`).join("")}
  </g></svg>`);
  return node;
}

function riverbed() {
  const node = el("div", { class: "bed" });
  node.innerHTML = `<svg viewBox="0 0 400 76" aria-hidden="true"><g class="ink">
    <g class="fish" style="--from: ${-reach + 250}px; --to: ${reach + 150}px; ${loop(44 * (reach * 2 - 100))}"><path d="M-10 0q9-9 19 0q-10 9-19 0zM-10 0l-7-5v10z"/><circle cx="4" cy="-1" r="1"/></g>
    <path class="line" d="M${-reach} 66q25-5 50 0${"t50 0".repeat(reach / 25 + 7)}"/>
    <path class="line" d="M322 65q-5-8 0-15t0-14M333 65q4-7 0-12t1-11M58 66q-4-6 0-11t-1-9"/>
    <rect class="sunk" x="148" y="57" width="22" height="7" rx="1.5" transform="rotate(-5 159 61)"/>
    <rect class="sunk" x="196" y="58" width="22" height="7" rx="1.5" transform="rotate(3 207 62)"/>
    <rect class="sunk" x="241" y="56" width="22" height="7" rx="1.5" transform="rotate(-12 252 60)"/>
  </g></svg>`;
  return node;
}

const KOFI = "https://ko-fi.com/aleju03";
const BOB_KOFI_MS = 3200;
const STEAM_MS = 2600;

// moored on the shore under the settings
function kofi() {
  const node = el("a", { class: "kofi", href: KOFI, target: "_blank", rel: "noreferrer", title: "Support on Ko-fi" });
  node.innerHTML = `<svg viewBox="0 0 92 60" width="110" height="72" aria-hidden="true"><g class="ink">
    <g class="ripples" style="${loop(BOB_KOFI_MS)}"><path d="M-6 47q4-3 8 0t8 0"/><path d="M76 47q4-3 8 0t8 0"/></g>
    <g class="boat" style="${loop(BOB_KOFI_MS)}">
      <path class="mast" d="M64 39V9"/>
      <path class="flag" d="M64 9l15 4.5l-15 4.5z"/>
      <g class="steam"><path d="M31 15q-3-4 0-7t0-7" style="${loop(STEAM_MS)}"/><path d="M39 16q3-4 0-7t1-7" style="${loop(STEAM_MS, 0.5)}"/></g>
      <path class="cup" d="M24 19h22v12q0 7-7 7h-8q-7 0-7-7z"/>
      <path class="handle" d="M46 22q7 0 7 5t-7 5"/>
      <path class="heart" d="M35 32q-6-4-6-7.5q0-3 3-3q2 0 3 2q1-2 3-2q3 0 3 3q0 3.5-6 7.5z"/>
      <path class="hull" d="M3 37q36 5 72 0l-8 13q-28 4-56 0z"/>
      <path class="plank" d="M11 44q28 3 56 0"/>
    </g>
  </g></svg>`;
  return el("span", { class: "kofi-dock" }, node, el("a", { class: "kofi-say", href: KOFI, target: "_blank", rel: "noreferrer" }, "Support on Ko-fi"));
}

function shore(boat = false, adrift = null) {
  const node = el("div", { class: "shore" });
  const width = Math.max(innerWidth, 400);
  // as wide as the window could ever get
  const water = Math.max(screen.width, width);
  node.innerHTML = `<svg width="100%" height="72" aria-hidden="true"><g class="still">
    <svg y="100%" overflow="visible">
      <path class="water" d="M-40 -66q10-3 20 0${"t20 0".repeat(Math.ceil(water / 20) + 6)}V10H-40z" style="${loop(9000)}"/>
      <g transform="translate(0 -56)"><g class="fish" style="--from: -40px; --to: ${width + 40}px; ${loop(88 * (width + 80))}"><path d="M-10 0q9-9 19 0q-10 9-19 0zM-10 0l-7-5v10z"/><circle cx="4" cy="-1" r="1"/></g></g>
      <path class="line" d="M28 0q-5-10 0-19t0-17t1-12M40 0q4-9 0-15t1-13M52 0q-3-6 0-11"/>
      <svg x="64%" overflow="visible">
        <rect class="sunk" x="-34" y="-11" width="22" height="7" rx="1.5" transform="rotate(-5 -23 -7)"/>
        <rect class="sunk" x="6" y="-10" width="22" height="7" rx="1.5" transform="rotate(4 17 -6)"/>
      </svg>
      <svg x="100%" overflow="visible">
        <path class="line" d="M-32 0q-4-9 0-16t0-14t1-10M-44 0q4-7 0-12t1-10"/>
        <rect class="sunk" x="-98" y="-12" width="22" height="7" rx="1.5" transform="rotate(-10 -87 -8)"/>
      </svg>
    </svg>
  </g></svg>`;
  if (boat) node.append(kofi());
  if (adrift) node.append(bottle(adrift));
  return node;
}

function rows(site) {
  const { tosu } = snapshot;
  const osuOpen = tosu.connected && tosu.osu;
  return {
    osu: osuOpen
      ? { state: "ok", name: gameName(tosu), detail: osuLabel(tosu), sub: tosu.player || "Guest" }
      : tosu.blocked
        ? { state: "warn", name: "osu!", detail: "Can't be read" }
        : { state: "idle", name: "osu!", detail: "Not open" },
    tosu: tosu.connected
      ? { state: "ok", name: "tosu", detail: "Running" }
      : { state: { starting: "spin", "with-osu": "idle" }[tosu.launch] ?? "warn", name: "tosu", detail: tosuDetail(tosu) },
    site,
  };
}

function intro(pose) {
  return bridge(rows({
    up: { state: "idle", name: "Mania Tracker", detail: "Not connected" },
    down: { state: "spin", name: "Mania Tracker", detail: "Waiting for you" },
    flat: { state: "ok", name: "Mania Tracker", detail: "Connected" },
  }[pose]), pose);
}

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

// mappers often name the difficulty "[7K] Insane" already
function diffName(keys, version) {
  const k = keys ? `${Math.round(keys)}K` : null;
  if (k && version && new RegExp(`\\b${k}\\b`, "i").test(version)) return version;
  return [k, version].filter(Boolean).join(" ");
}

function chartName(play) {
  const diff = diffName(play.keys, play.version);
  return diff ? `${play.title ?? "Unknown"} [${diff}]` : play.title ?? "Unknown";
}

function playRow(p) {
  const status = playStatus(p);
  const diff = diffName(p.play.keys, p.play.version);
  const acc = typeof p.play.accuracy === "number" && p.play.accuracy > 0 ? `${accText(p.play.accuracy)}%` : null;
  const mods = modList(p.play.mods);
  const holdable = !p.practice && ["waiting_replay", "send"].includes(p.phase);
  const removable = holdable || ["held", "failed", "unbound"].includes(p.phase);
  const play = (what) => () => action(`plays/${encodeURIComponent(p.id)}/${what}`);

  const name = `${p.play.artist ?? ""} - ${chartName(p.play)}`;
  return el("li", { class: `play ${status.kind}` },
    cover(p),
    el("div", { class: "text" },
      p.url
        ? el("a", { class: "title", href: p.url, target: "_blank", rel: "noreferrer", title: `${name}\nOpen on Mania Tracker` }, p.play.title ?? "Unknown")
        : el("span", { class: "title", title: name }, p.play.title ?? "Unknown"),
      p.play.artist && el("span", { class: "artist" }, p.play.artist),
      el("div", { class: "facts" },
        diff && el("span", {}, diff),
        mods.length > 0 && el("span", { class: "mods" }, mods.map(modBadge)),
        acc && el("span", {}, acc)),
      (status.detail || removable) && el("span", { class: "detail" }, status.detail && el("span", {}, status.detail),
        holdable && el("button", { class: "link act", disabled: slow, onclick: play(p.onHold ? "send" : "hold") }, p.onHold ? "Send" : "Hold"),
        removable && el("button", { class: "link act", disabled: slow, onclick: play("remove") }, "Remove")),
      rating(p),
    ),
    el("div", { class: "side" },
      stamp(status, p.id),
      el("span", { class: "when" },
        p.analysis && el("button", { class: "look", title: "Analysis", "aria-label": "Analysis", onclick: () => openAnalysis(p) }, icon(CHART)),
        time(p.capturedAt)),
    ),
  );
}

// the site's dan labels are bare: "8+" is the 8+ dan, "gamma-" stays as it is
function danName(d) {
  const name = /^\d/.test(d.label) ? `${d.label} dan` : d.label;
  return d.ln ? `LN ${name}` : name;
}

// maps the site said it has no board for, this session
const boardless = new Set();

function rankButton(p) {
  if (!p.url || !p.md5 || (!p.rank && (p.noBoard || boardless.has(p.md5)))) return null;
  const r = p.rank;
  const open = () => openBoard({ md5: p.md5, artist: p.play.artist, title: p.play.title, version: p.play.version, keys: p.play.keys }, false, !!r, r ?? modVariant(modList(p.play.mods)));
  if (!r) return el("button", { class: "link act board-link", onclick: open }, "Leaderboard");
  return el("button", {
    class: `rank${r.best ? "" : " off"}`,
    title: r.best ? "Your spot on this map's leaderboard" : "Not a new best. Your best is here on this map's leaderboard",
    onclick: open,
  }, `#${r.position}`, r.total && el("span", {}, ` of ${r.total}`));
}

function rating(p) {
  const r = p.rating;
  const rank = rankButton(p);
  const msd = typeof r?.msd === "number";
  const dan = r?.dan && snapshot.showDans !== false;
  if (!rank && !msd && !dan) return null;
  return el("div", { class: "rating" },
    rank,
    msd && el("span", { class: "msd" }, el("b", {}, r.msd.toFixed(2)), " MSD"),
    dan && el("span", {
      class: `dan${r.dan.eligible ? "" : " off"}`,
      title: r.dan.eligible ? null : "This play doesn't count toward your dan",
    }, danName(r.dan)));
}

// a page of plays sized to the window, so a long session doesn't turn into a long scroll. a row with
// its rating, most rows have one
const ROW_PX = 106;
const PAGER_PX = 40;
let perPage = 5;
let page = 0;

function turnPage(by) {
  page += by;
  render();
}

function playList(plays) {
  const pages = Math.ceil(plays.length / perPage);
  page = Math.max(0, Math.min(page, pages - 1));
  const from = page * perPage;
  const shown = plays.slice(from, from + perPage);
  // kept for every play, not just this page, so flipping back doesn't fade the covers in again
  const ids = new Set(plays.map((p) => p.id));
  for (const cache of [covers, stamped, stampAt]) {
    for (const id of cache.keys()) if (!ids.has(id)) cache.delete(id);
  }
  return el("div", { class: "history" },
    el("ul", { class: "plays" }, shown.map(playRow)),
    pages > 1 && el("div", { class: "pager" },
      el("button", { class: "link", disabled: page === 0, onclick: () => turnPage(-1) }, "Previous"),
      el("span", {}, `${from + 1}–${from + shown.length} of ${plays.length}`),
      el("button", { class: "link", disabled: page === pages - 1, onclick: () => turnPage(1) }, "Next")));
}

function fitPage() {
  const shore = document.querySelector(".shore");
  const list = document.querySelector(".home .plays, .home .standings");
  if (!list || !shore) return;
  const scroller = document.documentElement.classList.contains("framed") ? app : document.scrollingElement;
  const room = innerHeight - (list.getBoundingClientRect().top + scroller.scrollTop) - PAGER_PX - shore.offsetHeight;
  if (list.classList.contains("standings")) {
    const mine = list.querySelector(".mine");
    const fit = Math.max(5, Math.floor((room - (mine ? mine.offsetHeight + parseFloat(getComputedStyle(mine).marginTop) : 0)) / STANDING_PX));
    if (fit === board.perPage) return;
    board.perPage = fit;
    return render();
  }
  const fit = Math.max(3, Math.floor(room / ROW_PX));
  if (fit === perPage) return;
  page = Math.floor(page * perPage / fit);
  perPage = fit;
  render();
}

// a map's leaderboard. follow: it's the map selected in osu!, and changes with it
const STANDING_PX = 44;
let board = null;
let boardTimer = 0;

// which of a map's boards: speed "ht", "dt" or none (nomod and DT together), plus Invert, Hold Off, No Release
const boardKey = (b) => `${b.md5}?speed=${b.speed ?? ""}&mods=${(b.mods ?? []).join(",")}`;

// same as the helper's for the selected map
function modVariant(mods) {
  const has = (...l) => mods.some((m) => l.includes(m));
  return { speed: has("HT", "DC") ? "ht" : has("DT", "NC") ? "dt" : null, mods: ["HO", "IN", "NR"].filter((m) => has(m)) };
}

// mine: open on the page with your own row. from: the first row shown, the wheel moves it a row at a time
function openBoard(map, follow = false, mine = false, variant = map) {
  goalsOpen = false;
  settingsOpen = notesOpen = false;
  analysis = maker = null;
  dropTrainer();
  clearTimeout(boardTimer);
  board = {
    map, follow, mine, md5: map.md5, speed: variant.speed ?? null, mods: variant.mods ?? [],
    data: null, error: null, from: 0, wheel: 0, perPage: board?.perPage ?? 8, seen: acceptedOn(map.md5),
  };
  loadBoard();
  render();
}

function closeBoard() {
  board = null;
  clearTimeout(boardTimer);
  render();
}

function acceptedOn(md5) {
  return snapshot.plays.filter((p) => p.md5 === md5 && p.receipt?.state === "accepted").length;
}

async function loadBoard() {
  const want = board;
  if (!want) return;
  // a following board changes in place, only the last ask counts
  const ask = want.asked = (want.asked ?? 0) + 1;
  const current = () => board === want && want.asked === ask;
  want.loading = true;
  try {
    const res = await fetch(`${BRIDGE}/leaderboard/${boardKey(want)}`);
    const body = await res.json().catch(() => ({}));
    if (!current()) return;
    if (res.ok) {
      want.data = body;
      want.error = null;
      if (body.unknown) boardless.add(want.md5);
    } else {
      want.error = body.error ?? "Couldn't load the leaderboard.";
    }
  } catch {
    if (current()) want.error = "Lost Hashi's helper.";
  }
  if (!current()) return;
  want.loading = false;
  render();
}

// called on every render, follows the selection and new plays on the map
function boardChanged() {
  if (!board) return;
  const selected = snapshot.tosu.map;
  if (board.follow && selected && boardKey(selected) !== boardKey(board)) {
    board.map = selected;
    board.md5 = selected.md5;
    board.speed = selected.speed ?? null;
    board.mods = selected.mods ?? [];
    board.data = board.error = null;
    board.from = 0;
    board.seen = acceptedOn(selected.md5);
    // whatever is on its way is for the map before
    board.asked = (board.asked ?? 0) + 1;
    clearTimeout(boardTimer);
    // scrolling through song select shouldn't ask for every map on the way
    boardTimer = setTimeout(loadBoard, 500);
    return;
  }
  const seen = acceptedOn(board.md5);
  if (seen !== board.seen) {
    board.seen = seen;
    loadBoard();
  }
}

const GRADES = { XH: ["SS", "silver"], X: ["SS", "ss"], SH: ["S", "silver"], S: ["S", "gold"], A: ["A", "a"], B: ["B", "b"], C: ["C", "c"], D: ["D", "d"], F: ["F", "d"] };

function ago(iso) {
  const s = (Date.now() - new Date(iso)) / 1000;
  if (!(s >= 0)) return "";
  for (const [n, unit] of [[31536000, "y"], [2592000, "mo"], [86400, "d"], [3600, "h"], [60, "m"]]) {
    if (s >= n) return `${Math.floor(s / n)}${unit} ago`;
  }
  return "now";
}

function standing(e, mine) {
  const [grade, tone] = GRADES[e.grade] ?? [e.grade, "d"];
  const name = e.username === snapshot.connection.username;
  return el("li", { class: `standing${name ? " me" : ""}${mine ? " mine" : ""}` },
    el("span", { class: "pos" }, `#${e.position ?? "?"}`),
    grade && el("span", { class: `grade ${tone}` }, grade),
    el("span", { class: "avatar" }, e.avatarUrl && el("img", { src: e.avatarUrl, alt: "", loading: "lazy" })),
    el("a", {
      class: "player",
      href: `${snapshot.connection.origin}/player/${encodeURIComponent(e.username)}`,
      target: "_blank",
      rel: "noreferrer",
      title: `Open ${e.username}'s profile on Mania Tracker`,
    }, e.username),
    e.app === "mania-bridge" && el("img", { class: "app", src: "images/app.svg", alt: "", title: "Sent with Hashi" }),
    el("span", { class: "mods" }, e.mods.filter((m) => m !== "CL").map((m) => modBadge(m, { rate: e.rate }))),
    el("span", { class: "acc" }, typeof e.accuracy === "number" ? `${accText(e.accuracy)}%` : ""),
    el("span", { class: "when", title: e.playedAt ? new Date(e.playedAt).toLocaleString() : null }, e.playedAt ? ago(e.playedAt) : ""));
}

function boardView() {
  const { map, data } = board;
  const title = data?.title ?? map.title ?? "Unknown";
  const diff = diffName(map.keys, data?.version ?? map.version);
  const players = data?.entries?.length ? `${data.total} ${data.total === 1 ? "player" : "players"}` : null;
  const head = el("div", { class: "head" },
    el("p", { class: "say" }, "Leaderboard"),
    el("button", { class: "link", onclick: closeBoard }, "Back to plays"));
  const about = el("div", { class: "map" },
    data?.url
      ? el("a", { class: "title", href: data.url, target: "_blank", rel: "noreferrer", title: "Open on Mania Tracker" }, title)
      : el("span", { class: "title" }, title),
    el("span", { class: "artist" }, data?.artist ?? map.artist),
    el("div", { class: "facts" },
      diff && el("span", {}, diff),
      (board.speed || board.mods.length > 0) && el("span", { class: "mods", title: "Only plays with these mods" },
        [board.speed && { ht: "HT", dt: "DT" }[board.speed], ...board.mods].filter(Boolean).map(modBadge)),
      players && el("span", { class: "players" }, players)),
    board.follow && el("span", { class: "line" }, "The map selected in osu!"));

  let body;
  if (board.error) {
    body = el("p", { class: "status bad" }, board.error, " ", el("button", { class: "link", onclick: () => { board.error = null; loadBoard(); render(); } }, "Try again"));
  } else if (!data) {
    body = el("p", { class: "doing" }, el("i", { class: "spinner", style: `animation-delay: -${Date.now() % SPIN_MS}ms` }), "Loading");
  } else if (data.unsupported) {
    body = el("p", { class: "lead" }, "Mania Tracker can't show leaderboards to Hashi yet.");
  } else if (data.unknown) {
    body = el("p", { class: "lead" }, "Mania Tracker has no leaderboard for this map.");
  } else if (!data.entries.length) {
    body = el("p", { class: "lead" }, "Nobody has played this map yet.");
  } else {
    const last = Math.max(0, data.entries.length - board.perPage);
    // kept until the list moves, the page size can still change once it's drawn
    const at = board.mine ? data.entries.findIndex((e) => e.username === snapshot.connection.username) : -1;
    if (at >= 0) board.from = Math.floor(at / board.perPage) * board.perPage;
    board.from = Math.max(0, Math.min(board.from, last));
    const from = board.from;
    const shown = data.entries.slice(from, from + board.perPage);
    const move = (by) => {
      const to = Math.max(0, Math.min(board.from + by, last));
      if (to === board.from) return;
      board.from = to;
      board.mine = false;
      render();
    };
    // a row per row's height of wheel, so a trackpad's small steps add up
    const wheel = (e) => {
      if (!e.deltaY) return;
      if (Math.sign(e.deltaY) !== Math.sign(board.wheel)) board.wheel = 0;
      board.wheel += e.deltaY;
      const rows = Math.trunc(board.wheel / STANDING_PX);
      board.wheel -= rows * STANDING_PX;
      // past either end the window scrolls as usual
      if (e.deltaY < 0 ? from > 0 : from < last) e.preventDefault();
      move(rows);
    };
    body = el("div", { class: "history" },
      el("ol", { class: "standings", onwheel: wheel },
        shown.map((e) => standing(e)),
        data.self && standing(data.self, true)),
      el("div", { class: "pager" },
        el("button", { class: "link", disabled: from === 0, onclick: () => move(-board.perPage) }, "Previous"),
        el("span", {}, `${from + 1}–${from + shown.length} of ${data.entries.length}`),
        el("button", { class: "link", disabled: from >= last, onclick: () => move(board.perPage) }, "Next")));
  }
  return el("div", { class: "board" }, head, about, body);
}

let analysis = null;

const GOAL_NAMES = { clean: "Real acc run", practice: "Practice goal" };
const GOAL_UNITS = { clean: "clean", practice: "at full speed" };

function goalTrack(g, width = 460) {
  const values = g.values ?? [g.start, g.previous, g.last].filter((v) => v != null);
  // a goal already beaten on its first try still needs some track to the left of the flag
  const lo = Math.min(g.target - 1, g.start ?? g.target - 5, ...values);
  const left = 10;
  const right = width - 30;
  const x = (v) => left + Math.max(0, Math.min(1, (v - lo) / (g.target - lo))) * (right - left);
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
  return `Best ${pct(g.best)}, last ${pct(g.last)}${change}, ${g.tries} ${g.tries === 1 ? "try" : "tries"}. ${accText(Math.max(0, g.target - g.best))}% to go.`;
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

const scroller = () => (document.documentElement.classList.contains("framed") ? app : document.scrollingElement);

function stored(key, fallback) {
  try { return JSON.parse(localStorage.getItem(key)) ?? fallback; } catch { return fallback; }
}

function store(key, value) {
  try { localStorage.setItem(key, JSON.stringify(value)); } catch {}
}

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
  scheduleTicks();
  requestAnimationFrame(followTicks);
  trainer.refresh?.();
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
    }, "Patterns mix in a key alone now and then, like a jumpjack"));
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
const TIPS_SHOWN = 4;
const SVG_NS = "http://www.w3.org/2000/svg";

function openAnalysis(p) {
  goalsOpen = false;
  settingsOpen = notesOpen = false;
  board = null;
  clearTimeout(boardTimer);
  analysis = { id: p.id, play: p, data: null, error: null, lane: "presses", allTips: false, openedAt: Date.now() };
  loadAnalysis();
  render();
}

function closeAnalysis() {
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

function heading(title, ...extra) {
  const node = el("div", { class: "part-head" }, el("h3", {}, title), extra);
  node.querySelector("h3").insertAdjacentHTML("beforeend", `<svg class="squiggle" viewBox="0 0 64 8" width="64" height="8" aria-hidden="true"><path d="M2 5q7-5 14 0t14 0t14 0t14 0"/></svg>`);
  return node;
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

function segs(options, current, pick, label) {
  return el("div", { class: "segs", title: label },
    options.map(([key, text]) => el("button", {
      class: `seg${current === key ? " on" : ""}`,
      "aria-pressed": String(current === key),
      disabled: slow || analysis?.loading,
      onclick: () => current !== key && pick(key),
    }, text)));
}

const KEY_GAP = 0.2;
const FINGER_CYCLE = ["index", "middle", "ring", "pinky", "thumb"];

async function pickFinger(d, column) {
  const next = FINGER_CYCLE[(FINGER_CYCLE.indexOf(d.columns[column].finger) + 1) % FINGER_CYCLE.length];
  await action(`finger/${d.keys}/${column}/${next}`);
  loadAnalysis();
}

const sameDay = (a, b) => a.toDateString() === b.toDateString();
const whenText = (iso) => (sameDay(new Date(iso), new Date()) ? `at ${time(iso)}` : `on ${new Date(iso).toLocaleDateString("en-US", { month: "short", day: "numeric" })}, ${time(iso)}`);
const deltaText = (x) => `${x >= 0 ? "+" : "−"}${Math.abs(x).toFixed(1)}`;

// early above the judgement line and late below, the way the notes were falling
function stageView(d, width, tip, laneH = 200) {
  const k = d.columns.length;
  const split = d.columns.findIndex((c) => c.hand === "right");
  const left = 42;
  const gap = k > 7 ? 4 : 6;
  const handGap = split > 0 ? 12 : 0;
  const laneW = (width - left - gap * (k - 1) - handGap) / k;
  const laneX = (i) => left + i * (laneW + gap) + (split > 0 && i >= split ? handGap : 0);
  const laneTop = 26;
  const line = laneTop + laneH / 2;
  const reach = d.windows[2];
  const y = (o) => line + (Math.max(-reach, Math.min(reach, o)) / reach) * (laneH / 2 - 4);
  const bins = 15;
  const binH = (laneH / 2 - 4) / bins;
  const piles = d.columns.map(() => new Array(bins * 2).fill(0));
  for (const [, o, j, c] of d.hits) {
    if (j > 5) continue;
    const b = Math.max(0, Math.min(bins * 2 - 1, Math.floor(((o + reach) / (2 * reach)) * bins * 2)));
    piles[c][b] += 1;
  }
  const most = Math.max(1, ...piles.flat());
  // by rank, not value, or one bad thumb leaves every other key the same color. KEY_GAP keeps
  // keys all at 99.9-100 from still going green to red
  const accs = d.columns.map((c) => c.acc).filter((a) => a != null);
  const heat = (a) => {
    if (a == null || accs.length < 2) return null;
    const t = accs.filter((x) => x - a >= KEY_GAP).length / (accs.length - 1);
    const stops = [[76, 195, 138], [242, 205, 92], [240, 136, 62], [229, 72, 77]];
    const at = t * (stops.length - 1);
    const i = Math.min(stops.length - 2, Math.floor(at));
    const mix = stops[i].map((v, j) => Math.round(v + (stops[i + 1][j] - v) * (at - i)));
    return `rgb(${mix.join(" ")})`;
  };
  const capTop = laneTop + laneH + 10;
  const capH = 24;
  const holdY = capTop + capH + 16;
  const braceTop = capTop + capH + (d.tapHold != null ? 26 : 10);
  const height = braceTop + 44;
  const root = svg("svg", { class: "chart stage", viewBox: `0 0 ${width} ${height}`, width: "100%", role: "img", "aria-label": "Each column's hits around the judgement line" });

  root.append(svg("text", { class: "side-label", x: 0, y: laneTop + 12 }, "early"));
  root.append(svg("text", { class: "side-label", x: 0, y: laneTop + laneH - 4 }, "late"));
  const lanes = d.columns.map((c, i) => {
    const x = laneX(i);
    const g = svg("g", { class: `lane${heat(c.acc) ? " hot" : ""}`, style: `--wobble: url(#ink${(i % 3) + 1}); --heat: ${heat(c.acc)}` });
    g.append(svg("rect", { class: "lane-bg", x, y: laneTop, width: laneW, height: laneH, rx: 7 }));
    for (const w of d.windows.slice(0, 2)) {
      for (const s of [-1, 1]) g.append(svg("line", { class: "guide", x1: x + 3, x2: x + laneW - 3, y1: y(s * w), y2: y(s * w) }));
    }
    const pile = svg("g", { class: "pile" });
    piles[i].forEach((n, b) => {
      if (!n) return;
      const center = -reach + ((b + 0.5) / (bins * 2)) * 2 * reach;
      const w = Math.max(4, (n / most) * (laneW - 8));
      pile.append(svg("rect", { class: `j${windowOf(center, d.windows)}`, x: x + (laneW - w) / 2, y: laneTop + 4 + b * binH + 0.6, width: w, height: Math.max(1.5, binH - 1.2), rx: Math.min(binH / 2, 3) }));
    });
    g.append(pile);
    if (c.hits) g.append(svg("rect", { class: "avg-note", x: x + laneW * 0.14, y: y(c.mean) - 3.5, width: laneW * 0.72, height: 7, rx: 3.5 }));
    const was = d.previous?.columns?.[i]?.acc;
    const change = c.acc != null && was != null ? c.acc - was : null;
    g.append(svg("text", { class: "lane-acc", x: x + laneW / 2, y: laneTop - 8, "text-anchor": "middle" },
      c.acc == null ? "–" : accText(c.acc, 1),
      change != null && laneW >= 56 && svg("tspan", { class: `delta ${change >= 0 ? "up" : "down"}`, dx: 4 }, deltaText(change))));
    g.append(svg("rect", { class: "cap-base", x: x + 1, y: capTop + 3, width: laneW - 2, height: capH, rx: 6 }));
    g.append(svg("rect", { class: "cap", x: x + 1, y: capTop, width: laneW - 2, height: capH, rx: 6 }));
    const name = laneW >= 40 ? c.finger : { thumb: "th", index: "ix", middle: "md", ring: "rg", pinky: "pk" }[c.finger];
    g.append(svg("text", { class: "cap-label", x: x + laneW / 2, y: capTop + 16, "text-anchor": "middle" }, name));
    if (c.tapHold != null) g.append(svg("text", { class: "hold-label", x: x + laneW / 2, y: holdY, "text-anchor": "middle" }, `${c.tapHold} ms`));
    const hit = svg("rect", { class: "hit", x, y: laneTop - 22, width: laneW, height: capTop - laneTop + 20 });
    const show = (e) => {
      g.classList.add("on");
      tip.show(e, pct(c.acc), `column ${i + 1}, ${c.hand} ${c.finger}`, `${signedMs(c.mean)} ms ±${msText(c.sd)}`, c.tapHold != null && `taps held ${c.tapHold} ms`, c.counts[5] ? `${c.counts[5]} ${c.counts[5] === 1 ? "miss" : "misses"}` : null, was != null && `${pct(was)} last time`);
    };
    const pick = svg("rect", { class: "hit pick", x, y: capTop - 2, width: laneW, height: capH + 6 });
    pick.addEventListener("pointerenter", (e) => {
      g.classList.add("on");
      tip.show(e, `${c.hand} ${c.finger}`, "Click to change which finger plays this key");
    });
    pick.addEventListener("pointerleave", () => {
      g.classList.remove("on");
      tip.hide();
    });
    pick.addEventListener("click", () => !slow && !analysis?.loading && pickFinger(d, i));
    hit.addEventListener("pointerenter", show);
    hit.addEventListener("pointermove", show);
    hit.addEventListener("pointerleave", () => {
      g.classList.remove("on");
      tip.hide();
    });
    g.append(hit, pick);
    return g;
  });
  root.append(...lanes);
  root.append(svg("rect", { class: "judgement", x: left - 4, y: line - 1.5, width: width - left + 8, height: 3, rx: 1.5 }));
  for (const [hand, from, to] of [["left", 0, split < 0 ? k : split], ["right", split < 0 ? k : split, k]]) {
    if (to <= from) continue;
    const x1 = laneX(from) + 2;
    const x2 = laneX(to - 1) + laneW - 2;
    const mid = (x1 + x2) / 2;
    root.append(svg("path", { class: "brace", d: `M${x1} ${braceTop}q0 6 6 6H${mid - 6}q6 0 6 6q0-6 6-6H${x2 - 6}q6 0 6-6` }));
    const h = d.hands[hand];
    const was = d.previous?.hands?.[hand]?.acc;
    root.append(svg("text", { class: "hand-label", x: mid, y: braceTop + 30, "text-anchor": "middle" },
      svg("tspan", { class: "strong" }, `${hand === "left" ? "Left" : "Right"} ${pct(h.acc)}`),
      h.acc != null && was != null && svg("tspan", { class: `delta ${h.acc >= was ? "up" : "down"}`, dx: 4 }, deltaText(h.acc - was)),
      svg("tspan", {}, `  ${signedMs(h.mean)} ms ±${msText(h.sd)}`)));
  }
  return root;
}

function histogram(values, windows, { width, height = 140, reach, label, tip, names, plain = false }) {
  const bins = Math.max(24, Math.min(64, Math.round(width / 8)));
  const step = (reach * 2) / bins;
  const counts = new Array(bins).fill(0);
  for (const v of values) counts[Math.floor((Math.max(-reach, Math.min(reach - 1e-9, v)) + reach) / step)] += 1;
  const top = Math.max(1, ...counts);
  const pad = { l: 4, r: 4, t: 28, b: 24 };
  const plotW = width - pad.l - pad.r;
  const plotH = height - pad.t - pad.b;
  const base = pad.t + plotH;
  const x = (v) => pad.l + ((v + reach) / (reach * 2)) * plotW;
  const barW = Math.max(2, plotW / bins - 2.5);
  const root = svg("svg", { class: "chart histogram", viewBox: `0 0 ${width} ${height}`, width: "100%", role: "img", "aria-label": label });
  const bars = svg("g", { class: "bars" });
  const hits = svg("g");
  counts.forEach((c, i) => {
    const center = -reach + (i + 0.5) * step;
    const h = c ? Math.max(3, (c / top) * plotH) : 0;
    const bx = pad.l + i * (plotW / bins) + 1.25;
    const bar = svg("rect", { class: plain ? "plain" : `j${windowOf(center, windows)}`, x: bx, y: base - h, width: barW, height: h + 3, rx: Math.min(3, barW / 2), style: `--i: ${Math.abs(i - bins / 2)}` });
    bars.append(bar);
    const hit = svg("rect", { class: "hit", x: bx - 1, y: pad.t - 10, width: barW + 2.5, height: plotH + 10 });
    const from = -reach + i * step;
    hit.addEventListener("pointerenter", (e) => {
      bar.classList.add("on");
      tip.show(e, `${c} ${c === 1 ? "hit" : "hits"}`, `${signedMs(from)} to ${signedMs(from + step)} ms`, !plain && names[windowOf(center, windows) - 1]);
    });
    hit.addEventListener("pointerleave", () => {
      bar.classList.remove("on");
      tip.hide();
    });
    hits.append(hit);
  });
  root.append(bars, svg("rect", { class: "baseline", x: pad.l - 2, y: base, width: plotW + 4, height: 2.5, rx: 1.25 }));
  let lastLabel = -Infinity;
  for (const v of [-reach, -windows[1], -windows[0], 0, windows[0], windows[1], reach]) {
    const px = x(v);
    if (Math.abs(v) < reach) root.append(svg("path", { class: "tick", d: `M${px} ${base + 3}v4` }));
    if (px - lastLabel < 28) continue;
    lastLabel = px;
    root.append(svg("text", { class: "tick-label", x: Math.max(pad.l + 8, Math.min(width - pad.r - 8, px)), y: height - 4, "text-anchor": "middle" }, v === 0 ? "0" : `${v > 0 ? "" : "−"}${Math.round(Math.abs(v))}`));
  }
  root.append(svg("text", { class: "edge-label", x: pad.l, y: 10 }, "early"), svg("text", { class: "edge-label", x: width - pad.r, y: 10, "text-anchor": "end" }, "late"));
  if (values.length) {
    const m = values.reduce((a, b) => a + b, 0) / values.length;
    const mx = x(Math.max(-reach, Math.min(reach, m)));
    const right = mx < width - 90;
    const flag = svg("g", { class: "pennant" });
    flag.append(svg("path", { class: "pole", d: `M${mx} ${base}V${pad.t - 20}` }));
    flag.append(svg("path", { class: "cloth", d: right ? `M${mx} ${pad.t - 20}l14 4.5-14 4.5z` : `M${mx} ${pad.t - 20}l-14 4.5 14 4.5z` }));
    flag.append(svg("text", { class: "flag-label", x: mx + (right ? 18 : -18), y: pad.t - 12, "text-anchor": right ? "start" : "end" }, `avg ${signedMs(m)}`));
    root.append(flag);
  }
  root.append(hits);
  return root;
}

function cssColors() {
  const style = getComputedStyle(document.documentElement);
  const get = (n) => style.getPropertyValue(n).trim();
  return { j: [1, 2, 3, 4, 5, 6].map((i) => get(`--j${i}`)), rule: get("--osu-b3"), muted: get("--osu-f1"), ink: get("--ink"), accent: get("--accent") };
}

function scatter(d, tip) {
  const releases = analysis.lane === "releases";
  const points = releases ? d.releases.map(([t, o, c]) => [t, o, d.mode === "stable" ? 0 : windowOf(o / 1.5, d.windows), c]) : d.hits;
  const reach = releases ? d.windows[3] * 1.5 : d.windows[3];
  const wrap = el("div", { class: "scatter" });
  const canvas = el("canvas", { height: 180, role: "img", "aria-label": releases ? "Long note releases over the play" : "Presses over the play" });
  const ring = el("i", { class: "ring", hidden: "" });
  wrap.append(canvas, ring);
  const pad = { l: 36, r: 10, t: 12, b: 22 };
  let layout = null;
  wrap.draw = () => {
    const w = canvas.clientWidth;
    if (!w) return;
    const h = canvas.clientHeight || 180;
    const dpr = devicePixelRatio || 1;
    canvas.width = w * dpr;
    canvas.height = h * dpr;
    const ctx = canvas.getContext("2d");
    ctx.scale(dpr, dpr);
    const c = cssColors();
    const plotW = w - pad.l - pad.r;
    const plotH = h - pad.t - pad.b;
    const x = (t) => pad.l + (t / Math.max(1, d.length)) * plotW;
    const y = (o) => pad.t + plotH / 2 + (Math.max(-reach, Math.min(reach, o)) / reach) * (plotH / 2);
    layout = { x, y };
    ctx.font = `10px Outfit, sans-serif`;
    ctx.lineWidth = 1;
    ctx.fillStyle = c.muted;
    ctx.textAlign = "right";
    for (const wv of d.windows.slice(0, 2)) {
      const edge = releases ? wv * 1.5 : wv;
      for (const s of [-1, 1]) {
        ctx.strokeStyle = c.rule;
        ctx.beginPath();
        ctx.moveTo(pad.l, Math.round(y(s * edge)) + 0.5);
        ctx.lineTo(w - pad.r, Math.round(y(s * edge)) + 0.5);
        ctx.stroke();
      }
    }
    if (d.practice) {
      const [a, b] = [x(d.practice.from), x(d.practice.to)];
      ctx.fillStyle = c.accent;
      ctx.globalAlpha = 0.14;
      ctx.fillRect(a, pad.t, Math.max(2, b - a), plotH);
      ctx.globalAlpha = 1;
      ctx.fillStyle = c.accent;
      const fits = a + 4 + ctx.measureText("practice").width <= w - pad.r;
      ctx.textAlign = fits ? "left" : "right";
      ctx.fillText("practice", fits ? a + 4 : b - 4, pad.t + 10);
      ctx.fillStyle = c.muted;
      ctx.textAlign = "right";
    }
    ctx.fillText("early", pad.l - 8, pad.t + 8);
    ctx.fillText("late", pad.l - 8, pad.t + plotH);
    if (!releases) {
      ctx.strokeStyle = c.j[5];
      ctx.globalAlpha = 0.5;
      for (const [t] of d.misses) {
        const mx = Math.round(x(t)) + 0.5;
        ctx.beginPath();
        ctx.moveTo(mx, pad.t);
        ctx.lineTo(mx, pad.t + plotH);
        ctx.stroke();
      }
      ctx.globalAlpha = 1;
    }
    const r = points.length > 3000 ? 1.4 : points.length > 1000 ? 1.8 : 2.4;
    ctx.globalAlpha = 0.9;
    for (const [t, o, j] of points) {
      ctx.fillStyle = j ? c.j[Math.min(6, j) - 1] : c.muted;
      ctx.beginPath();
      ctx.arc(x(t), y(o), r, 0, Math.PI * 2);
      ctx.fill();
    }
    ctx.globalAlpha = 1;
    ctx.strokeStyle = c.ink;
    ctx.lineWidth = 2;
    ctx.lineCap = "round";
    ctx.beginPath();
    ctx.moveTo(pad.l - 4, y(0));
    ctx.lineTo(w - pad.r + 4, y(0));
    ctx.stroke();
    ctx.fillStyle = c.muted;
    ctx.textAlign = "left";
    ctx.fillText(clock(d.offset), pad.l, h - 5);
    ctx.textAlign = "center";
    ctx.fillText(clock(d.offset + d.length / 2), pad.l + plotW / 2, h - 5);
    ctx.textAlign = "right";
    ctx.fillText(clock(d.offset + d.length), w - pad.r, h - 5);
  };
  // the nearest point to the pointer, not the one under it
  canvas.addEventListener("pointermove", (e) => {
    if (!layout) return;
    const box = canvas.getBoundingClientRect();
    const px = e.clientX - box.left;
    const py = e.clientY - box.top;
    let best = null;
    let bestD = 24 * 24;
    for (const p of points) {
      const dx = layout.x(p[0]) - px;
      const dy = layout.y(p[1]) - py;
      if (dx * dx + dy * dy < bestD) {
        bestD = dx * dx + dy * dy;
        best = p;
      }
    }
    if (!best) {
      ring.hidden = true;
      return tip.hide();
    }
    ring.hidden = false;
    ring.style.left = `${layout.x(best[0])}px`;
    ring.style.top = `${layout.y(best[1])}px`;
    const f = d.columns[best[3]];
    tip.show(e, `${signedMs(best[1])} ms`, clock(d.offset + best[0]), `column ${best[3] + 1}, ${f.hand} ${f.finger}`, releases ? "release" : d.names[best[2] - 1]);
  });
  canvas.addEventListener("pointerleave", () => {
    ring.hidden = true;
    tip.hide();
  });
  return wrap;
}

// monotone cubic: smooth, but never overshoots the points, so it stays under 100%
function curve(pts) {
  const n = pts.length;
  const f = (v) => v.toFixed(1);
  if (n < 3) return pts.map(([x, y], i) => `${i ? "L" : "M"}${f(x)} ${f(y)}`).join("");
  const slope = pts.slice(1).map(([x, y], i) => (y - pts[i][1]) / (x - pts[i][0]));
  const m = pts.map((_, i) => {
    if (i === 0) return slope[0];
    if (i === n - 1) return slope[n - 2];
    const [a, b] = [slope[i - 1], slope[i]];
    return a * b <= 0 ? 0 : (3 * a * b) / (2 * Math.max(a, b) + Math.min(a, b));
  });
  let d = `M${f(pts[0][0])} ${f(pts[0][1])}`;
  for (let i = 1; i < n; i++) {
    const [[x0, y0], [x1, y1]] = [pts[i - 1], pts[i]];
    const h = (x1 - x0) / 3;
    d += `C${f(x0 + h)} ${f(y0 + m[i - 1] * h)} ${f(x1 - h)} ${f(y1 - m[i] * h)} ${f(x1)} ${f(y1)}`;
  }
  return d;
}

function riverView(d, width, tip) {
  const sections = d.sections;
  const accs = sections.map((s) => s.acc).filter((a) => a != null);
  if (accs.length < 3) return null;
  const floor = Math.max(0, Math.floor(Math.min(...accs) - 1));
  const pad = { l: 36, r: 6 };
  const plotW = width - pad.l - pad.r;
  const step = plotW / sections.length;
  const cx = (i) => pad.l + (i + 0.5) * step;
  const accTop = 10;
  const accBottom = 84;
  const ay = (a) => accTop + (1 - (a - floor) / (100 - floor)) * (accBottom - accTop);
  const surface = 104;
  const bed = 152;
  const height = 176;
  const root = svg("svg", { class: "chart stream", viewBox: `0 0 ${width} ${height}`, width: "100%", role: "img", "aria-label": "Accuracy, density and misses over the play" });
  if (d.practice) {
    const tx = (t) => pad.l + (t / Math.max(1, d.length)) * plotW;
    const [a, b] = [tx(d.practice.from), tx(d.practice.to)];
    root.append(svg("rect", { class: "practice-band", x: a, y: accTop - 6, width: Math.max(2, b - a), height: bed - accTop + 6, rx: 4 }));
    // about the label's width: near the end of the song it goes inside the band's right edge
    const fits = a + 50 <= width - pad.r;
    root.append(svg("text", { class: "practice-label", x: fits ? a + 4 : b - 4, y: accTop + 4, "text-anchor": fits ? "start" : "end" }, "practice"));
  }
  for (const v of [floor, 100]) {
    root.append(svg("path", { class: "grid", d: `M${pad.l} ${ay(v)}H${width - pad.r}` }));
    root.append(svg("text", { class: "tick-label", x: pad.l - 6, y: ay(v) + 3, "text-anchor": "end" }, `${v}%`));
  }
  const top = Math.max(1, ...sections.map((s) => s.nps));
  const wy = (n) => bed - (n / top) * (bed - surface);
  const pts = sections.map((s, i) => [cx(i), wy(s.nps)]);
  const water = `M${pad.l} ${pts[0][1]}L${curve(pts).slice(1)}L${width - pad.r} ${pts[pts.length - 1][1]}`;
  root.append(svg("path", { class: "flow", d: `${water}V${bed}H${pad.l}Z` }));
  root.append(svg("path", { class: "flow-edge", d: water }));
  root.append(svg("text", { class: "tick-label", x: pad.l - 6, y: surface + 4, "text-anchor": "end" }, `${Math.round(top)}/s`));
  root.append(svg("path", { class: "bed-line", d: `M${pad.l - 6} ${bed + 2}q12-4 24 0${"t24 0".repeat(Math.ceil(plotW / 24))}` }));
  const sunk = svg("g", { class: "sunk-notes" });
  d.misses.forEach(([t], i) => {
    const mx = pad.l + (t / Math.max(1, d.length)) * plotW;
    const turn = ((i * 47) % 70) - 35;
    sunk.append(svg("rect", { x: mx - 5, y: bed - 6 - ((i * 13) % 3) * 3, width: 10, height: 5, rx: 2.5, transform: `rotate(${turn} ${mx} ${bed - 4})` }));
  });
  root.append(sunk);
  root.append(svg("path", { class: "acc-line", d: curve(sections.map((s, i) => s.acc != null && [cx(i), ay(s.acc)]).filter(Boolean)) }));
  root.append(svg("text", { class: "tick-label", x: pad.l, y: height - 2 }, clock(d.offset)));
  root.append(svg("text", { class: "tick-label", x: width - pad.r, y: height - 2, "text-anchor": "end" }, clock(d.offset + d.length)));
  const cross = svg("path", { class: "cross", hidden: "" });
  const dot = svg("circle", { class: "dot", r: 4.5, hidden: "" });
  root.append(cross, dot);
  const hit = svg("rect", { class: "hit", x: pad.l, y: 0, width: plotW, height: bed + 6 });
  hit.addEventListener("pointermove", (e) => {
    const box = root.getBoundingClientRect();
    const px = ((e.clientX - box.left) / box.width) * width;
    const i = Math.max(0, Math.min(sections.length - 1, Math.floor((px - pad.l) / step)));
    const s = sections[i];
    cross.setAttribute("d", `M${cx(i)} ${accTop - 4}V${bed}`);
    cross.removeAttribute("hidden");
    if (s.acc != null) {
      dot.setAttribute("cx", cx(i));
      dot.setAttribute("cy", ay(s.acc));
      dot.removeAttribute("hidden");
    } else dot.setAttribute("hidden", "");
    tip.show(e, s.acc != null ? pct(s.acc) : "No notes", `${clock(d.offset + s.at)}–${clock(d.offset + s.at + d.sectionLength)}`, `${s.nps} notes a second`, s.misses ? `${s.misses} ${s.misses === 1 ? "miss" : "misses"}` : null);
  });
  hit.addEventListener("pointerleave", () => {
    cross.setAttribute("hidden", "");
    dot.setAttribute("hidden", "");
    tip.hide();
  });
  root.append(hit);
  return root;
}

const PATTERN_GLYPHS = {
  Singles: `<rect x="8" y="9" width="14" height="6" rx="3"/>`,
  Chords: `<rect x="1" y="9" width="13" height="6" rx="3"/><rect x="16" y="9" width="13" height="6" rx="3"/>`,
  Jacks: `<rect x="8" y="3" width="14" height="6" rx="3"/><rect x="8" y="15" width="14" height="6" rx="3"/>`,
  "Long notes": `<rect x="8" y="1" width="14" height="22" rx="3"/>`,
};

function patternsView(d) {
  return el("div", { class: "patterns" },
    el("div", { class: "prow head" }, el("span"), el("span"), el("span", {}, "Notes"), el("span", {}, "Accuracy"), el("span", {}, "Timing")),
    d.patterns.map((p, i) => {
      const glyph = el("span", { class: "pglyph" });
      glyph.innerHTML = `<svg viewBox="0 0 30 24" width="30" height="24" aria-hidden="true">${PATTERN_GLYPHS[p.name] ?? ""}</svg>`;
      return el("div", { class: "prow", style: `--wobble: url(#ink${(i % 3) + 1})` },
        glyph,
        el("b", {}, p.name),
        el("span", {}, p.notes),
        el("span", {}, pct(p.acc)),
        el("span", {}, p.hits ? `${signedMs(p.mean)} ms` : "–"));
    }));
}

function part(area, ...children) {
  return el("section", { class: `part ${area}` }, children);
}

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
    el("button", { class: "link", onclick: closeAnalysis }, "Back to plays"));
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

const PRESENCE_NOTES = {
  off: "Shown on your profile while osu! is open.",
  unsupported: "Mania Tracker doesn't support this yet.",
  needs_reconnect: "Mania Tracker needs your OK first.",
  sharing: "Shown on your profile while osu! is open.",
  paused: "Shown on your profile once osu! is open.",
};

function tosuAutostartNote(on, launch) {
  if (launch === "with-osu") return "tosu opens with osu! here.";
  if (snapshot.getTosu && !snapshot.getTosu.step) {
    return ["Couldn't find tosu on this computer. ", el("button", { class: "link", disabled: slow, onclick: () => action("get-tosu") }, "Get tosu")];
  }
  if (!on) return null;
  if (launch === "missing") return "Couldn't find tosu on this computer.";
  if (launch === "failed") return "tosu didn't start.";
  return null;
}

// cached so the page opens in the right color before the helper answers
const DEFAULT_THEME = { hue: 333, sat: 100 };
const THEME_KEY = "mania-bridge-theme";
const PRESET_HUES = [
  [333, "Pink"], [355, "Rose"], [15, "Red"], [30, "Orange"], [50, "Amber"], [95, "Lime"],
  [140, "Green"], [170, "Teal"], [200, "Cyan"], [225, "Blue"], [260, "Indigo"], [290, "Purple"],
];
const RAINBOW = "linear-gradient(to right, hsl(0,95%,55%), hsl(30,95%,55%), hsl(60,95%,50%), hsl(120,70%,45%), hsl(170,75%,45%), hsl(200,85%,50%), hsl(240,90%,60%), hsl(280,80%,55%), hsl(320,90%,55%), hsl(360,95%,55%))";

// don't re-render under a held slider, snapshots wait for the release
let themeDraft = null;
let renderWaiting = false;

function applyTheme({ hue, sat }) {
  document.documentElement.style.setProperty("--hue", hue);
  document.documentElement.style.setProperty("--sat", sat / 100);
  try { localStorage.setItem(THEME_KEY, JSON.stringify({ hue, sat })); } catch {}
}

function isDefaultTheme({ hue, sat }) {
  return hue === DEFAULT_THEME.hue && sat === DEFAULT_THEME.sat;
}

function pickTheme(theme) {
  if (busy) return;
  // so the next redraw doesn't flash the old value
  snapshot.theme = theme;
  applyTheme(theme);
  action(`theme/${theme.hue}/${theme.sat}`);
}

function themeStrip(label, key, max, gradient) {
  const theme = snapshot.theme;
  const send = (e) => {
    themeDraft = null;
    renderWaiting = false;
    pickTheme({ ...theme, [key]: Number(e.target.value) });
  };
  // no change event if it didn't move. setTimeout so a change event goes first
  const release = () => setTimeout(() => {
    if (!themeDraft) return;
    themeDraft = null;
    if (renderWaiting) {
      renderWaiting = false;
      render();
    }
  });
  return el("input", {
    type: "range", class: "strip", min: 0, max, value: theme[key], "aria-label": label, disabled: slow,
    style: `--track: ${gradient}`,
    onpointerdown: () => (themeDraft = { ...theme }),
    oninput: (e) => {
      themeDraft = { ...(themeDraft ?? theme), [key]: Number(e.target.value) };
      applyTheme(themeDraft);
    },
    onchange: send,
    onpointerup: release,
    onpointercancel: release,
  });
}

function themeSetting() {
  const theme = snapshot.theme ?? DEFAULT_THEME;
  return el("div", { class: "setting theme" },
    el("div", { class: "text" },
      el("span", { class: "title" }, "Theme color"),
      el("div", { class: "swatches" }, PRESET_HUES.map(([hue, name]) => el("button", {
        class: `swatch${hue === theme.hue ? " on" : ""}`, title: name, "aria-label": name,
        "aria-pressed": String(hue === theme.hue), disabled: slow,
        style: `background: hsl(${hue}, ${theme.sat}%, 65%)`,
        onclick: () => pickTheme({ ...theme, hue }),
      }))),
      themeStrip("Theme hue", "hue", 359, RAINBOW),
      themeStrip("Theme saturation", "sat", 100,
        `linear-gradient(to right, hsl(${theme.hue},0%,2%), hsl(${theme.hue},3%,8%) 22%, hsl(${theme.hue},18%,18%) 48%, hsl(${theme.hue},55%,56%) 76%, hsl(${theme.hue},100%,70%))`)),
    el("button", { class: "link reset", disabled: slow || isDefaultTheme(theme), onclick: () => pickTheme(DEFAULT_THEME) }, "Reset"));
}

function getUpdate() {
  action(snapshot.update.install ? "update/install" : "update/download");
}

function update() {
  const u = snapshot.update;
  if (!u || (u.dismissed && !u.step)) return null;
  const version = el("b", {}, u.version);
  if (u.step === "downloading" || u.step === "installing") {
    return el("div", { class: "pinned" },
      el("p", {}, u.step === "installing"
        ? ["Installing ", version, ", Hashi opens again in a moment."]
        : ["Downloading ", version, u.percent != null ? ` · ${u.percent}%` : "", "…"]),
      el("i", { class: "spinner", style: `animation-delay: -${Date.now() % SPIN_MS}ms` }));
  }
  if (u.step === "failed") {
    return el("div", { class: "pinned failed" },
      el("p", {}, "Couldn't update to ", version, ". ", el("span", { class: "why" }, u.error)),
      el("button", { class: "key go", disabled: slow, onclick: () => action("update/install") }, "Try again"),
      el("button", { class: "link", disabled: slow, onclick: () => action("update/download") }, "Download"));
  }
  return el("div", { class: "pinned" },
    el("p", {}, "Hashi ", version, " is out."),
    el("button", { class: "key go", disabled: slow, onclick: getUpdate }, u.install ? "Update" : "Download"),
    el("button", { class: "link", disabled: slow, onclick: () => action("update/later") }, "Later"));
}

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

let settingsOpen = false;

function toggle(title, on, path, note, cls = "") {
  return el("div", { class: `setting${cls}` },
    el("div", { class: "text" },
      el("span", { class: "title" }, title),
      note && el("span", { class: "line" }, note)),
    el("button", {
      class: `switch${on ? " on" : ""}`,
      role: "switch",
      "aria-checked": String(on),
      "aria-label": title,
      disabled: slow,
      onclick: () => action(`${path}/${on ? "off" : "on"}`),
    }, el("i")));
}

// plays, live status or both: never neither, then the app would do nothing
let lockedHint = null;

function sharing() {
  const p = snapshot.presence;
  const plays = snapshot.sendPlays;
  const live = p.enabled;
  const liveAvailable = p.status !== "unsupported";
  const liveNote = p.status === "error" ? p.message : PRESENCE_NOTES[p.status];
  const hint = "Keep one of them on, or nothing is sent.";
  const row = (key, title, on, locked, can, next, line, cls) => el("div", { class: `setting${lockedHint === key ? " is-error" : cls}` },
    el("div", { class: "text" },
      el("span", { class: "title" }, title),
      lockedHint === key ? el("span", { class: "line" }, hint) : line && el("span", { class: "line" }, line)),
    el("button", {
      class: `switch${on ? " on" : ""}${locked ? " locked" : ""}`,
      role: "switch",
      "aria-checked": String(on),
      "aria-label": title,
      disabled: slow || (!on && !can),
      onclick: () => {
        if (locked) {
          lockedHint = key;
          return render();
        }
        action(`sharing/${next}`);
      },
    }, el("i")));
  return [
    row("plays", "Send my plays", plays, plays && !(live && liveAvailable), true, plays ? "live" : live ? "both" : "plays",
      !plays && "Off. The maps you finish aren't sent.", ""),
    row("live", "Share my live status", live, live && !plays, liveAvailable, live ? "plays" : plays ? "both" : "live",
      live ? liveNote : p.status === "unsupported" && liveNote, live ? ` is-${p.status}` : ""),
  ];
}

function settings(c, host, reconnect) {
  const t = snapshot.tosuAutostart;
  const { launch } = snapshot.tosu;
  const tosuNote = tosuAutostartNote(t, launch);
  const computer = [
    t != null && toggle("Open tosu with Hashi", t, "tosu-autostart", tosuNote,
      t && ["missing", "failed"].includes(launch) ? " is-error" : ""),
    t && snapshot.tosuDashboard != null && toggle("Open its dashboard too", snapshot.tosuDashboard, "tosu-dashboard",
      "In your browser, each time tosu starts.", " sub"),
    snapshot.autostart != null && toggle("Open Hashi on startup", snapshot.autostart, "autostart"),
    snapshot.startInTray != null && toggle("Start in the tray", snapshot.startInTray, "start-in-tray"),
    snapshot.onTop != null && toggle("Always on top", snapshot.onTop, "on-top", "The window stays above other windows."),
  ].filter(Boolean);
  return el("div", { class: "settings" },
    el("div", { class: "head" },
      el("p", { class: "say" }, "Settings"),
      el("button", { class: "link", onclick: toggleSettings }, "Back to plays")),
    unsaved && el("p", { class: "status bad" }, unsaved),
    el("p", { class: "group" }, "Mania Tracker"),
    sharing(),
    el("div", { class: "setting" },
      el("div", { class: "text" },
        el("span", { class: "title" }, "Connected as ", c.username),
        host && el("span", { class: "line" }, host)),
      reconnect
        ? el("button", { class: "key go", disabled: slow, onclick: () => action("connect") }, "Connect again")
        : el("button", { class: "key danger", disabled: slow, onclick: () => action("disconnect") }, "Disconnect")),
    computer.length > 0 && [el("p", { class: "group" }, "This computer"), ...computer],
    el("p", { class: "group" }, "Look"),
    snapshot.showDans != null && toggle("Show dans", snapshot.showDans, "show-dans", "The dan next to a play's MSD."),
    snapshot.thumb && el("div", { class: "setting" },
      el("div", { class: "text" },
        el("span", { class: "title" }, "Thumb on the middle key"),
        el("span", { class: "line" }, "Which hand it counts for in an analysis of 5K, 7K or 9K.")),
      el("div", { class: "segs" }, ["left", "right"].map((side) => el("button", {
        class: `seg${snapshot.thumb === side ? " on" : ""}`,
        "aria-pressed": String(snapshot.thumb === side),
        disabled: slow,
        onclick: () => snapshot.thumb !== side && action(`thumb/${side}`),
      }, side === "left" ? "Left" : "Right")))),
    snapshot.theme && themeSetting(),
    el("p", { class: "about" }, `Hashi ${snapshot.version}`,
      shownNotes().length > 0 && [" · ", el("button", { class: "link", onclick: openNotes }, "What's new")],
      snapshot.update && !snapshot.update.step
        && [" · ", el("button", { class: "link", disabled: slow, onclick: getUpdate }, `${snapshot.update.install ? "Update to" : "Get"} ${snapshot.update.version}`)],
));
}

function toggleSettings() {
  settingsOpen = !settingsOpen;
  if (settingsOpen) {
    goalsOpen = false;
    notesOpen = false;
    analysis = maker = null;
    dropTrainer();
    board = null;
    clearTimeout(boardTimer);
  }
  lockedHint = null;
  render();
}

function statusLine(c) {
  switch (c.status) {
    case "offline": return el("p", { class: "status warn" }, c.message ?? "Mania Tracker is not reachable.");
    case "not_in_beta": return el("p", { class: "status warn" }, c.message ?? "This account is not in the beta.");
    case "revoked":
    case "error": return el("p", { class: "status bad" }, c.message ?? "This connection no longer works.");
    case "disabled": return el("p", { class: "status warn" }, c.message);
    default: return null;
  }
}

function now() {
  const { tosu } = snapshot;
  const line = (state, name, ...rest) => el("div", { class: `now ${state}` }, el("i"), el("b", {}, name), rest);
  if (!tosu.connected) return line(tosu.launch === "with-osu" ? "off" : "missing", "tosu", tosuDetail(tosu));
  if (tosu.blocked) return line("missing", "osu!", "tosu can't read it");
  if (!tosu.osu) return line("off", "osu!", "Not open");
  return line("on", gameName(tosu), el("span", {}, osuLabel(tosu), " as ", el("b", {}, tosu.player || "Guest")));
}

function waiting() {
  const node = el("div", { class: "blank" });
  node.innerHTML = `<svg width="100%" height="52" aria-hidden="true"><g class="still">
    <rect class="dashed" x="1" y="1" width="72" height="50" rx="8"/>
    <path class="thick" d="M88 12h96"/>
    <path d="M88 26h58M88 41h26M124 41h18M152 41h30"/>
    <svg x="100%" overflow="visible">
      <rect class="dashed" x="-84" y="13" width="78" height="24" rx="6" transform="rotate(-5 -45 25)"/>
    </svg>
  </g></svg>`;
  return node;
}

// reuse the same <img>, a new one is blank until it decodes
let avatar = null;

function face(c) {
  if (!c.avatarUrl) return c.username[0].toUpperCase();
  if (avatar?.getAttribute("src") !== c.avatarUrl) avatar = el("img", { src: c.avatarUrl, alt: "" });
  return avatar;
}

function home(c, host) {
  const reconnect = c.status === "revoked" || c.status === "error";
  return el("div", { class: "home" },
    el("div", { class: "me" },
      el("span", { class: "face" }, face(c)),
      el("div", { class: "who" },
        el("div", { class: "name" },
          el("a", {
            class: "profile",
            href: `${c.origin}/player/${encodeURIComponent(c.username)}`,
            target: "_blank",
            rel: "noreferrer",
            title: "Open your profile on Mania Tracker",
            style: c.accent && `color: ${c.accent}`,
          }, c.username),
          host && el("span", {}, host)),
        now()),
      snapshot.tosu.map && el("button", {
        class: `gear${board?.follow ? " on" : ""}`,
        title: "Leaderboard of the map selected in osu!",
        "aria-pressed": String(!!board?.follow),
        onclick: () => (board?.follow ? closeBoard() : openBoard(snapshot.tosu.map, true)),
      }, icon(PODIUM)),
      (snapshot.goals ?? []).length > 0 && el("button", {
        class: `gear${goalsOpen ? " on" : ""}${goalsUnseen() ? " dot" : ""}`,
        title: "Goals",
        "aria-pressed": String(goalsOpen),
        onclick: toggleGoals,
      }, icon(TARGET)),
      el("button", { class: `gear${settingsOpen ? " on" : ""}`, title: "Settings", "aria-pressed": String(settingsOpen), onclick: toggleSettings }, icon(GEAR))),
    update(),
    tosuHint(),
    statusLine(c),
    reconnect && !settingsOpen
      && el("button", { class: "key go", disabled: slow, onclick: () => action("connect") }, "Connect again"),
    helperDown && el("p", { class: "status warn" }, "Lost Hashi's helper. Reconnecting."),
    settingsOpen
      ? settings(c, host, reconnect)
      : notesOpen
        ? notesView()
      : goalsOpen
        ? goalsView()
      : board
        ? boardView()
      : maker
        ? makerView()
      : trainer
        ? trainerView()
      : analysis
        ? analysisView()
      : snapshot.plays.length
        ? playList(snapshot.plays)
        : el("div", { class: "nothing" },
            el("p", { class: "say" }, snapshot.sendPlays ? "No plays yet." : "Sending plays is off."),
            el("p", { class: "lead" }, snapshot.sendPlays
              ? "Finish a mania map and it shows up here."
              : "The maps you finish stay off your profile. Turn it back on in the settings."),
            waiting(), el("i"), el("i"), el("i")));
}

const GEAR = `<path d="M4.10 0.39L5.90 0.39L6.07 1.88L7.16 2.51L8.55 1.92L9.44 3.47L8.24 4.37L8.24 5.63L9.44 6.53L8.55 8.08L7.16 7.49L6.07 8.12L5.90 9.61L4.10 9.61L3.93 8.12L2.84 7.49L1.45 8.08L0.56 6.53L1.76 5.63L1.76 4.37L0.56 3.47L1.45 1.92L2.84 2.51L3.93 1.88z" stroke-linejoin="round"/><circle cx="5" cy="5" r="1.4"/>`;
const PODIUM = `<path d="M3.5 9.5v-5h3v5M.5 9.5v-3h3M6.5 9.5v-4h3v4M0 9.5h10M5 1l.5 1h1l-.8.7.3 1-1-.6-1 .6.3-1-.8-.7h1z" stroke-linejoin="round"/>`;
const TARGET = `<circle cx="5" cy="5" r="4.5"/><circle cx="5" cy="5" r="2.5"/><circle cx="5" cy="5" r=".6" fill="currentColor"/>`;
const TRAY = `<path d="M1 1l6 6M7 3v4H3M.5 9.5h9"/>`;
const CHART = `<path d="M1 9.5h8M2.5 7.5v-2M5 7.5V1.5M7.5 7.5v-4" stroke-linecap="round"/>`;
const PIN = `<path class="head" d="M3 .5h4V4l1.5 1.5h-7L3 4z" stroke-linejoin="round"/><path d="M5 5.5v4"/>`;

// a different view opens at its top, not wherever the last one was scrolled to
let shownView = null;

function render() {
  if (themeDraft) {
    renderWaiting = true;
    return;
  }
  app.replaceChildren();
  if (helperDown && !snapshot) {
    app.append(el("div", { class: "connect full" }, el("p", { class: "note" }, "Hashi's helper is not running.")));
    return;
  }
  if (!snapshot) return;
  if (pin) {
    pin.classList.toggle("on", !!snapshot.onTop);
    pin.setAttribute("aria-pressed", String(!!snapshot.onTop));
  }

  const c = snapshot.connection;
  const host = c.origin === OFFICIAL_ORIGIN ? null : c.origin.replace(/^https?:\/\//, "");
  if (snapshot.theme) applyTheme(snapshot.theme);
  const ownTheme = snapshot.theme && !isDefaultTheme(snapshot.theme);
  document.documentElement.style.setProperty("--accent", (!ownTheme && c.accent) || "var(--osu-pink)");
  const hasAccount = c.username && !["disconnected", "connecting"].includes(c.status);
  if (c.status === "connected" && hasAccount && wasConnecting && !STILL.matches) {
    crossAt = Date.now();
    if (document.hidden) document.addEventListener("visibilitychange", () => crossAt && (cross(), render()), { once: true });
    else cross();
  }
  if (!hasAccount) crossAt = 0;
  wasConnecting = c.status === "connecting";
  const atHome = hasAccount && !crossAt;
  if (!atHome) {
    settingsOpen = notesOpen = false;
    analysis = maker = null;
    dropTrainer();
    board = null;
    clearTimeout(boardTimer);
  }
  boardChanged();
  analysisChanged();

  if (!snapshot.titlebar && location.origin === BRIDGE) {
    app.append(el("div", { class: "corner" },
      snapshot.tray && el("button", { title: "Hide to tray", onclick: () => windowAction("hide") }, icon(TRAY)),
      el("button", { class: "close", title: "Quit", onclick: () => windowAction("close") }, icon(`<path d="m.5.5 9 9m0-9-9 9"/>`))));
  }

  if (atHome) {
    app.append(el("div", { class: "ground" }, home(c, host), shore(settingsOpen, bottleVersion())));
    const view = settingsOpen ? "settings" : notesOpen ? "notes" : goalsOpen ? "goals" : board ? "board" : maker ? "maker" : trainer ? "trainer" : analysis ? `analysis ${analysis.id}` : "plays";
    document.documentElement.classList.toggle("roomy", !!analysis && ROOMY.matches);
    if (view !== shownView) (document.documentElement.classList.contains("framed") ? app : document.scrollingElement).scrollTop = 0;
    shownView = view;
    fitPage();
    return;
  }
  shownView = null;
  document.documentElement.classList.remove("roomy");

  const connecting = c.status === "connecting";
  // it comes from the site's discovery document
  const again = /^https?:\/\//.test(c.authorizeUrl ?? "") ? c.authorizeUrl : null;
  const full = !snapshot.plays.length;
  const river = crossAt ? [
    el("p", { class: "say" }, "Connected as ", c.username, "."),
    el("p", { class: "lead" }, "Every mania play you finish goes to your profile from here on."),
  ] : [
    el("p", { class: "say" }, connecting ? "Halfway there." : "Not connected yet."),
    el("p", { class: "lead" }, connecting
      ? "Finish connecting on the page that opened in your browser."
      : "Nothing you play gets across. Connect and every mania play you finish goes to your profile on its own."),
    !snapshot.tosu.connected && !["starting", "with-osu"].includes(snapshot.tosu.launch)
      && el("p", { class: "field-hint" }, "Plays are only sent while tosu is open."),
    blockedHint(),
    el("div", { class: "cross" },
      connecting
        ? el("button", { class: "primary waiting", disabled: true }, "Waiting for the browser")
        : el("button", { class: "primary", disabled: slow, onclick: () => action("connect") }, "Connect to Mania Tracker"),
      host && el("span", { class: "host" }, host)),
    connecting
      ? el("p", { class: "note" },
          again && [el("a", { href: again, target: "_blank", rel: "noreferrer" }, "Open the page again"), " · "],
          el("button", { class: "link", onclick: () => action("cancel") }, "Cancel"))
      : c.message && el("p", { class: "note" }, c.message),
    update(),
    getTosu(),
  ];
  app.append(el("div", { class: `connect${full ? " full" : ""}` },
    intro(crossAt ? "flat" : connecting ? "down" : "up"),
    el("div", { class: "river" }, river, full && riverbed())));
  wakeInk();

  if (helperDown) app.append(el("p", { class: "status warn" }, "Lost Hashi's helper. Reconnecting."));
  if (snapshot.plays.length) app.append(playList(snapshot.plays));
}

function windowAction(name) {
  fetch(`${BRIDGE}/window/${name}`, { method: "POST" }).catch(() => {});
}

function icon(paths) {
  const node = el("span", { class: "icon" });
  node.innerHTML = `<svg viewBox="0 0 10 10" width="10" height="10" fill="none" stroke="currentColor" stroke-width="1">${paths}</svg>`;
  return node;
}

// windows: no system frame, so draw a title bar
let titlebar = null;
let pin = null;
function showTitlebar() {
  if (titlebar || !snapshot?.titlebar || location.origin !== BRIDGE) return;
  pin = snapshot.onTop != null && el("button", { class: "pin", title: "Always on top", onclick: () => action(`on-top/${snapshot.onTop ? "off" : "on"}`) }, icon(PIN));
  titlebar = el("header", {
    class: "titlebar",
    onmousedown: (e) => e.button === 0 && !e.target.closest("button") && windowAction("drag"),
  },
    el("img", { src: "images/app.svg", alt: "", width: 16, height: 16 }),
    el("span", { class: "name" }, "Hashi"),
    pin,
    el("button", { title: "Hide to tray", onclick: () => windowAction("hide") }, icon(TRAY)),
    el("button", { title: "Minimize", onclick: () => windowAction("minimize") }, icon(`<path d="M0 5.5h10"/>`)),
    el("button", { class: "close", title: "Close", onclick: () => windowAction("close") }, icon(`<path d="m.5.5 9 9m0-9-9 9"/>`)),
  );
  document.body.prepend(titlebar);
  document.documentElement.classList.add("framed");
  addEventListener("blur", () => document.documentElement.classList.add("inactive"));
  addEventListener("focus", () => document.documentElement.classList.remove("inactive"));
}

function listen() {
  const events = new EventSource(`${BRIDGE}/events`);
  events.onmessage = (ev) => {
    snapshot = JSON.parse(ev.data);
    helperDown = false;
    showTitlebar();
    render();
  };
  events.addEventListener("reload", () => location.reload());
  events.onerror = () => {
    helperDown = true;
    render();
  };
}

// animated ink wobble. ink1-3 are static variants so neighbouring rows differ
const INK = 2.6;
const INK_MS = 700;

function inkFilter() {
  document.body.insertAdjacentHTML("beforeend", `<svg class="defs" width="0" height="0" aria-hidden="true">
    <filter id="ink" x="-5%" y="-5%" width="110%" height="110%" color-interpolation-filters="sRGB">
      <feTurbulence type="fractalNoise" baseFrequency="0.04" numOctaves="2" seed="1" result="from"/>
      <feTurbulence type="fractalNoise" baseFrequency="0.04" numOctaves="2" seed="2" result="to"/>
      <feComposite in="from" in2="to" operator="arithmetic" k1="0" k2="1" k3="0" k4="0" result="noise"/>
      <feDisplacementMap in="SourceGraphic" in2="noise" scale="${INK}" xChannelSelector="R" yChannelSelector="G"/>
    </filter>${[1, 2, 3].map((n) => `
    <filter id="ink${n}" x="-5%" y="-5%" width="110%" height="110%" color-interpolation-filters="sRGB">
      <feTurbulence type="fractalNoise" baseFrequency="0.04" numOctaves="2" seed="${n * 13}" result="noise"/>
      <feDisplacementMap in="SourceGraphic" in2="noise" scale="${INK}" xChannelSelector="R" yChannelSelector="G"/>
    </filter>`).join("")}</svg>`);
  const [from, to] = document.querySelectorAll("#ink feTurbulence");
  const mix = document.querySelector("#ink feComposite");
  const push = document.querySelector("#ink feDisplacementMap");
  let seed = 2;
  let start = 0;
  let last = 0;
  let running = false;
  const frame = (now) => {
    // 30fps
    if (now - last >= 30) {
      if (STILL.matches || !document.querySelector(".scene, .opening")) {
        running = false;
        return;
      }
      last = now;
      let t = (now - start) / INK_MS;
      if (t >= 1) {
        from.setAttribute("seed", seed);
        to.setAttribute("seed", (seed = seed % 999 + 1));
        start = now;
        t = 0;
      }
      mix.setAttribute("k2", 1 - t);
      mix.setAttribute("k3", t);
      // averaging two patterns flattens it, compensate
      push.setAttribute("scale", INK / Math.hypot(1 - t, t));
    }
    requestAnimationFrame(frame);
  };
  wakeInk = () => {
    if (running) return;
    running = true;
    requestAnimationFrame(frame);
  };
  STILL.addEventListener("change", wakeInk);
}

// the loop stops while nothing on screen is inked
let wakeInk = () => {};

// ?done: where the browser lands after the consent page
function donePage(ok) {
  document.documentElement.classList.add("done");
  crossAt = Date.now();
  crossNoteMs = 4200;
  reach = Math.ceil(Math.max(screen.width, innerWidth) / 200) * 100;
  app.append(el("div", { class: "connect full" },
    bridge({
      osu: { state: "ok", name: "osu!" },
      tosu: { state: "ok", name: "tosu" },
      site: ok
        ? { state: "ok", name: "Mania Tracker", detail: "Connected" }
        : { state: "idle", name: "Mania Tracker", detail: "Not connected" },
    }, ok ? "flat" : "up"),
    el("div", { class: "river" },
      el("p", { class: "say" }, ok ? "Connected." : "Nothing was connected."),
      el("p", { class: "lead" }, "You can close this tab."),
      riverbed())));
  wakeInk();
}

let resized = 0;
addEventListener("resize", () => {
  clearTimeout(resized);
  resized = setTimeout(() => document.querySelector(".shore") && render(), 200);
});

// overlay scrollbar, the page still scrolls natively
function scrollbar() {
  const bar = el("div", { class: "scrollbar", hidden: "" }, el("i"));
  const thumb = bar.firstChild;
  document.body.append(bar);
  const page = () => document.documentElement.classList.contains("framed") ? app : document.scrollingElement;
  const MIN = 40;
  const update = () => {
    const p = page();
    const over = p.scrollHeight - p.clientHeight;
    bar.hidden = over <= 1;
    if (bar.hidden) return;
    const track = bar.clientHeight;
    const size = Math.max(MIN, track * p.clientHeight / p.scrollHeight);
    thumb.style.height = `${size}px`;
    thumb.style.top = `${(track - size) * p.scrollTop / over}px`;
  };
  bar.addEventListener("pointerdown", (e) => {
    e.preventDefault();
    const p = page();
    const track = bar.clientHeight;
    const size = thumb.offsetHeight;
    const per = (p.scrollHeight - p.clientHeight) / Math.max(1, track - size);
    if (e.target !== thumb) p.scrollTop = (e.clientY - bar.getBoundingClientRect().top - size / 2) * per;
    const from = e.clientY;
    const start = p.scrollTop;
    bar.setPointerCapture(e.pointerId);
    bar.classList.add("dragging");
    const move = (m) => { p.scrollTop = start + (m.clientY - from) * per; };
    const up = () => {
      bar.classList.remove("dragging");
      bar.removeEventListener("pointermove", move);
    };
    bar.addEventListener("pointermove", move);
    bar.addEventListener("pointerup", up, { once: true });
    bar.addEventListener("pointercancel", up, { once: true });
  });
  let queued = false;
  const soon = () => {
    if (queued) return;
    queued = true;
    requestAnimationFrame(() => { queued = false; update(); });
  };
  document.addEventListener("scroll", soon, true);
  addEventListener("resize", soon);
  new MutationObserver(soon).observe(app, { childList: true, subtree: true });
  new ResizeObserver(soon).observe(app);
}

const OPENING_MS = 1300;

async function openingScreen() {
  document.documentElement.classList.add("alone");
  if (!STILL.matches) {
    const node = el("div", { class: "opening" });
    node.innerHTML = `<svg viewBox="0 0 160 160" width="160" height="160" aria-hidden="true"><g class="ink">
      <clipPath id="opening-disc"><circle cx="80" cy="80" r="70"/></clipPath>
      <circle class="disc" cx="80" cy="80" r="70"/>
      <g clip-path="url(#opening-disc)">
        <path class="arrow" d="M50 39h29V23l32 31-32 31V69H50z"/>
        <path class="line spark" d="M-14-4l-7-8M0-9v-11M14-4l7-8"/>
        <path class="deck" d="M-10 103H19Q80 74 141 103H170V147H111A31 33 0 0 0 49 147H-10z"/>
        <path class="water" d="M-60 131q10-3 20 0${"t20 0".repeat(13)}V170H-60z" style="${loop(4000)}"/>
      </g>
      <circle class="rim" cx="80" cy="80" r="70"/>
    </g></svg>`;
    document.body.append(node);
    wakeInk();
    await new Promise((resolve) => setTimeout(resolve, OPENING_MS));
    await node.animate({ opacity: [1, 0] }, { duration: 240, easing: "steps(4)", fill: "forwards" }).finished;
  }
  windowAction("opened");
}

const done = new URLSearchParams(location.search).get("done");
try { applyTheme(JSON.parse(localStorage.getItem(THEME_KEY)) ?? DEFAULT_THEME); } catch {}
inkFilter();
scrollbar();
if (done) donePage(done === "yes");
else if (new URLSearchParams(location.search).has("opening")) openingScreen();
else {
  listen();
  render();
  loadNotes();
}
