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

function gameName(reader) {
  return reader.client === "lazer" ? "osu!lazer" : "osu!";
}

function osuLabel(reader) {
  return OSU_STATES[reader.state] ?? String(reader.state ?? "").replace(/([a-z])([A-Z])/g, "$1 $2").replace(/^./, (c) => c.toUpperCase());
}

function blockedHint() {
  const { blocked } = snapshot.reader;
  return blocked && el("div", { class: "blocked" },
    el("p", {}, "osu! can't be read until you run this once:"),
    el("code", { title: "Click to copy", onclick: (e) => {
      navigator.clipboard?.writeText(blocked.fix);
      e.currentTarget.classList.add("copied");
    } }, blocked.fix));
}

function readHint() {
  return snapshot.reader.blocked ? blockedHint() : null;
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
  const { reader } = snapshot;
  const line = (state, name, ...rest) => el("div", { class: `now ${state}` }, el("i"), el("b", {}, name), rest);
  if (!reader.connected || reader.blocked) return line("missing", "osu!", "Can't be read");
  if (!reader.osu) return line("off", "osu!", "Not open");
  return line("on", gameName(reader), el("span", {}, osuLabel(reader), reader.player && [" as ", el("b", {}, reader.player)]));
}

// reuse the same <img>, a new one is blank until it decodes
let avatar = null;

// the account's name, or the osu! one when Hashi isn't connected
function playerName(c) {
  return c.username ?? snapshot.player ?? "Player";
}

function face(c) {
  if (!c.avatarUrl) return playerName(c)[0].toUpperCase();
  if (avatar?.getAttribute("src") !== c.avatarUrl) avatar = el("img", { src: c.avatarUrl, alt: "" });
  return avatar;
}

function home(c, host) {
  const reconnect = c.status === "revoked" || c.status === "error";
  const account = !!c.username;
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
            }, playerName(c))
            : el("span", { class: "profile", style: c.accent && `color: ${c.accent}` }, playerName(c)),
          account
            ? host && el("span", {}, host)
            : el("button", { class: "link", disabled: slow, onclick: () => action("connect") }, "Connect to Mania Tracker")),
        now()),
      snapshot.reader.map && el("button", {
        class: `gear${cutter ? " on" : ""}`,
        title: "Cut the map selected in osu!",
        "aria-pressed": String(!!cutter),
        onclick: () => (cutter ? closeCutter() : openCutter()),
      }, icon(SCISSORS)),
      snapshot.reader.map && snapshot.reader.client !== "lazer" && el("button", {
        class: `gear${rater ? " on" : ""}`,
        title: "Change the rate of the map selected in osu!",
        "aria-pressed": String(!!rater),
        onclick: () => (rater ? closeRater() : openRater()),
      }, icon(GAUGE)),
      account && snapshot.reader.map && el("button", {
        class: `gear${board?.follow ? " on" : ""}`,
        title: "Leaderboard of the map selected in osu!",
        "aria-pressed": String(!!board?.follow),
        onclick: () => (board?.follow ? closeBoard() : openBoard(snapshot.reader.map, true)),
      }, icon(PODIUM)),
      snapshot.analysis && (snapshot.goals ?? []).length > 0 && el("button", {
        class: `gear${goalsOpen ? " on" : ""}${goalsUnseen() ? " dot" : ""}`,
        title: "Goals",
        "aria-pressed": String(goalsOpen),
        onclick: toggleGoals,
      }, icon(TARGET)),
      el("button", {
        class: `gear${skinPage ? " on" : ""}`,
        title: "Skin editor",
        "aria-pressed": String(!!skinPage),
        onclick: () => (skinPage ? closeSkin() : openSkin()),
      }, icon(STAGE)),
      el("button", { class: `gear${settingsOpen ? " on" : ""}`, title: "Settings", "aria-pressed": String(settingsOpen), onclick: toggleSettings }, icon(GEAR))),
    update(),
    readHint(),
    statusLine(c),
    !account && c.status === "disconnected" && c.message && el("p", { class: "status warn" }, c.message),
    reconnect && !settingsOpen
      && el("button", { class: "key go", disabled: slow, onclick: () => action("connect") }, "Connect again"),
    helperDown && el("p", { class: "status warn" }, "Lost Hashi's helper. Reconnecting."),
    settingsOpen
      ? settings(c, host, reconnect)
      : notesOpen
        ? notesView()
      : reportOpen
        ? reportView()
      : goalsOpen
        ? goalsView()
      : profilePage
        ? profileView()
      : skinPage
        ? skinView()
      : cutter
        ? cutterView()
      : rater
        ? raterView()
      : board
        ? boardView()
      : maker
        ? makerView()
      : trainer
        ? trainerView()
      : analysis
        ? analysisView()
      : [pickedMap(), snapshot.plays.length
        ? playList(snapshot.plays)
        : el("div", { class: "nothing" },
            el("p", { class: "say" }, snapshot.sendPlays ? "No plays yet." : "Sending plays is off."),
            el("p", { class: "lead" }, snapshot.sendPlays
              ? "Finish a mania map and it shows up here."
              : snapshot.analysis
                ? "The maps you finish stay off your profile, but still show up here with their analysis."
                : "The maps you finish stay off your profile. Turn it back on in the settings."),
            waiting(), el("i"), el("i"), el("i"))]);
}

// a different view opens at its top, not wherever the last one was scrolled to
let shownView = null;

function render() {
  const node = document.activeElement;
  if (heldFocus || !node || node === document.body || !app.contains(node)) return draw();
  heldFocus = node;
  let range = null;
  try { range = [node.selectionStart, node.selectionEnd]; } catch {}
  try { draw(); } finally { heldFocus = null; }
  if (!node.isConnected || document.activeElement === node) return;
  node.focus({ preventScroll: true });
  try { if (range[0] != null) node.setSelectionRange(...range); } catch {}
}

function draw() {
  if (themeDraft) {
    renderWaiting = true;
    return;
  }
  if (OVERLAY) {
    if (snapshot) paintTheme();
    return drawOverlay();
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
  paintTheme();
  const hasAccount = c.username && !["disconnected", "connecting"].includes(c.status);
  if (c.status === "connected" && hasAccount && wasConnecting && !STILL.matches) {
    crossAt = Date.now();
    if (document.hidden) document.addEventListener("visibilitychange", () => crossAt && (cross(), render()), { once: true });
    else cross();
  }
  if (!hasAccount) crossAt = 0;
  wasConnecting = c.status === "connecting";
  const atHome = (hasAccount || (snapshot.skipConnect && c.status !== "connecting")) && !crossAt;
  if (!atHome) {
    settingsOpen = notesOpen = goalsOpen = reportOpen = false;
    analysis = maker = null;
    dropTrainer();
    dropCutter();
    rater = null;
    skinPage = profilePage = null;
    board = null;
    clearTimeout(boardTimer);
  }
  if (snapshot.reader.client === "lazer") rater = null;
  boardChanged();
  analysisChanged();
  skinFollow();
  raterFollow();

  if (!snapshot.titlebar && location.origin === BRIDGE) {
    app.append(el("div", { class: "corner" },
      snapshot.onTop != null && overlayButton(),
      snapshot.tray && el("button", { title: "Hide to tray", onclick: () => windowAction("hide") }, icon(TRAY)),
      el("button", { title: "Maximize", onclick: () => windowAction("maximize") }, icon(MAXIMIZE)),
      el("button", { class: "close", title: "Quit", onclick: () => windowAction("close") }, icon(`<path d="m.5.5 9 9m0-9-9 9"/>`))));
  }

  if (atHome) {
    app.append(el("div", { class: "ground" }, home(c, host), shore(settingsOpen, bottleVersion(), sentBottle())));
    const view = settingsOpen ? "settings" : notesOpen ? "notes" : reportOpen ? "report" : goalsOpen ? "goals" : profilePage ? "profile" : skinPage ? "skin" : cutter ? "cut" : rater ? "rate" : board ? "board" : maker ? "maker" : trainer ? "trainer" : analysis ? `analysis ${analysis.id}` : "plays";
    document.documentElement.classList.toggle("roomy", !!(analysis || skinPage || cutter || rater) && ROOMY.matches);
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
      : [
          c.message && el("p", { class: "note" }, c.message),
          !crossAt && el("p", { class: "note" },
            el("button", { class: "link", disabled: slow, onclick: () => action("skip-connect/on") }, "Continue without connecting")),
        ],
    update(),
  ];
  app.append(el("div", { class: `connect${full ? " full" : ""}` },
    intro(crossAt ? "flat" : connecting ? "down" : "up"),
    el("div", { class: "river" }, river, full && riverbed())));
  wakeInk();

  if (helperDown) app.append(el("p", { class: "status warn" }, "Lost Hashi's helper. Reconnecting."));
  if (snapshot.plays.length) app.append(playList(snapshot.plays));
}

function paintTheme() {
  if (snapshot.theme) applyTheme(snapshot.theme);
  const own = snapshot.theme && !isDefaultTheme(snapshot.theme);
  document.documentElement.style.setProperty("--accent", (!own && snapshot.connection.accent) || "var(--osu-pink)");
}

// windows: no system frame, so draw a title bar
let titlebar = null;
let pin = null;
function showTitlebar() {
  if (OVERLAY || titlebar || !snapshot?.titlebar || location.origin !== BRIDGE) return;
  pin = snapshot.onTop != null && el("button", { class: "pin", title: "Always on top", onclick: () => action(`on-top/${snapshot.onTop ? "off" : "on"}`) }, icon(PIN));
  titlebar = el("header", {
    class: "titlebar",
    onmousedown: (e) => e.button === 0 && e.detail === 1 && !e.target.closest("button") && windowAction("drag"),
    ondblclick: (e) => !e.target.closest("button") && windowAction("maximize"),
  },
    el("img", { src: "images/app.svg", alt: "", width: 16, height: 16 }),
    el("span", { class: "name" }, "Hashi"),
    pin,
    overlayButton(),
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
  events.addEventListener("skin-player", (ev) => skinPlayerNews(JSON.parse(ev.data)));
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
