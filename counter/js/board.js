// A map's leaderboard.

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
  skinPage = profilePage = null;
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
