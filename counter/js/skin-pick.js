// What an edit goes on: parts of columns, picked by pressing them in the preview, dragging a box
// over them, or in the grid. Colors and Parts both work on them. Kept as "head:3" and so on
const SKIN_PICK_PARTS = [["note", "Notes"], ["head", "LN Heads"], ["body", "LN Bodies"], ["tail", "LN Tails"], ["key", "Receptors"], ["keyDown", "Pressed"]];
const SKIN_NOTE_PARTS = SKIN_PICK_PARTS.slice(0, 4);
// the parts by the name their colors go under
const SKIN_COLOR_OF = { note: "notes", head: "heads", body: "bodies", tail: "tails" };
// a press that moves less than this, in screen pixels, is a click
const SKIN_PICK_SLOP = 4;

const skinPickKey = (part, c) => `${part}:${c}`;
const skinPicked = (e) => (e.sel ?? []).map((k) => k.split(":")).map(([part, c]) => ({ part, c: Number(c) }));
const skinIsPicked = (e, part, c) => (e.sel ?? []).includes(skinPickKey(part, c));

function skinPick(page, keys, add = false) {
  const e = skinEdit(page);
  e.sel = [...new Set([...(add ? e.sel ?? [] : []), ...keys])];
  render();
}

// a group of cells on or off together: on unless they're all on already
function skinPickToggle(page, keys) {
  const e = skinEdit(page);
  const all = keys.every((k) => e.sel?.includes(k));
  e.sel = all ? (e.sel ?? []).filter((k) => !keys.includes(k)) : [...new Set([...(e.sel ?? []), ...keys])];
  render();
}

// "Notes in columns 1–3", "LN Heads in column 4", or how many when it's a mix
function skinPickText(picks, keys) {
  if (!picks.length) return "";
  const names = Object.fromEntries(SKIN_PICK_PARTS);
  const parts = [...new Set(picks.map((p) => p.part))];
  const cols = [...new Set(picks.map((p) => p.c))].sort((a, b) => a - b);
  const whole = picks.length === parts.length * cols.length;
  if (!whole) return `${picks.length} picked`;
  const what = parts.length === 1 ? names[parts[0]] : parts.length === 4 && parts.every((p) => SKIN_COLOR_OF[p]) ? "Everything" : parts.map((p) => names[p]).join(", ");
  if (cols.length === keys) return `${what} in every column`;
  const run = cols.length > 2 && cols.at(-1) - cols[0] === cols.length - 1;
  return `${what} in column${cols.length > 1 ? "s" : ""} ${run ? `${cols[0] + 1}–${cols.at(-1) + 1}` : cols.map((c) => c + 1).join(", ")}`;
}

const SKIN_THUMB = 64;
const skinThumbs = new WeakMap();

// the art cropped to where it shows, as a small picture: a receptor is often a ring at the foot of a
// stage-tall image. `wide` is how much wider than its own shape the preview draws it; a body is
// stretched over the whole picture to show its color along it
function skinArtThumb(img, stretch, wide = 1) {
  const kept = skinThumbs.get(img) ?? {};
  const key = stretch ? "body" : wide.toFixed(2);
  if (key in kept) return kept[key];
  let out = null;
  try {
    const k = Math.min(1, 256 / Math.max(img.naturalWidth, img.naturalHeight));
    const [w, h] = [Math.max(1, Math.round(img.naturalWidth * k)), Math.max(1, Math.round(img.naturalHeight * k))];
    const read = Object.assign(document.createElement("canvas"), { width: w, height: h }).getContext("2d", { willReadFrequently: true });
    read.drawImage(img, 0, 0, w, h);
    const px = read.getImageData(0, 0, w, h).data;
    let [x0, y0, x1, y1] = [w, h, -1, -1];
    for (let y = 0; y < h; y++) {
      for (let x = 0; x < w; x++) {
        if (px[(y * w + x) * 4 + 3] <= 25) continue;
        [x0, y0, x1, y1] = [Math.min(x0, x), Math.min(y0, y), Math.max(x1, x), Math.max(y1, y)];
      }
    }
    if (x1 >= 0) {
      const [cw, ch] = [x1 - x0 + 1, y1 - y0 + 1];
      const f = stretch ? null : SKIN_THUMB / Math.max(cw * wide, ch);
      const [dw, dh] = stretch ? [SKIN_THUMB, SKIN_THUMB] : [cw * wide * f, ch * f];
      const canvas = Object.assign(document.createElement("canvas"), { width: SKIN_THUMB, height: SKIN_THUMB });
      canvas.getContext("2d").drawImage(read.canvas, x0, y0, cw, ch, (SKIN_THUMB - dw) / 2, (SKIN_THUMB - dh) / 2, dw, dh);
      out = canvas.toDataURL();
    }
  } catch {
    out = img.src;
  }
  skinThumbs.set(img, { ...kept, [key]: out });
  return out;
}

// the part's art as the preview draws it
function skinPartArt(page, part, c) {
  const live = page.live;
  const col = live?.scene.columns[c];
  if (!col) return null;
  // the sprite and the box it's drawn into, for its shape on the stage
  let s = null;
  let drawn = null;
  if (part === "body") s = col.body?.s;
  else if (part === "key" || part === "keyDown") {
    const op = (part === "key" ? col.keyUp : col.keyDown).find((o) => o.t === "img");
    [s, drawn] = [op?.s, op && [op.src[2], op.src[3], op.dst[2], op.dst[3]]];
  } else {
    const f = { note: col.tap, head: col.head, tail: col.tail }[part]?.[0];
    if (f?.s != null) [s, drawn] = [f.s, [...live.scene.sprites[f.s].slice(0, 2), col.w, f.h]];
  }
  const img = s == null ? null : live.images[s];
  const wide = drawn && drawn.every((v) => v > 0) ? (drawn[2] / drawn[0]) / (drawn[3] / drawn[1]) : 1;
  const thumb = img?.complete && skinArtThumb(img, part === "body", wide);
  return thumb ? `url("${thumb}") center / contain no-repeat` : null;
}

// the parts by columns: a cell picks one, a row or column's label all of it. Each cell shows its
// part as the preview has it: its art, or its color where there's no art
function skinPickGrid(page, rows) {
  const e = skinEdit(page);
  const keys = page.keys;
  const cols = [...Array(keys).keys()];
  const cell = (part, c) => {
    const art = skinPartArt(page, part, c);
    const color = !art && SKIN_COLOR_OF[part] && (e.colors[SKIN_COLOR_OF[part]][c] ?? skinPartColour(page, c, SKIN_COLOR_OF[part]));
    return el("button", {
      class: `skin-cell${skinIsPicked(e, part, c) ? " on" : ""}${art || color ? "" : " none"}`,
      style: art ? `--art: ${art}` : color ? `--swatch: ${color}` : "",
      title: `${Object.fromEntries(SKIN_PICK_PARTS)[part]}, column ${c + 1}`,
      "aria-pressed": String(skinIsPicked(e, part, c)),
      onclick: () => skinPickToggle(page, [skinPickKey(part, c)]),
    });
  };
  return el("div", { class: "skin-grid", style: `--cols: ${keys}`, "data-drag": "cells" },
    el("span"),
    cols.map((c) => el("button", { class: "skin-grid-col", title: `All of column ${c + 1}`, onclick: () => skinPickToggle(page, rows.map(([p]) => skinPickKey(p, c))) }, c + 1)),
    rows.map(([part, name]) => [
      el("button", { class: "skin-grid-row", title: `${name} in every column`, onclick: () => skinPickToggle(page, cols.map((c) => skinPickKey(part, c))) }, name),
      cols.map((c) => cell(part, c)),
    ]));
}

function skinPickTools(page, rows) {
  const e = skinEdit(page);
  const all = rows.flatMap(([p]) => [...Array(page.keys).keys()].map((c) => skinPickKey(p, c)));
  return el("div", { class: "skin-tools skin-pick-tools" },
    el("span", { class: "skin-label" }, skinPickText(skinPicked(e), page.keys) || "Nothing picked"),
    el("span", {},
      el("button", { class: "link", onclick: () => skinPick(page, all) }, "Pick all"),
      " ",
      e.sel?.length > 0 && el("button", { class: "link", onclick: () => skinPick(page, []) }, "Unpick")));
}

const skinIsKey = (part) => part === "key" || part === "keyDown";

// what's under the point in the preview, top first; a note over a receptor is the note's
function skinPickHit(page, px, py) {
  const under = (page.live?.hits ?? []).filter(({ r: [x, y, w, h] }) => px >= x && px <= x + w && py >= y && py <= y + h).reverse();
  return under.find((h) => !skinIsKey(h.part)) ?? under[0] ?? null;
}

// a press on the preview that isn't on a side picture: a click picks what's under it, a drag picks
// what its box touches. Shift or Ctrl adds to what's picked. The preview holds still meanwhile, so
// what's picked is what was under the pointer
function skinPickStart(page, ev) {
  // a box dragged past the preview's edge isn't a text selection
  ev.preventDefault();
  getSelection()?.removeAllRanges();
  const [x0, y0, k] = skinSidePoint(page, ev);
  const add = ev.shiftKey || ev.ctrlKey || ev.metaKey;
  page.holdStill = true;
  let box = null;
  const move = (m) => {
    const [x1, y1] = skinSidePoint(page, m);
    if (!box && Math.hypot(x1 - x0, y1 - y0) < SKIN_PICK_SLOP * k) return;
    box = [Math.min(x0, x1), Math.min(y0, y1), Math.abs(x1 - x0), Math.abs(y1 - y0)];
    page.pickBox = box;
  };
  const up = () => {
    removeEventListener("pointermove", move);
    removeEventListener("pointerup", up);
    removeEventListener("pointercancel", up);
    page.holdStill = false;
    page.pickBox = null;
    const e = skinEdit(page);
    let keys;
    if (box) {
      const [bx, by, bw, bh] = box;
      const touched = (page.live?.hits ?? []).filter(({ r: [x, y, w, h] }) => x < bx + bw && x + w > bx && y < by + bh && y + h > by);
      // a box round some notes takes the receptors under them only when it's round nothing else
      const notes = touched.filter((h) => !skinIsKey(h.part));
      keys = [...new Set((notes.length ? notes : touched).map((h) => skinPickKey(h.part, h.c)))];
    } else {
      const hit = skinPickHit(page, x0, y0);
      if (!hit) {
        // a click on nothing lets go of what's picked, or pauses when nothing is
        if (e.sel?.length && !add) {
          page.noPause = true;
          skinPick(page, []);
        }
        return;
      }
      keys = [skinPickKey(hit.part, hit.c)];
      // pressing one that's picked, with Shift, unpicks it
      if (add && skinIsPicked(e, hit.part, hit.c)) {
        page.noPause = true;
        return skinPickToggle(page, keys);
      }
    }
    page.noPause = true;
    if (!keys.length && !add) return skinPick(page, []);
    if (!["colours", "parts"].includes(page.tab)) page.tab = keys.every((k) => SKIN_COLOR_OF[k.split(":")[0]]) ? "colours" : "parts";
    skinPick(page, keys, add);
  };
  addEventListener("pointermove", move);
  addEventListener("pointerup", up);
  addEventListener("pointercancel", up);
}

// a receptor's box, every op its art is drawn with
function skinOpsBox(ops) {
  let box = null;
  for (const o of ops) {
    const r = o.dst ?? o.r ?? (o.p && (() => {
      const xs = o.p.filter((_, i) => i % 2 === 0);
      const ys = o.p.filter((_, i) => i % 2 === 1);
      return [Math.min(...xs), Math.min(...ys), Math.max(...xs) - Math.min(...xs), Math.max(...ys) - Math.min(...ys)];
    })());
    if (!r) continue;
    box = !box ? [...r] : (() => {
      const [x, y] = [Math.min(box[0], r[0]), Math.min(box[1], r[1])];
      return [x, y, Math.max(box[0] + box[2], r[0] + r[2]) - x, Math.max(box[1] + box[3], r[1] + r[3]) - y];
    })();
  }
  return box;
}

// what's picked outlined on the preview, and the box being dragged
function skinPickMarks(ctx) {
  const page = skinPage;
  const live = page?.live;
  if (!live) return;
  const e = page.edits?.[page.keys];
  ctx.save();
  if (e?.sel?.length && ["colours", "parts"].includes(page.tab)) {
    ctx.lineWidth = 2;
    ctx.strokeStyle = "#fff";
    ctx.shadowColor = "rgba(0, 0, 0, 0.8)";
    ctx.shadowBlur = 4;
    for (const h of live.hits ?? []) if (skinIsPicked(e, h.part, h.c)) ctx.strokeRect(h.r[0] + 1, h.r[1] + 1, h.r[2] - 2, h.r[3] - 2);
  }
  if (page.pickBox) {
    const [x, y, w, h] = page.pickBox;
    ctx.shadowBlur = 0;
    ctx.fillStyle = "rgba(120, 170, 255, 0.18)";
    ctx.strokeStyle = "rgba(120, 170, 255, 0.9)";
    ctx.lineWidth = 1;
    ctx.fillRect(x, y, w, h);
    ctx.strokeRect(x + 0.5, y + 0.5, w - 1, h - 1);
  }
  ctx.restore();
}

document.addEventListener("keydown", (ev) => {
  const page = skinPage;
  if (!page?.live || ev.target.closest?.("input, textarea")) return;
  if (ev.key === "Escape" && page.edits?.[page.keys]?.sel?.length) {
    skinPick(page, []);
  } else if ((ev.ctrlKey || ev.metaKey) && ev.key.toLowerCase() === "a" && ["colours", "parts"].includes(page.tab)) {
    ev.preventDefault();
    const rows = page.tab === "colours" ? SKIN_NOTE_PARTS : SKIN_PICK_PARTS;
    skinPick(page, rows.flatMap(([p]) => [...Array(page.keys).keys()].map((c) => skinPickKey(p, c))));
  }
});
