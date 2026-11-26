// supporters' name styles and avatar frames, drawn the way Mania Tracker's lists draw them
const OUTLINE_WIDTHS = ["0.04em", "0.07em", "0.105em", "0.14em"];
// how far a frame may reach past the avatar's slot before the avatar shrinks to make room
const FRAME_REACH = { x: 2.2, y: 1.5 };
// wider than this, the frame's sides fade out
const FRAME_CLIP = 1.8;
const FRAMED_SQUARE_RADIUS = `${(16 / 124) * 100}%`;

let looksSheet = null;
function wantLooksSheet() {
  const href = snapshot.connection.looksSheet;
  if (!href || looksSheet?.getAttribute("href") === href) return;
  looksSheet?.remove();
  looksSheet = el("link", { rel: "stylesheet", href });
  document.head.append(looksSheet);
}

// the name in its style, font and outline; its own element, so the slot it sits in keeps its layout
function styledName(text, name) {
  if (!name) return text;
  wantLooksSheet();
  const cls = [];
  const style = [];
  if (name.style) {
    const solid = /^solid-([0-9a-f]{6})$/.exec(name.style);
    const bare = name.style === "terminal-bare";
    cls.push("name-tag", `name-tag--${solid ? "solid" : bare ? "terminal" : name.style}`);
    if (bare) cls.push("name-tag--bare");
    if (solid) style.push(`color: #${solid[1]}`);
  }
  if (name.font) cls.push(`name-font--${name.font}`);
  if (name.outline) {
    cls.push("name-outline");
    const thickness = name.outline.length > 6 ? Number(name.outline[6]) : OUTLINE_WIDTHS.length;
    style.push(`--name-outline: #${name.outline.slice(0, 6)}`, `--name-outline-width: ${OUTLINE_WIDTHS[thickness - 1]}`);
  }
  return el("bdi", { class: `look-name ${cls.join(" ")}`, style: style.join("; ") || null }, text);
}

// the player's own row wears the look the header does, which is fetched again when the window
// comes back. the board's own until that's loaded
function rowLook(e) {
  const c = snapshot.connection;
  return c.userId != null && String(e.userId) === String(c.userId) ? c.look ?? e.look : e.look;
}

// transparent WebM needs VP9 alpha, which WebKit drops
function canPlayFrames() {
  const ua = navigator.userAgent;
  if (/AppleWebKit/.test(ua) && !/Chrome|Chromium|Edg\//.test(ua)) return false;
  if (matchMedia("(prefers-reduced-motion: reduce)").matches) return false;
  return document.createElement("video").canPlayType('video/webm; codecs="vp9"') !== "";
}

// kept across redraws, so a playing frame doesn't start over
const frameArt = new Map();
const FRAMES_KEPT = 600;

function frameNode(frame, play, key) {
  // avatars here are round: a frame with a round version uses it, and only its own loop
  const video = play && canPlayFrames() ? (frame.round ? frame.roundVideo : frame.video) : null;
  const still = frame.round ?? frame.still;
  const id = video ?? still;
  let node = frameArt.get(key);
  if (!node || node.dataset.art !== id) {
    node = video
      ? el("video", { class: "look-frame", src: video, poster: still, autoplay: true, loop: true, playsinline: true, disablepictureinpicture: true, "aria-hidden": "true" })
      : el("img", { class: "look-frame", src: still, alt: "", draggable: "false", "aria-hidden": "true" });
    if (video) node.muted = true;
    node.dataset.art = id;
    if (frameArt.size >= FRAMES_KEPT) frameArt.delete(frameArt.keys().next().value);
    frameArt.set(key, node);
  }
  const [left, top, width, height] = frame.box;
  node.style.cssText = `left: ${left * 100}%; top: ${top * 100}%; width: ${width * 100}%; height: ${height * 100}%`;
  return node;
}

// an avatar slot `size` px wide; with a frame, the avatar shrinks so the whole frame fits around it, centred
function lookFace(cls, content, frame, size, { play = false, key = "" } = {}) {
  const img = content instanceof HTMLImageElement ? content : null;
  if (!frame || !img) {
    if (img) img.style.borderRadius = img.style.filter = "";
    return el("span", { class: cls }, content);
  }
  img.style.borderRadius = frame.square ? FRAMED_SQUARE_RADIUS : "50%";
  img.style.filter = frame.avatarFilter ?? "";
  const [l, t, r, b] = frame.overhang;
  const fit = Math.min(1, FRAME_REACH.x / (1 + l + r), FRAME_REACH.y / (1 + t + b));
  const inner = size * fit;
  const x = (size - inner * (1 + l + r)) / 2 + inner * l;
  const y = (size - inner * (1 + t + b)) / 2 + inner * t;
  const fade = inner * (1 + l + r) > size * FRAME_CLIP;
  const pad = { x: fade ? (size * (FRAME_CLIP - 1)) / 2 : 0, y: fade ? size : 0 };
  const px = (n) => `${n.toFixed(2)}px`;
  return el("span", { class: `${cls} has-frame` },
    el("span", { class: `look-fit${fade ? " faded" : ""}`, style: `left: ${px(-pad.x)}; top: ${px(-pad.y)}; width: ${px(size + 2 * pad.x)}; height: ${px(size + 2 * pad.y)}` },
      el("span", { class: "look-face", style: `left: ${px(pad.x + x)}; top: ${px(pad.y + y)}; width: ${px(inner)}; height: ${px(inner)}` },
        img, frameNode(frame, play, key))));
}

// a look just picked on the site, or supporter just bought, shows once the window is back in front
addEventListener("focus", () => fetch(`${BRIDGE}/looks/mine`, { method: "POST" }).catch(() => {}));
