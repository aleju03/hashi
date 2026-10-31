// A map's leaderboard.

// a map's leaderboard. follow: it's the map selected in osu!, and changes with it
const STANDING_PX = 44;
let board = null;
let boardTimer = 0;

// which of a map's boards: speed "ht", "dt" or none (nomod and DT together), plus Invert, Hold Off, No Release
const boardKey = (b) => `${b.md5}?speed=${b.speed ?? ""}&mods=${(b.mods ?? []).join(",")}`;
// whose plays, and which 50 of them
const BOARD_PAGE = 50;
const boardFilters = (b) => `&offset=${b.offset}&country=${b.country ?? ""}&friends=${b.friends ? 1 : 0}${b.q ? `&q=${encodeURIComponent(b.q)}` : ""}`;
const countryNames = (() => {
  try { return new Intl.DisplayNames(["en"], { type: "region" }); } catch { return null; }
})();
const countryName = (code) => countryNames?.of(code) ?? code;
// osu!'s own flags, the ones Mania Tracker shows. `slot` keeps one image per place it's drawn in
const flag = (code, slot) => rowImg(`flag ${slot}`, {
  class: "flag", src: `https://osu.ppy.sh/images/flags/${code}.png`, alt: "", onerror: (ev) => ev.target.remove(),
});

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
  // the country or friends picked last time stay picked
  board = {
    map, follow, mine, md5: map.md5, speed: variant.speed ?? null, mods: variant.mods ?? [],
    data: null, error: null, from: 0, wheel: 0, perPage: board?.perPage ?? 8, seen: acceptedOn(map.md5),
    offset: 0, country: board?.country ?? null, friends: board?.friends ?? false, q: "",
    countries: board?.countries ?? null, selfCountry: board?.selfCountry ?? null,
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
    const res = await fetch(`${BRIDGE}/leaderboard/${boardKey(want)}${boardFilters(want)}`);
    const body = await res.json().catch(() => ({}));
    if (!current()) return;
    if (res.ok) {
      want.data = body;
      want.error = null;
      if (body.unknown) boardless.add(want.md5);
      if (body.countries) want.countries = body.countries;
      if (body.selfCountry) want.selfCountry = body.selfCountry;
      if (body.url) want.siteUrl = body.url;
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
    board.from = board.offset = 0;
    board.countries = null;
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

// reused across redraws: a new <img> is blank until it decodes, a flash on every click. One per
// player, players without an avatar of their own share a picture and a node can only be in one row
const rowImages = new Map();
const ROW_IMAGES_KEPT = 600;

function rowImg(key, attrs) {
  let img = rowImages.get(key);
  if (!img || img.getAttribute("src") !== attrs.src) {
    img = el("img", attrs);
    if (rowImages.size >= ROW_IMAGES_KEPT) rowImages.delete(rowImages.keys().next().value);
    rowImages.set(key, img);
  }
  return img;
}

function standing(e, mine) {
  const [grade, tone] = GRADES[e.grade] ?? [e.grade, "d"];
  const name = e.username === snapshot.connection.username;
  return el("li", { class: `standing${name ? " me" : ""}${mine ? " mine" : ""}` },
    el("span", { class: "pos" }, `#${e.position ?? "?"}`),
    grade && el("span", { class: `grade ${tone}` }, grade),
    el("span", { class: "avatar" }, e.avatarUrl && rowImg(`avatar ${e.userId ?? e.username}`, { src: e.avatarUrl, alt: "", loading: "lazy" })),
    el("a", {
      class: "player",
      href: `${snapshot.connection.origin}/player/${encodeURIComponent(e.username)}`,
      target: "_blank",
      rel: "noreferrer",
      title: `Open ${e.username}'s profile on Mania Tracker`,
    }, e.username),
    e.app === "mania-bridge" && rowImg(`app ${e.userId ?? e.username}`, { class: "app", src: "images/app.svg", alt: "", title: "Sent with Hashi" }),
    el("span", { class: "mods" }, e.mods.filter((m) => m !== "CL").map((m) => modBadge(m, { rate: e.rate }))),
    el("span", { class: "acc" }, typeof e.accuracy === "number" ? `${accText(e.accuracy)}%` : ""),
    el("span", { class: "when", title: e.playedAt ? new Date(e.playedAt).toLocaleString() : null }, e.playedAt ? ago(e.playedAt) : ""));
}

// a new set of players starts over from the top
function boardFilter(change) {
  change(board);
  board.offset = board.from = 0;
  board.mine = false;
  loadBoard();
  render();
}

// kept across redraws, which take it out of the page and its focus with it, so it's given back
function boardSearch() {
  const keep = (ev) => (board.caret = [ev.target.selectionStart, ev.target.selectionEnd]);
  const input = (board.searchField ??= el("input", {
    class: "board-search", type: "search", spellcheck: "false", maxlength: 32, placeholder: "Search players", "aria-label": "Search players",
    onfocus: () => (board.typing = true),
    // a redraw takes it out and puts it back focused, only a focus that stays gone is the player leaving
    onblur: () => setTimeout(() => document.activeElement !== board?.searchField && board && (board.typing = false), 0),
    onkeyup: keep,
    onclick: keep,
    oninput: (ev) => {
      keep(ev);
      const q = ev.target.value.trim();
      clearTimeout(board.searchTimer);
      board.searchTimer = setTimeout(() => q !== board.q && boardFilter((b) => (b.q = q)), 250);
    },
  }));
  if (board.typing) {
    requestAnimationFrame(() => {
      if (!input.isConnected || document.activeElement === input) return;
      input.focus();
      if (board.caret) input.setSelectionRange(...board.caret);
    });
  }
  return input;
}

const CARET = `<path d="M2.5 4l2.5 2.5 2.5-2.5" stroke-linecap="round" stroke-linejoin="round"/>`;

// the list is kept while it's open, so a redraw doesn't scroll it back to the top
function boardCountries(others, picked) {
  const key = `${others.map((c) => c.code).join()} ${picked}`;
  if (board.countryList?.key === key) return board.countryList.node;
  const node = el("ul", { class: "board-countries", role: "listbox", "aria-label": "Country" },
    others.map((c) => el("li", {},
      el("button", {
        class: c.code === picked ? "on" : "",
        role: "option",
        "aria-selected": String(c.code === picked),
        onclick: () => boardFilter((b) => {
          b.countriesOpen = false;
          b.friends = false;
          b.country = c.code;
        }),
      }, el("span", {}, el("img", { class: "flag", src: `https://osu.ppy.sh/images/flags/${c.code}.png`, alt: "", onerror: (ev) => ev.target.remove() }), countryName(c.code)), el("b", {}, c.count)))));
  board.countryList = { key, node };
  return node;
}

const closeCountries = () => {
  board.countriesOpen = false;
  render();
};
document.addEventListener("pointerdown", (e) => board?.countriesOpen && !e.target.closest(".board-pick") && closeCountries());
document.addEventListener("keydown", (e) => e.key === "Escape" && board?.countriesOpen && closeCountries());

const SEARCH = `<circle cx="4.2" cy="4.2" r="3.4"/><path d="M6.7 6.7l2.8 2.8" stroke-linecap="round"/>`;

// one row: whose plays, or the name search in its place while it's open
function boardScope() {
  if (board.searching || board.q) {
    return el("div", { class: "board-filters" },
      boardSearch(),
      el("button", {
        class: "link",
        onclick: () => {
          board.searching = board.typing = false;
          board.searchField.value = "";
          clearTimeout(board.searchTimer);
          if (board.q) boardFilter((b) => (b.q = ""));
          else render();
        },
      }, "Done"));
  }
  const own = board.selfCountry;
  const scope = board.friends ? "friends" : board.country ?? "all";
  const others = (board.countries ?? []).filter((c) => c.code !== own);
  const picked = board.country && board.country !== own ? board.country : "";
  return el("div", { class: "board-filters" },
    segs([["all", "Global"], ...(own ? [[own, [flag(own, "own"), countryName(own)]]] : []), ["friends", "Friends"]], scope,
      (s) => boardFilter((b) => {
        b.friends = s === "friends";
        b.country = s === "all" || s === "friends" ? null : s;
      }), "Whose plays"),
    (others.length > 0 || picked) && el("div", { class: "board-pick" },
      el("div", { class: "segs" },
        el("button", {
          class: `seg${picked ? " on" : ""}`,
          "aria-expanded": String(!!board.countriesOpen),
          onclick: () => {
            board.countriesOpen = !board.countriesOpen;
            render();
          },
        }, picked ? [flag(picked, "picked"), countryName(picked)] : "Countries", icon(CARET))),
      board.countriesOpen && boardCountries(others, picked)),
    el("div", { class: "segs" },
      el("button", {
        class: "seg", title: "Search players", "aria-label": "Search players",
        onclick: () => {
          board.searching = board.typing = true;
          render();
        },
      }, icon(SEARCH))));
}

function boardView() {
  const { map, data } = board;
  const title = data?.title ?? map.title ?? "Unknown";
  const diff = diffName(map.keys, data?.version ?? map.version);
  const players = data?.entries?.length ? `${data.total} ${data.total === 1 ? "player" : "players"}` : null;
  const head = el("div", { class: "head" },
    el("p", { class: "say" }, "Leaderboard", board.follow && el("span", { class: "following", title: "It changes with the map selected in osu!" }, "Selected in osu!")),
    el("button", { class: "link", onclick: closeBoard }, "Back to plays"));
  const about = el("div", { class: "map" },
    el("div", { class: "name" },
      data?.url
        ? el("a", { class: "title", href: data.url, target: "_blank", rel: "noreferrer", title: "Open on Mania Tracker" }, title)
        : el("span", { class: "title" }, title),
      el("span", { class: "artist" }, data?.artist ?? map.artist)),
    el("div", { class: "facts" },
      diff && el("span", {}, diff),
      (board.speed || board.mods.length > 0) && el("span", { class: "mods", title: "Only plays with these mods" },
        [board.speed && { ht: "HT", dt: "DT" }[board.speed], ...board.mods].filter(Boolean).map(modBadge)),
      players && el("span", { class: "players" }, players)));
  const filtered = board.friends || board.country || board.q;
  const scope = data && !data.unsupported && !data.unknown && boardScope();

  let body;
  if (board.error) {
    body = el("p", { class: "status bad" }, board.error, " ", el("button", { class: "link", onclick: () => { board.error = null; loadBoard(); render(); } }, "Try again"));
  } else if (!data) {
    body = el("p", { class: "doing" }, el("i", { class: "spinner", style: `animation-delay: -${Date.now() % SPIN_MS}ms` }), "Loading");
  } else if (data.unsupported) {
    body = el("p", { class: "lead" }, "Mania Tracker can't show leaderboards to Hashi yet.");
  } else if (data.unknown) {
    body = el("p", { class: "lead" }, "Mania Tracker has no leaderboard for this map.");
  } else if (data.friendsNotLoaded) {
    body = el("p", { class: "lead" }, "Load your osu! friends on Mania Tracker first, from a map's leaderboard there.",
      board.siteUrl && [" ", el("a", { class: "link", href: board.siteUrl, target: "_blank", rel: "noreferrer" }, "Open this map there")]);
  } else if (!data.entries.length) {
    body = el("p", { class: "lead" }, filtered ? "No tracked plays match." : "Nobody has played this map yet.");
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
    // past the 50 loaded, the next or previous 50
    const more = data.offset + data.entries.length < data.total;
    const page = (dir) => {
      if (dir > 0 && from >= last && more) board.offset += BOARD_PAGE;
      else if (dir < 0 && from === 0 && data.offset > 0) board.offset -= BOARD_PAGE;
      else return move(dir * board.perPage);
      board.from = dir > 0 ? 0 : Number.MAX_SAFE_INTEGER;
      board.mine = false;
      loadBoard();
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
        el("button", { class: "link", disabled: board.loading || (from === 0 && data.offset === 0), onclick: () => page(-1) }, "Previous"),
        el("span", {}, `${data.offset + from + 1}–${data.offset + from + shown.length} of ${data.total}`),
        el("button", { class: "link", disabled: board.loading || (from >= last && !more), onclick: () => page(1) }, "Next")));
  }
  // the filters sit beside the map when there's room, under it when there isn't
  return el("div", { class: "board" }, head, el("div", { class: "board-top" }, about, scope), body);
}
