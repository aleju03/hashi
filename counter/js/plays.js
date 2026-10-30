// The list of plays: each row, its status, mods, cover and rating, and the pages.

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
        snapshot.analysis && p.analysis && el("button", { class: "look", title: "Analysis", "aria-label": "Analysis", onclick: () => openAnalysis(p) }, icon(CHART)),
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
