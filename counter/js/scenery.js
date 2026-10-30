// The drawn world around the app: the bridge, the river and shore, the ink wobble, the scrollbar and the opening screen.

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
