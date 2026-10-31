// Settings, the theme color and updates.

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

function sharing() {
  const p = snapshot.presence;
  const plays = snapshot.sendPlays;
  const live = p.enabled;
  const liveAvailable = p.status !== "unsupported";
  const liveNote = p.status === "error" ? p.message : PRESENCE_NOTES[p.status];
  const choice = (sendPlays, shareLive) => (sendPlays ? (shareLive ? "both" : "plays") : shareLive ? "live" : "none");
  const row = (title, on, can, next, line, cls) => el("div", { class: `setting${cls}` },
    el("div", { class: "text" },
      el("span", { class: "title" }, title),
      line && el("span", { class: "line" }, line)),
    el("button", {
      class: `switch${on ? " on" : ""}`,
      role: "switch",
      "aria-checked": String(on),
      "aria-label": title,
      disabled: slow || (!on && !can),
      onclick: () => action(`sharing/${next}`),
    }, el("i")));
  return [
    row("Send my plays", plays, true, choice(!plays, live), !plays && `Off. The maps you finish aren't sent${snapshot.analysis ? ", only analyzed here" : ""}.`, ""),
    row("Share my live status", live, liveAvailable, choice(plays, !live),
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
    snapshot.analysis != null && toggle("Analysis", snapshot.analysis, "analysis",
      "Reads your replays for each play's analysis, goals and your profile."),
    snapshot.analysis && snapshot.thumb && el("div", { class: "setting sub" },
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
    el("div", { class: "setting" },
      el("div", { class: "text" },
        el("span", { class: "title" }, "Skin editor"),
        el("span", { class: "line" }, "Preview your osu! skin as you edit it.")),
      el("button", { class: "key", onclick: () => openSkin() }, "Open")),
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
    skinPage = profilePage = null;
    board = null;
    clearTimeout(boardTimer);
  }
  render();
}
