// in scene units: the note's length, and room past its end for late releases
const RELEASE_NOTE = 230;
const RELEASE_PAST = 50;
const RELEASE_MAX_H = 300;
const RELEASE_LABELS = 22;

function releaseScroll() {
  const speed = Number(stored(SKIN_SPEED, 24));
  return Math.min(SKIN_SPEED_MAX, Math.max(SKIN_SPEED_MIN, Number.isFinite(speed) ? speed : 24));
}

function releaseScene(d) {
  analysis.releaseScene ??= (async () => {
    // Hashi's edits start from the skin's original, so what's applied goes back on top of it
    const applied = await (await fetch(`${BRIDGE}/skin/applied`)).json().catch(() => ({}));
    return loadSkinScene({ keys: d.keys, stamp: analysis.openedAt, edits: {}, editsJson: applied?.[d.keys]?.edits ?? "" });
  })().catch(() => null);
  return analysis.releaseScene;
}

function releaseView(d, width, tip) {
  const cols = d.columns.map((c) => c.release);
  const wrap = el("div", { class: "release-view" });
  const canvas = el("canvas", { class: "release-shot" });
  wrap.append(canvas);
  const speed = releaseScroll();
  const gapNote = el("span");
  const draw = (loaded) => {
    const geo = releaseDraw(canvas, loaded, cols, speed, width, d.keys);
    gapNote.textContent = geo.gapped ? " The dashed line is the note's real end, where releases count from: your skin's art stops short of it." : "";
    canvas.onmousemove = (e) => {
      const x = e.clientX - canvas.getBoundingClientRect().left;
      const i = geo.findIndex(([l, r]) => x >= l && x < r);
      if (i < 0) return tip.hide();
      const r = cols[i];
      const who = d.columns[i].finger ? `, ${d.columns[i].hand} ${d.columns[i].finger}` : "";
      if (!r) return tip.show(e, `Column ${i + 1}`, "Too few long notes here to tell");
      tip.show(e, `${msText(r.mean)} ms ${leanWord(r.mean, 0.05)}`, `column ${i + 1}${who}`, `usually within ±${msText(r.sd)} ms, over ${r.n} long notes`);
    };
    canvas.onmouseleave = () => tip.hide();
  };
  draw(null);
  releaseScene(d).then((loaded) => loaded && wrap.isConnected && draw(loaded));
  return [wrap, el("p", { class: "caption" }, "Where on each long note you let go, on average.", gapNote)];
}

// empty rows at the top of a body image, the tail's end, are the skin's body gap
const releaseLeads = new WeakMap();
function releaseLead(img) {
  if (releaseLeads.has(img)) return releaseLeads.get(img);
  let lead = 0;
  try {
    const c = document.createElement("canvas");
    c.width = img.naturalWidth;
    c.height = img.naturalHeight;
    const x = c.getContext("2d", { willReadFrequently: true });
    x.drawImage(img, 0, 0);
    const px = x.getImageData(0, 0, c.width, c.height).data;
    const row = c.width * 4;
    while (lead < c.height - 1) {
      let seen = false;
      for (let i = lead * row + 3; i < (lead + 1) * row; i += 4) {
        if (px[i] >= 25) {
          seen = true;
          break;
        }
      }
      if (seen) break;
      lead++;
    }
  } catch {}
  releaseLeads.set(img, lead);
  return lead;
}

// the body gap's length in scene units, null when there's no art to draw
function releaseBody(ctx, img, b, x, w, end, to) {
  const lead = releaseLead(img);
  const rows = img.naturalHeight;
  if (lead >= rows - 1) return null;
  if (b.style === 0) {
    ctx.drawImage(img, 0, 0, img.naturalWidth, rows, x, end, w, to - end);
    return (lead / rows) * (to - end);
  }
  const scale = b.row > 0 ? b.row : 1;
  for (let y = end; y < to; y += rows * scale) {
    const n = Math.min(rows, (to - y) / scale);
    ctx.drawImage(img, 0, 0, img.naturalWidth, n, x, y, w, n * scale);
  }
  return lead * scale;
}

// each column's left and right edge on the canvas, for the tooltip
function releaseDraw(canvas, loaded, cols, speed, width, keys) {
  const s = loaded?.scene;
  const images = loaded?.images ?? [];
  const n = s ? s.columns.length : keys;
  const plainW = 80;
  const spots = s
    ? s.columns.map((c) => [c.x, c.w])
    : Array.from({ length: n }, (_, i) => [i * plainW, plainW - 8]);
  const left = Math.min(...spots.map(([x]) => x));
  const right = Math.max(...spots.map(([x, w]) => x + w));
  const units = s ? s.hitY / skinSpeedMs(speed, s.ownHit) : 402 / skinSpeedMs(speed, 402);
  const late = Math.max(0, ...cols.filter(Boolean).map((r) => r.mean));
  const past = Math.max(RELEASE_PAST, late * units + 20);
  const heads = s ? s.columns.map((c) => skinFrame(c.head, 0) ?? skinFrame(c.tap, 0)) : [];
  const headH = Math.max(30, ...heads.map((f) => f?.h ?? 0));
  const viewH = past + RELEASE_NOTE + headH / 2 + 8;
  const k = Math.min(width / (right - left), (RELEASE_MAX_H - RELEASE_LABELS) / viewH);
  const w = Math.round(width);
  const h = Math.round(viewH * k + RELEASE_LABELS);
  const dpr = devicePixelRatio || 1;
  canvas.width = Math.round(w * dpr);
  canvas.height = Math.round(h * dpr);
  canvas.style.width = `${w}px`;
  canvas.style.height = `${h}px`;
  const ctx = canvas.getContext("2d");
  const shift = (w - (right - left) * k) / 2;
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  ctx.clearRect(0, 0, w, h);
  ctx.save();
  if (s?.upscroll) {
    ctx.translate(0, viewH * k);
    ctx.scale(1, -1);
  }
  ctx.translate(shift, 0);
  ctx.scale(k, k);
  ctx.translate(-left, 0);
  const end = past;
  const headY = end + RELEASE_NOTE;
  const gaps = [];
  spots.forEach(([x, cw], c) => {
    ctx.fillStyle = "rgba(255, 255, 255, 0.04)";
    ctx.fillRect(x, 0, cw, viewH);
    if (!cols[c]) return;
    const col = s?.columns[c];
    const head = heads[c];
    const tail = col && skinFrame(col.tail, 0);
    const art = col?.body && images[col.body.s];
    gaps[c] = art ? releaseBody(ctx, art, col.body, x, cw, end, headY) : null;
    if (gaps[c] == null) {
      ctx.fillStyle = (art && skinArtColour(art)) ?? (col ? skinColor(col.headColor) : "rgb(190, 200, 230)");
      ctx.fillRect(x + cw * 0.08, end, cw * 0.84, headY - end);
    }
    if (tail && tail.top != null && images[tail.s]) {
      // stable draws a tail upside down on downscroll
      const [sw, sh] = s.sprites[tail.s];
      ctx.save();
      ctx.translate(0, end * 2);
      ctx.scale(1, -1);
      ctx.drawImage(images[tail.s], 0, 0, sw, sh, x, end, cw, tail.h);
      ctx.restore();
    }
    if (head && images[head.s]) {
      const [sw, sh] = s.sprites[head.s];
      ctx.drawImage(images[head.s], 0, 0, sw, sh, x, headY - head.h / 2, cw, head.h);
    } else {
      ctx.fillStyle = "#e8ecff";
      ctx.beginPath();
      ctx.roundRect(x + 2, headY - 14, cw - 4, 28, 8);
      ctx.fill();
    }
  });
  ctx.restore();
  const font = getComputedStyle(document.body).fontFamily;
  ctx.textAlign = "center";
  ctx.textBaseline = "top";
  const flip = (y) => (s?.upscroll ? viewH * k - y : y);
  // a gap of a couple of units is the art's own edge, not a body gap
  const gapped = gaps.map((g) => g != null && g >= 3);
  const edges = spots.map(([x, cw], c) => {
    const [x0, x1] = [shift + (x - left) * k, shift + (x + cw - left) * k];
    const r = cols[c];
    if (r && gapped[c]) {
      const y = flip(end * k);
      ctx.setLineDash([4, 3]);
      ctx.strokeStyle = "rgba(255, 255, 255, 0.7)";
      ctx.lineWidth = 1.5;
      ctx.beginPath();
      ctx.moveTo(x0 + (x1 - x0) * 0.08, y);
      ctx.lineTo(x1 - (x1 - x0) * 0.08, y);
      ctx.stroke();
      ctx.setLineDash([]);
    }
    ctx.font = `700 12px ${font}`;
    ctx.fillStyle = r ? "#fff" : "rgba(255, 255, 255, 0.35)";
    ctx.fillText(r ? signedMs(r.mean) : "–", (x0 + x1) / 2, h - RELEASE_LABELS + 5);
    if (r) {
      // edged dark to show on a body of any colour
      const y = flip((end - r.mean * units) * k);
      ctx.fillStyle = "rgba(0, 0, 0, 0.75)";
      ctx.fillRect(x0 - 4, y - 3, x1 - x0 + 8, 6);
      ctx.fillStyle = "#fff";
      ctx.fillRect(x0 - 3, y - 1.5, x1 - x0 + 6, 3);
    }
    return [x0, x1];
  });
  edges.gapped = gapped.some((g, c) => g && cols[c]);
  return edges;
}
