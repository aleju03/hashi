// The page itself: the header, which view shows, the title bar, the connect screen and starting up.

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
          snapshot.analysis
            ? el("button", {
              class: "profile",
              title: "Your profile",
              "aria-pressed": String(!!profilePage),
              style: c.accent && `color: ${c.accent}`,
              onclick: () => (profilePage ? closeProfile() : openProfile()),
            }, c.username)
            : el("span", { class: "profile", style: c.accent && `color: ${c.accent}` }, c.username),
          host && el("span", {}, host)),
        now()),
      snapshot.tosu.map && el("button", {
        class: `gear${board?.follow ? " on" : ""}`,
        title: "Leaderboard of the map selected in osu!",
        "aria-pressed": String(!!board?.follow),
        onclick: () => (board?.follow ? closeBoard() : openBoard(snapshot.tosu.map, true)),
      }, icon(PODIUM)),
      snapshot.analysis && (snapshot.goals ?? []).length > 0 && el("button", {
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
      : profilePage
        ? profileView()
      : skinPage
        ? skinView()
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
              : snapshot.analysis
                ? "The maps you finish stay off your profile, but still show up here with their analysis."
                : "The maps you finish stay off your profile. Turn it back on in the settings."),
            waiting(), el("i"), el("i"), el("i")));
}

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
    skinPage = profilePage = null;
    board = null;
    clearTimeout(boardTimer);
  }
  boardChanged();
  analysisChanged();

  if (!snapshot.titlebar && location.origin === BRIDGE) {
    app.append(el("div", { class: "corner" },
      snapshot.tray && el("button", { title: "Hide to tray", onclick: () => windowAction("hide") }, icon(TRAY)),
      el("button", { title: "Maximize", onclick: () => windowAction("maximize") }, icon(MAXIMIZE)),
      el("button", { class: "close", title: "Quit", onclick: () => windowAction("close") }, icon(`<path d="m.5.5 9 9m0-9-9 9"/>`))));
  }

  if (atHome) {
    app.append(el("div", { class: "ground" }, home(c, host), shore(settingsOpen, bottleVersion())));
    const view = settingsOpen ? "settings" : notesOpen ? "notes" : goalsOpen ? "goals" : profilePage ? "profile" : skinPage ? "skin" : board ? "board" : maker ? "maker" : trainer ? "trainer" : analysis ? `analysis ${analysis.id}` : "plays";
    document.documentElement.classList.toggle("roomy", !!(analysis || skinPage) && ROOMY.matches);
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

// windows: no system frame, so draw a title bar
let titlebar = null;
let pin = null;
function showTitlebar() {
  if (titlebar || !snapshot?.titlebar || location.origin !== BRIDGE) return;
  pin = snapshot.onTop != null && el("button", { class: "pin", title: "Always on top", onclick: () => action(`on-top/${snapshot.onTop ? "off" : "on"}`) }, icon(PIN));
  titlebar = el("header", {
    class: "titlebar",
    onmousedown: (e) => e.button === 0 && e.detail === 1 && !e.target.closest("button") && windowAction("drag"),
    ondblclick: (e) => !e.target.closest("button") && windowAction("maximize"),
  },
    el("img", { src: "images/app.svg", alt: "", width: 16, height: 16 }),
    el("span", { class: "name" }, "Hashi"),
    pin,
    el("button", { title: "Hide to tray", onclick: () => windowAction("hide") }, icon(TRAY)),
    el("button", { title: "Minimize", onclick: () => windowAction("minimize") }, icon(`<path d="M0 5.5h10"/>`)),
    el("button", { title: "Maximize", onclick: () => windowAction("maximize") }, icon(MAXIMIZE)),
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
