// StageLeft, StageRight and StageBottom as pictures, in pixels of a 768-tall screen from the stage's
// edge (its middle for StageBottom), x negative on the left. The helper draws each group into one image
const SIDE_HEIGHT = 768;
const SIDE_REACH = 2048;
const SIDE_LAYER_MAX = 4096;
const SIDE_SMALLEST = 8;
// a picture added starts no wider than this, so it fits beside the stage
const SIDE_ADD_WIDTH = 600;
// screen pixels around a corner that resize instead of moving
const SIDE_GRAB = 10;
// "over" is StageBottom, which the game draws over the notes, centred on the stage
const SIDES = [["left", "Left"], ["over", "Over the stage"], ["right", "Right"]];
// upscroll turns StageBottom upside down at the top, where pictures wouldn't land as placed
const skinSideGroups = (page) => SIDES.filter(([w]) => w !== "over" || !page.live?.scene.upscroll);
// its art is drawn at its own size in 480ths of the screen's height
const SIDE_BOTTOM_UNITS = SIDE_HEIGHT / 480;
const SIDE_BIG = 150 * 150;

// the skin's own image as the one picture, where the game shows it. StageBottom's sits on the
// stage's bottom edge, or as high as the Stage tab raised it
function skinSideOwn(page, which) {
  const own = page.live?.scene.ownSides?.[which];
  if (!own) return [];
  const raise = which === "over" ? (page.edits?.[page.keys]?.bottom ?? 0) * SIDE_BOTTOM_UNITS : 0;
  const x = which === "left" ? -own.w : which === "over" ? -own.w / 2 : 0;
  return [{ src: own.src, x: Math.round(x), y: Math.round(SIDE_HEIGHT - own.h - raise), w: Math.round(own.w), h: Math.round(own.h), flip: false }];
}

const skinSideEdge = (s, which) => (which === "left" ? s.stageEdges[0] : which === "right" ? s.stageEdges[1] : (s.stageEdges[0] + s.stageEdges[1]) / 2);

const skinSideLayers = (page, e, which) => e.sides?.[which] ?? skinSideOwn(page, which);
const skinSideWhich = (e) => e.sideWhich ?? "left";

// the picked pictures of the picked side: sideAt, then any picked along with it
const skinSidePicked = (e) => (e.sideAt == null ? [] : [e.sideAt, ...(e.sideAlso ?? [])]);

function skinSidePick(e, which, i, add = false) {
  if (add && e.sideAt != null && skinSideWhich(e) === which) {
    const all = skinSidePicked(e);
    const next = all.includes(i) ? all.filter((x) => x !== i) : [...all, i];
    [e.sideAt, e.sideAlso] = [next[0] ?? null, next.slice(1)];
  } else [e.sideWhich, e.sideAt, e.sideAlso] = [which, i, []];
}

const skinSideUnpick = (e) => ([e.sideAt, e.sideAlso] = [null, []]);

// An edit to the picked side's pictures, which start as the skin's own; `which` picks another first.
// The pictures may move in the list, so only one stays picked
function skinSideEdit(page, change, which = null) {
  skinChange(page, (x) => {
    if (which) x.sideWhich = which;
    which = skinSideWhich(x);
    x.sideAlso = [];
    x.sides ??= { left: null, right: null };
    x.sides[which] ??= skinSideOwn(page, which);
    change(x.sides[which], x);
  });
}

function skinSideUrl(page, src) {
  const id = src.startsWith("upload:") ? src.slice(7) : null;
  return id ? `${BRIDGE}/skin/upload/${id}.png` : `${BRIDGE}/skin/thumb?path=${encodeURIComponent(src)}&at=${page.stamp}`;
}

const skinSideName = (l) => `${l.src.startsWith("upload:") ? "Your picture" : l.src.split(/[\\/]/).pop()}${l.crop ? ", a part" : ""}`;

// each part shows just its piece of the same file, where it was
async function skinSideSplit(page, i) {
  const e = skinEdit(page);
  const which = skinSideWhich(e);
  const l = skinSideLayers(page, e, which)[i];
  const was = JSON.stringify(l);
  page.sideError = page.sideNote = null;
  let found = null;
  try {
    const res = await fetch(`${BRIDGE}/skin/split?path=${encodeURIComponent(l.src)}&at=${page.stamp}`);
    if (res.ok) found = await res.json();
  } catch {}
  if (skinPage !== page || JSON.stringify(skinSideLayers(page, e, which)[i]) !== was) return;
  if (!found) {
    page.sideError = "Couldn't read the picture.";
    return render();
  }
  const [cx, cy, cw, ch] = l.crop ?? [0, 0, found.w, found.h];
  const parts = found.pieces
    .map(([x, y, w, h]) => [Math.max(x, cx), Math.max(y, cy), Math.min(x + w, cx + cw), Math.min(y + h, cy + ch)])
    .filter(([x0, y0, x1, y1]) => x1 > x0 && y1 > y0)
    .map(([x0, y0, x1, y1]) => [x0, y0, x1 - x0, y1 - y0]);
  if (parts.length < 2) {
    page.sideNote = "That picture is all one part.";
    return render();
  }
  skinSideEdit(page, (list, x) => {
    const made = parts.map(([px, py, pw, ph]) => {
      const [rx, rw] = [(px - cx) / cw, pw / cw];
      return {
        src: l.src, crop: [px, py, pw, ph], flip: !!l.flip,
        x: Math.round(l.x + l.w * (l.flip ? 1 - rx - rw : rx)), y: Math.round(l.y + (l.h * (py - cy)) / ch),
        w: Math.round(l.w * rw), h: Math.round((l.h * ph) / ch),
      };
    });
    list.splice(i, 1, ...made);
    skinSideUnpick(x);
  }, which);
  page.sideNote = `Split into ${parts.length} parts.`;
  render();
}

function skinSideBlit(ctx, img, l, x, y, w, h) {
  const [sx, sy, sw, sh] = l.crop ?? [0, 0, img.naturalWidth, img.naturalHeight];
  ctx.save();
  if (l.flip) {
    ctx.translate(x + w, y);
    ctx.scale(-1, 1);
    [x, y] = [0, 0];
  }
  ctx.drawImage(img, sx, sy, sw, sh, x, y, w, h);
  ctx.restore();
}

const SIDE_THUMB = 96;

function skinSideThumb(page, l) {
  if (!l.crop) return el("img", { src: skinSideUrl(page, l.src), alt: "", loading: "lazy" });
  const canvas = el("canvas", { width: SIDE_THUMB, height: SIDE_THUMB });
  const img = skinSideArt(page, l.src);
  const draw = () => {
    const k = Math.min(SIDE_THUMB / l.crop[2], SIDE_THUMB / l.crop[3]);
    const [w, h] = [l.crop[2] * k, l.crop[3] * k];
    skinSideBlit(canvas.getContext("2d"), img, l, (SIDE_THUMB - w) / 2, (SIDE_THUMB - h) / 2, w, h);
  };
  if (img.complete && img.naturalWidth) draw();
  else img.addEventListener("load", draw, { once: true });
  return canvas;
}

// `at` is where it was dropped: the side and the point its middle goes on, else it goes against the
// stage's edge at the bottom of the picked side
function skinSideAdd(page, src, w, h, at = null) {
  const fit = Math.min(1, SIDE_HEIGHT / h, SIDE_ADD_WIDTH / w);
  const [lw, lh] = [Math.round(w * fit), Math.round(h * fit)];
  skinSideEdit(page, (list, x) => {
    const which = skinSideWhich(x);
    const [px, py] = at ? [Math.round(at.x - lw / 2), Math.round(at.y - lh / 2)] : [which === "left" ? -lw : which === "over" ? -Math.round(lw / 2) : 0, SIDE_HEIGHT - lh];
    list.push({ src, x: px, y: py, w: lw, h: lh, flip: false });
    x.sideAt = list.length - 1;
  }, at?.which);
}

function skinSideAt(page, ev) {
  const s = page.live?.scene;
  if (!s?.stageEdges) return null;
  const [px, py] = skinSidePoint(page, ev);
  const which = skinSideRegion(page, px);
  const f = s.height / SIDE_HEIGHT;
  return { which, x: (px - skinSideEdge(s, which)) / f, y: py / f };
}

const skinSideImage = (items) => [...(items ?? [])].find((i) => i.kind === "file" && i.type.startsWith("image/"))?.getAsFile();

const SIDE_PASTE_WAIT_MS = 150;

// A picture pasted anywhere in the editor goes on the side being edited. Linux's webview hands the
// page no picture, so when none comes with the paste the helper reads what's copied
document.addEventListener("keydown", (ev) => {
  const page = skinPage;
  if (!page?.live || !(ev.ctrlKey || ev.metaKey) || ev.key.toLowerCase() !== "v" || ev.target.closest?.("input, textarea")) return;
  clearTimeout(page.pasteTimer);
  page.pasteTimer = setTimeout(() => skinSideUpload(page, null), SIDE_PASTE_WAIT_MS);
});

document.addEventListener("paste", (ev) => {
  const page = skinPage;
  if (!page?.live || ev.target.closest?.("input, textarea")) return;
  const file = skinSideImage(ev.clipboardData?.items);
  if (!file) return;
  clearTimeout(page.pasteTimer);
  ev.preventDefault();
  skinSideUpload(page, file);
});

const skinSideDragging = (ev) => [...(ev.dataTransfer?.items ?? [])].some((i) => i.kind === "file" && i.type.startsWith("image/"));

function skinSideDragOver(page, ev) {
  if (!page?.live || !skinSideDragging(ev)) return;
  ev.preventDefault();
  ev.stopPropagation();
  ev.dataTransfer.dropEffect = "copy";
}

// on the preview it goes where it's dropped, anywhere else on the page as if it was added
function skinSideDrop(page, ev) {
  const file = [...(ev.dataTransfer?.files ?? [])].find((f) => f.type.startsWith("image/"));
  if (!page?.live || !file) return;
  ev.preventDefault();
  ev.stopPropagation();
  skinSideUpload(page, file, ev.currentTarget === page.canvas ? skinSideAt(page, ev) : null);
}

// no file reads what's copied. Only a picture added opens the Sides tab: Ctrl+V with text copied
// shouldn't take the editor elsewhere
async function skinSideUpload(page, file, at = null) {
  page.sideError = null;
  page.sideBusy = true;
  if (file) page.tab = "sides";
  render();
  try {
    const res = await fetch(`${BRIDGE}/skin/${file ? "upload" : "paste"}`, { method: "POST", body: file });
    const body = await res.json().catch(() => ({}));
    if (!res.ok) throw new Error(body.error ?? "Couldn't add the picture.");
    if (skinPage === page) {
      page.tab = "sides";
      skinSideAdd(page, body.src, body.w, body.h, at);
    }
  } catch (e) {
    page.sideError = e.message === "Failed to fetch" ? "Lost Hashi's helper." : e.message;
  }
  page.sideBusy = false;
  if (skinPage === page) render();
}

// each picture's place on the card, the picked side's last so it's found first
function skinSideBoxes(page) {
  const s = page.live?.scene;
  if (!s?.stageEdges) return [];
  const e = skinEdit(page);
  const f = s.height / SIDE_HEIGHT;
  const picked = skinSideWhich(e);
  return [...skinSideGroups(page).map(([w]) => w).filter((w) => w !== picked), picked].flatMap((which) => {
    const edge = skinSideEdge(s, which);
    return skinSideLayers(page, e, which).map((l, i) => ({ which, i, x: edge + l.x * f, y: l.y * f, w: l.w * f, h: l.h * f }));
  });
}

function skinSideHit(page, px, py, grab) {
  const boxes = skinSideBoxes(page);
  for (let n = boxes.length - 1; n >= 0; n--) {
    const b = boxes[n];
    const corner = [[0, 0], [1, 0], [0, 1], [1, 1]].find(([cx, cy]) => Math.hypot(px - (b.x + cx * b.w), py - (b.y + cy * b.h)) <= grab);
    if (corner || (px >= b.x && px <= b.x + b.w && py >= b.y && py <= b.y + b.h)) return { ...b, corner };
  }
  return null;
}

function skinSideCursor(page, ev) {
  if (ev.buttons || !page.live) return;
  const [px, py, k] = skinSidePoint(page, ev);
  const hit = skinSideHit(page, px, py, SIDE_GRAB * k);
  page.canvas.style.cursor = !hit ? "" : !hit.corner ? "move" : hit.corner[0] === hit.corner[1] ? "nwse-resize" : "nesw-resize";
}

function skinSidePoint(page, ev) {
  const box = page.canvas.getBoundingClientRect();
  const k = page.live.scene.width / box.width;
  return [(ev.clientX - box.left) * k, (ev.clientY - box.top) * k, k];
}

// the picture at full size, drawn by the page while its side is dragged
function skinSideArt(page, src) {
  const all = (page.sideArt ??= new Map());
  if (!all.has(src)) {
    const img = new Image();
    img.src = `${BRIDGE}/skin/art?path=${encodeURIComponent(src)}&at=${page.stamp}`;
    all.set(src, img);
  }
  return all.get(src);
}

// A picture dragged on the preview, or by a corner to resize it, the art keeping its shape unless
// Shift is held. One step of undo for the whole drag. Meanwhile the helper draws the side empty and
// the page draws its pictures, so the drag never waits on it
function skinSideDrag(page, ev) {
  if (ev.button !== 0 || !page.live) return;
  const [px, py, k] = skinSidePoint(page, ev);
  const hit = skinSideHit(page, px, py, SIDE_GRAB * k);
  const e = skinEdit(page);
  // from any tab: pressing a picture is for moving it, not pausing
  if (hit) page.tab = "sides";
  if (!hit) {
    // a click beside the pictures lets go of the picked one, and only that
    if (e.sideAt != null) {
      skinSideUnpick(e);
      page.noPause = true;
      render();
    }
    return;
  }
  ev.preventDefault();
  page.noPause = true;
  if (ev.shiftKey || ev.ctrlKey || ev.metaKey) {
    skinSidePick(e, hit.which, hit.i, true);
    render();
    return;
  }
  const before = JSON.stringify(e);
  const picked = skinSideWhich(e) === hit.which ? skinSidePicked(e) : [];
  // pressing one of several picked moves them all; a corner resizes just that one
  const together = !hit.corner && picked.length > 1 && picked.includes(hit.i);
  if (!together) skinSidePick(e, hit.which, hit.i);
  e.sides ??= { left: null, right: null };
  e.sides[hit.which] ??= skinSideOwn(page, hit.which);
  const list = e.sides[hit.which];
  const moving = together ? picked.map((i) => list[i]).filter(Boolean) : [list[hit.i]];
  const starts = moving.map((one) => ({ ...one }));
  const l = list[hit.i];
  const start = { ...l };
  page.sideMoving = new Set(hit.corner ? [l] : moving);
  const f = page.live.scene.height / SIDE_HEIGHT;
  for (const one of e.sides[hit.which]) skinSideArt(page, one.src);
  page.sideHidden = hit.which;
  refreshSkin(page);
  const move = (m) => {
    const [mx, my] = skinSidePoint(page, m);
    const [dx, dy] = [(mx - px) / f, (my - py) / f];
    if (!hit.corner) {
      moving.forEach((one, n) => {
        one.x = Math.round(starts[n].x + dx);
        one.y = Math.round(starts[n].y + dy);
      });
    } else {
      const [cx, cy] = hit.corner;
      let w = start.w + (cx ? dx : -dx);
      let h = start.h + (cy ? dy : -dy);
      if (!m.shiftKey) {
        const r = Math.max(w / start.w, h / start.h);
        [w, h] = [start.w * r, start.h * r];
      }
      w = Math.round(Math.min(SIDE_LAYER_MAX, Math.max(SIDE_SMALLEST, w)));
      h = Math.round(Math.min(SIDE_LAYER_MAX, Math.max(SIDE_SMALLEST, h)));
      l.x = cx ? start.x : start.x + start.w - w;
      l.y = cy ? start.y : start.y + start.h - h;
      Object.assign(l, { w, h });
    }
  };
  const up = () => {
    removeEventListener("pointermove", move);
    removeEventListener("pointerup", up);
    removeEventListener("pointercancel", up);
    skinSideRehome(page, e, hit.which, [...page.sideMoving], l);
    page.sideMoving = null;
    if (JSON.stringify(e) !== before) {
      const h = skinHistory(page);
      h.undo.push(before);
      if (h.undo.length > SKIN_HISTORY_KEPT) h.undo.shift();
      h.redo = [];
      page.saved = null;
    }
    page.sideHidden = null;
    refreshSkin(page);
    render();
  };
  // on the window: a redraw mid-drag would drop a capture
  addEventListener("pointermove", move);
  addEventListener("pointerup", up);
  addEventListener("pointercancel", up);
  render();
}

function skinSideActions(page, i) {
  const e = skinEdit(page);
  const layers = skinSideLayers(page, e, skinSideWhich(e));
  const edit = (change) => skinSideEdit(page, change);
  return [
    { text: "Up", title: "In front of the next one", disabled: i === layers.length - 1, run: () => edit((list, x) => { [list[i], list[i + 1]] = [list[i + 1], list[i]]; x.sideAt = i + 1; }) },
    { text: "Down", title: "Behind the one before", disabled: i === 0, run: () => edit((list, x) => { [list[i], list[i - 1]] = [list[i - 1], list[i]]; x.sideAt = i - 1; }) },
    { text: "Flip", on: !!layers[i].flip, run: () => edit((list) => (list[i].flip = !list[i].flip)) },
    { text: "Split", title: "One picture per part of it with space around, to move apart", run: () => skinSideSplit(page, i) },
    { text: "Remove", run: () => edit((list, x) => { list.splice(i, 1); skinSideUnpick(x); }) },
  ];
}

function skinSideManyActions(page) {
  const e = skinEdit(page);
  const picked = skinSidePicked(e);
  return [
    { text: `Join the ${picked.length}`, title: "Make them one picture, as they show together", run: () => skinSideJoin(page) },
    { text: `Remove the ${picked.length}`, run: () => skinSideEdit(page, (list, x) => {
      const keep = list.filter((_, i) => !picked.includes(i));
      list.splice(0, list.length, ...keep);
      skinSideUnpick(x);
    }) },
  ];
}

// the joined picture takes the place in the list of the top one of them
async function skinSideJoin(page) {
  const e = skinEdit(page);
  const which = skinSideWhich(e);
  const layers = skinSideLayers(page, e, which);
  const picked = skinSidePicked(e).filter((i) => layers[i]).sort((a, b) => a - b);
  if (picked.length < 2) return;
  const was = JSON.stringify(layers);
  page.sideError = page.sideNote = null;
  let body = null;
  try {
    const res = await fetch(`${BRIDGE}/skin/join?at=${page.stamp}`, { method: "POST", body: JSON.stringify({ layers: picked.map((i) => layers[i]) }) });
    body = await res.json().catch(() => null);
    if (!res.ok) throw new Error(body?.error ?? "Couldn't join them.");
  } catch (err) {
    page.sideError = err.message === "Failed to fetch" ? "Lost Hashi's helper." : err.message;
    return render();
  }
  if (skinPage !== page || JSON.stringify(skinSideLayers(page, e, which)) !== was) return;
  const at = picked[picked.length - 1] - (picked.length - 1);
  skinSideEdit(page, (list, x) => {
    const keep = list.filter((_, i) => !picked.includes(i));
    keep.splice(at, 0, { src: body.src, x: Math.round(body.x), y: Math.round(body.y), w: Math.round(body.w), h: Math.round(body.h), flip: false });
    list.splice(0, list.length, ...keep);
    skinSidePick(x, which, at);
  }, which);
}

// the picked ones' actions when it's one of several picked
function skinSideMenuOpen(page, ev) {
  if (!page.live) return;
  const [px, py, k] = skinSidePoint(page, ev);
  const hit = skinSideHit(page, px, py, SIDE_GRAB * k);
  if (!hit) return;
  ev.preventDefault();
  page.tab = "sides";
  const e = skinEdit(page);
  const many = skinSideWhich(e) === hit.which && skinSidePicked(e).length > 1 && skinSidePicked(e).includes(hit.i);
  if (!many) skinSidePick(e, hit.which, hit.i);
  page.sideMenu = { x: ev.clientX, y: ev.clientY, i: hit.i, many };
  render();
}

const SIDE_MENU_HEIGHT = 200;

function skinSideMenu(page) {
  const m = page.sideMenu;
  const close = () => { page.sideMenu = null; render(); };
  return el("div", {
    class: "skin-side-menu", role: "menu",
    style: `left: ${m.x}px; top: ${Math.max(0, Math.min(m.y, innerHeight - SIDE_MENU_HEIGHT))}px`,
    oncontextmenu: (ev) => ev.preventDefault(),
  }, (m.many ? skinSideManyActions(page) : skinSideActions(page, m.i)).map((a) => el("button", {
    role: "menuitem", disabled: a.disabled, title: a.title,
    onclick: () => { page.sideMenu = null; a.run(); render(); },
  }, a.text === "Flip" && a.on ? "Flip back" : a.text)),
  el("button", { role: "menuitem", class: "cancel", onclick: close }, "Close"));
}

// after the click, so the redraw doesn't take away what was clicked
document.addEventListener("click", (ev) => {
  if (skinPage?.sideMenu && !ev.target.closest?.(".skin-side-menu")) {
    skinPage.sideMenu = null;
    render();
  }
});
document.addEventListener("keydown", (ev) => {
  if (ev.key === "Escape" && skinPage?.sideMenu) {
    skinPage.sideMenu = null;
    render();
  }
});

const SIDE_NUDGE_BIG = 10;
// presses closer together than this are one step of undo, and the helper draws once they stop
const SIDE_NUDGE_REST_MS = 600;

// the arrow keys move the picked picture a pixel, ten with Shift. Like a drag, the page draws its
// side until they stop
document.addEventListener("keydown", (ev) => {
  const page = skinPage;
  const by = { ArrowLeft: [-1, 0], ArrowRight: [1, 0], ArrowUp: [0, -1], ArrowDown: [0, 1] }[ev.key];
  const e = page?.edits?.[page.keys];
  if (!by || !page.live || e?.sideAt == null || ev.ctrlKey || ev.metaKey || ev.altKey || ev.target.closest?.("input, textarea, select")) return;
  const which = skinSideWhich(e);
  const list = e.sides?.[which] ?? skinSideOwn(page, which);
  const moving = skinSidePicked(e).map((i) => list[i]).filter(Boolean);
  if (!moving.length) return;
  ev.preventDefault();
  if (!page.nudge) {
    page.nudge = { before: JSON.stringify(e) };
    for (const one of list) skinSideArt(page, one.src);
    page.sideHidden = which;
    refreshSkin(page);
  }
  e.sides ??= { left: null, right: null, over: null };
  e.sides[which] = list;
  const step = ev.shiftKey ? SIDE_NUDGE_BIG : 1;
  for (const l of moving) {
    l.x += by[0] * step;
    l.y += by[1] * step;
  }
  clearTimeout(page.nudge.timer);
  page.sideMoving = new Set(moving);
  page.nudge.timer = setTimeout(() => {
    const { before } = page.nudge;
    page.nudge = null;
    skinSideRehome(page, e, which, moving, moving[0]);
    page.sideMoving = null;
    if (JSON.stringify(e) !== before) {
      const h = skinHistory(page);
      h.undo.push(before);
      if (h.undo.length > SKIN_HISTORY_KEPT) h.undo.shift();
      h.redo = [];
      page.saved = null;
    }
    page.sideHidden = null;
    refreshSkin(page);
    render();
  }, SIDE_NUDGE_REST_MS);
  render();
});

// the edits the helper draws, with the side being dragged left empty for the page to draw
function skinSideScene(page, edits) {
  const which = page.sideHidden;
  if (!which) return edits;
  const out = edits ? JSON.parse(edits) : {};
  out.sides = { ...out.sides, [which]: [] };
  return JSON.stringify(out);
}

// The group a point across the card belongs to, the nearer side over an upscroll stage. A picture
// being moved shows as it will once let go, in the group its middle is over
function skinSideRegion(page, x) {
  const [a, b] = page.live.scene.stageEdges;
  if (x < a) return "left";
  if (x > b) return "right";
  return skinSideGroups(page).some(([w]) => w === "over") ? "over" : x < (a + b) / 2 ? "left" : "right";
}

function skinSideDraw(page, ctx, which) {
  const s = page.live.scene;
  const e = skinEdit(page);
  const edge = skinSideEdge(s, which);
  const f = s.height / SIDE_HEIGHT;
  for (const l of skinSideLayers(page, e, which)) {
    const img = skinSideArt(page, l.src);
    if (!img.complete || !img.naturalWidth) continue;
    const region = page.sideMoving?.has(l) ? skinSideRegion(page, edge + (l.x + l.w / 2) * f) : which;
    ctx.save();
    ctx.beginPath();
    if (region === "over") ctx.rect(0, 0, s.width, s.height);
    else if (region === "left") ctx.rect(0, 0, s.stageEdges[0], s.height);
    else ctx.rect(s.stageEdges[1], 0, s.width - s.stageEdges[1], s.height);
    ctx.clip();
    skinSideBlit(ctx, img, l, edge + l.x * f, l.y * f, l.w * f, l.h * f);
    ctx.restore();
  }
}

// Pictures moved into another group's part of the screen join that group, staying where they show.
// The one grabbed stays picked, with any moved along with it that ended up beside it
function skinSideRehome(page, e, which, moved, grabbed) {
  const s = page.live?.scene;
  if (!s?.stageEdges || !grabbed) return;
  const f = s.height / SIDE_HEIGHT;
  const from = skinSideEdge(s, which);
  for (const l of moved) {
    const to = skinSideRegion(page, from + (l.x + l.w / 2) * f);
    if (to === which) continue;
    e.sides[which].splice(e.sides[which].indexOf(l), 1);
    e.sides[to] ??= skinSideOwn(page, to);
    l.x = Math.round((from + l.x * f - skinSideEdge(s, to)) / f);
    e.sides[to].push(l);
  }
  const home = SIDES.map(([w]) => w).find((w) => e.sides[w]?.includes(grabbed));
  const list = e.sides[home];
  e.sideWhich = home;
  e.sideAt = list.indexOf(grabbed);
  e.sideAlso = moved.filter((l) => l !== grabbed && list.includes(l)).map((l) => list.indexOf(l));
}

// the dragged side's pictures, where the game draws the sides and StageBottom: over the notes, under
// the judgements and combo
function skinSidePictures(ctx) {
  const page = skinPage;
  if (page?.live?.hidden) skinSideDraw(page, ctx, page.live.hidden);
}

function skinSideMarks(ctx) {
  const page = skinPage;
  if (!page?.live || page.tab !== "sides") return;
  const e = skinEdit(page);
  const picked = skinSidePicked(e);
  const boxes = skinSideBoxes(page).filter((b) => b.which === skinSideWhich(e) && picked.includes(b.i));
  ctx.save();
  ctx.lineWidth = 1.5;
  ctx.strokeStyle = "#fff";
  ctx.fillStyle = "#fff";
  for (const b of boxes) {
    ctx.strokeRect(b.x, b.y, b.w, b.h);
    for (const [cx, cy] of [[0, 0], [1, 0], [0, 1], [1, 1]]) ctx.fillRect(b.x + cx * b.w - 4, b.y + cy * b.h - 4, 8, 8);
  }
  ctx.restore();
}

// a click anywhere outside the preview and the Sides panel lets go of the picked picture, after
// whatever it was for
document.addEventListener("click", (ev) => {
  const page = skinPage;
  const e = page?.edits?.[page.keys];
  if (e?.sideAt == null || ev.target === page.canvas || ev.target.closest?.(".skin-sides, .skin-side-menu")) return;
  skinSideUnpick(e);
  render();
});

function skinSides(page) {
  const e = skinEdit(page);
  if (!skinSideGroups(page).some(([w]) => w === skinSideWhich(e))) e.sideWhich = "left";
  const which = skinSideWhich(e);
  const layers = skinSideLayers(page, e, which);
  const picked = skinSidePicked(e).filter((i) => i < layers.length);
  const at = picked.length === 1 ? picked[0] : null;
  const l = at == null ? null : layers[at];
  const pick = (i) => (ev) => { skinSidePick(e, which, i, ev.shiftKey || ev.ctrlKey || ev.metaKey); render(); };
  const edit = (change) => skinSideEdit(page, (list, x) => change(list, x));
  const set = (key) => (v) => edit((list) => (list[at][key] = v));
  const number = (short, label, value, min, max, key) => el("div", { class: "skin-side-number" },
    el("span", { class: "skin-label" }, short), skinNumber(page, label, Math.round(value), min, max, set(key)));
  const row = (i) => {
    const one = layers[i];
    return el("div", { class: `skin-layer${picked.includes(i) ? " on" : ""}` },
      el("button", { class: "skin-thumb", title: one.src, onclick: pick(i) }, skinSideThumb(page, one)),
      el("button", { class: "skin-layer-name", onclick: pick(i) }, skinSideName(one)),
      el("div", { class: "segs" }, skinSideActions(page, i).map((a) => el("button", {
        class: `seg${a.on ? " on" : ""}`, disabled: a.disabled, title: a.title, onclick: a.run,
      }, a.text))));
  };
  page.sideFile ??= el("input", {
    type: "file", accept: "image/png,image/jpeg,image/webp,image/gif,image/bmp", hidden: true,
    onchange: (ev) => {
      const file = ev.target.files?.[0];
      ev.target.value = "";
      if (file) skinSideUpload(skinPage, file);
    },
  });
  const list = page.images?.list;
  const usable = (list ?? []).filter((img) => img.frames <= 1 && !page.blanks?.has(img.path));
  const big = usable.filter((img) => img.w * img.h >= SIDE_BIG);
  const rest = usable.filter((img) => img.w * img.h < SIDE_BIG);
  const tile = (img) => el("button", {
    class: "skin-thumb", title: `${img.path}, ${img.w}×${img.h}`,
    onclick: () => skinSideAdd(page, img.path, img.hd ? img.w / 2 : img.w, img.hd ? img.h / 2 : img.h),
  }, el("img", { src: skinSideUrl(page, img.path), alt: "", loading: "lazy" }));
  return part("skin-sides", null,
    el("div", { class: "skin-tools" },
      segs(skinSideGroups(page), which, (w) => { e.sideWhich = w; skinSideUnpick(e); render(); }, "Side"),
      e.sides?.[which] && el("button", { class: "link", onclick: () => skinChange(page, (x) => { x.sides[which] = null; skinSideUnpick(x); }) }, "Back to the skin's own")),
    el("p", { class: "caption" }, "Drag a picture on the preview to move it, or a corner to resize it. Hold Shift to stretch it. Shift-click picks more than one, to move or join them. The arrow keys move what's picked a pixel, ten with Shift. Paste a picture or drop one on the preview to add it."),
    which === "over" && el("p", { class: "caption" }, "Over the stage shows on top of the notes, so keep it clear of where you hit them."),
    picked.length > 1 && el("div", { class: "skin-tools" },
      el("span", { class: "skin-label" }, `${picked.length} picked`),
      el("div", { class: "segs" }, skinSideManyActions(page).map((a) => el("button", { class: "seg", title: a.title, onclick: a.run }, a.text)))),
    layers.length
      ? el("div", { class: "skin-layers" }, [...layers.keys()].reverse().map(row))
      : el("p", { class: "caption" }, "Nothing on this side."),
    l && [
      skinRow("Place", "From the stage's edge, in pixels of a 768-tall screen", el("div", { class: "skin-side-place" },
        number("X", "Side X", l.x, -SIDE_REACH, SIDE_REACH, "x"),
        number("Y", "Side Y", l.y, -SIDE_HEIGHT, SIDE_HEIGHT * 2, "y"))),
      skinRow("Size", null, el("div", { class: "skin-side-place" },
        number("W", "Side width", l.w, SIDE_SMALLEST, SIDE_LAYER_MAX, "w"),
        number("H", "Side height", l.h, SIDE_SMALLEST, SIDE_LAYER_MAX, "h"))),
    ],
    el("div", { class: "skin-tools" },
      el("button", { class: "key", disabled: page.sideBusy, onclick: () => page.sideFile.click() }, page.sideBusy ? "Adding…" : "Add a picture"),
      page.sideFile),
    page.sideError && el("p", { class: "status bad" }, page.sideError),
    page.sideNote && el("p", { class: "caption" }, page.sideNote),
    el("p", { class: "caption" }, "Or one of the skin's images:"),
    !list ? el("p", { class: "caption" }, "Finding the skin's images…") : el("div", { class: "skin-thumbs", onwheel: skinListWheel },
      big.map(tile),
      rest.length > 0 && el("button", { class: "link skin-others", onclick: () => { page.otherImages = !page.otherImages; render(); } },
        `${page.otherImages ? "Hide" : "Show"} smaller images (${rest.length})`),
      page.otherImages && rest.map(tile)));
}
