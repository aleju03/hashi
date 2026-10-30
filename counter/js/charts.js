// The analysis's charts: the key lanes, the timing spread, every hit over time, the river and the patterns.

// early above the judgement line and late below, the way the notes were falling
function stageView(d, width, tip, laneH = 200) {
  const k = d.columns.length;
  const split = d.columns.findIndex((c) => c.hand === "right");
  const left = 42;
  const gap = k > 7 ? 4 : 6;
  const handGap = split > 0 ? 12 : 0;
  const laneW = (width - left - gap * (k - 1) - handGap) / k;
  const laneX = (i) => left + i * (laneW + gap) + (split > 0 && i >= split ? handGap : 0);
  const laneTop = 26;
  const line = laneTop + laneH / 2;
  const reach = d.windows[2];
  const y = (o) => line + (Math.max(-reach, Math.min(reach, o)) / reach) * (laneH / 2 - 4);
  const bins = 15;
  const binH = (laneH / 2 - 4) / bins;
  const piles = d.columns.map(() => new Array(bins * 2).fill(0));
  // the profile passes how many presses each entry stands for
  for (const [, o, j, c, n = 1] of d.hits) {
    if (j > 5) continue;
    const b = Math.max(0, Math.min(bins * 2 - 1, Math.floor(((o + reach) / (2 * reach)) * bins * 2)));
    piles[c][b] += n;
  }
  const most = Math.max(1, ...piles.flat());
  // by rank, not value, or one bad thumb leaves every other key the same color. KEY_GAP keeps
  // keys all at 99.9-100 from still going green to red
  const accs = d.columns.map((c) => c.acc).filter((a) => a != null);
  const heat = (a) => {
    if (a == null || accs.length < 2) return null;
    const t = accs.filter((x) => x - a >= KEY_GAP).length / (accs.length - 1);
    const stops = [[76, 195, 138], [242, 205, 92], [240, 136, 62], [229, 72, 77]];
    const at = t * (stops.length - 1);
    const i = Math.min(stops.length - 2, Math.floor(at));
    const mix = stops[i].map((v, j) => Math.round(v + (stops[i + 1][j] - v) * (at - i)));
    return `rgb(${mix.join(" ")})`;
  };
  const capTop = laneTop + laneH + 10;
  const capH = 24;
  const holdY = capTop + capH + 16;
  const braceTop = capTop + capH + (d.tapHold != null ? 26 : 10);
  const height = braceTop + 44;
  const root = svg("svg", { class: "chart stage", viewBox: `0 0 ${width} ${height}`, width: "100%", role: "img", "aria-label": "Each column's hits around the judgement line" });

  root.append(svg("text", { class: "side-label", x: 0, y: laneTop + 12 }, "early"));
  root.append(svg("text", { class: "side-label", x: 0, y: laneTop + laneH - 4 }, "late"));
  const lanes = d.columns.map((c, i) => {
    const x = laneX(i);
    const g = svg("g", { class: `lane${heat(c.acc) ? " hot" : ""}`, style: `--wobble: url(#ink${(i % 3) + 1}); --heat: ${heat(c.acc)}` });
    g.append(svg("rect", { class: "lane-bg", x, y: laneTop, width: laneW, height: laneH, rx: 7 }));
    for (const w of d.windows.slice(0, 2)) {
      for (const s of [-1, 1]) g.append(svg("line", { class: "guide", x1: x + 3, x2: x + laneW - 3, y1: y(s * w), y2: y(s * w) }));
    }
    const pile = svg("g", { class: "pile" });
    piles[i].forEach((n, b) => {
      if (!n) return;
      const center = -reach + ((b + 0.5) / (bins * 2)) * 2 * reach;
      const w = Math.max(4, (n / most) * (laneW - 8));
      pile.append(svg("rect", { class: `j${windowOf(center, d.windows)}`, x: x + (laneW - w) / 2, y: laneTop + 4 + b * binH + 0.6, width: w, height: Math.max(1.5, binH - 1.2), rx: Math.min(binH / 2, 3) }));
    });
    g.append(pile);
    if (c.hits) g.append(svg("rect", { class: "avg-note", x: x + laneW * 0.14, y: y(c.mean) - 3.5, width: laneW * 0.72, height: 7, rx: 3.5 }));
    const was = d.previous?.columns?.[i]?.acc;
    const change = c.acc != null && was != null ? c.acc - was : null;
    g.append(svg("text", { class: "lane-acc", x: x + laneW / 2, y: laneTop - 8, "text-anchor": "middle" },
      c.acc == null ? "–" : accText(c.acc, 1),
      change != null && laneW >= 56 && svg("tspan", { class: `delta ${change >= 0 ? "up" : "down"}`, dx: 4 }, deltaText(change))));
    g.append(svg("rect", { class: "cap-base", x: x + 1, y: capTop + 3, width: laneW - 2, height: capH, rx: 6 }));
    g.append(svg("rect", { class: "cap", x: x + 1, y: capTop, width: laneW - 2, height: capH, rx: 6 }));
    const name = laneW >= 40 ? c.finger : { thumb: "th", index: "ix", middle: "md", ring: "rg", pinky: "pk" }[c.finger];
    g.append(svg("text", { class: "cap-label", x: x + laneW / 2, y: capTop + 16, "text-anchor": "middle" }, name));
    if (c.tapHold != null) g.append(svg("text", { class: "hold-label", x: x + laneW / 2, y: holdY, "text-anchor": "middle" }, `${c.tapHold} ms`));
    const hit = svg("rect", { class: "hit", x, y: laneTop - 22, width: laneW, height: capTop - laneTop + 20 });
    const show = (e) => {
      g.classList.add("on");
      tip.show(e, pct(c.acc), `column ${i + 1}, ${c.hand} ${c.finger}`, `${signedMs(c.mean)} ms ±${msText(c.sd)}`, c.tapHold != null && `taps held ${c.tapHold} ms`, c.counts[5] ? `${c.counts[5]} ${c.counts[5] === 1 ? "miss" : "misses"}` : null, was != null && `${pct(was)} last time`);
    };
    const pick = svg("rect", { class: "hit pick", x, y: capTop - 2, width: laneW, height: capH + 6 });
    pick.addEventListener("pointerenter", (e) => {
      g.classList.add("on");
      tip.show(e, `${c.hand} ${c.finger}`, "Click to change which finger plays this key");
    });
    pick.addEventListener("pointerleave", () => {
      g.classList.remove("on");
      tip.hide();
    });
    pick.addEventListener("click", () => !slow && !analysis?.loading && pickFinger(d, i));
    hit.addEventListener("pointerenter", show);
    hit.addEventListener("pointermove", show);
    hit.addEventListener("pointerleave", () => {
      g.classList.remove("on");
      tip.hide();
    });
    g.append(hit, pick);
    return g;
  });
  root.append(...lanes);
  root.append(svg("rect", { class: "judgement", x: left - 4, y: line - 1.5, width: width - left + 8, height: 3, rx: 1.5 }));
  for (const [hand, from, to] of [["left", 0, split < 0 ? k : split], ["right", split < 0 ? k : split, k]]) {
    if (to <= from) continue;
    const x1 = laneX(from) + 2;
    const x2 = laneX(to - 1) + laneW - 2;
    const mid = (x1 + x2) / 2;
    root.append(svg("path", { class: "brace", d: `M${x1} ${braceTop}q0 6 6 6H${mid - 6}q6 0 6 6q0-6 6-6H${x2 - 6}q6 0 6-6` }));
    const h = d.hands[hand];
    const was = d.previous?.hands?.[hand]?.acc;
    root.append(svg("text", { class: "hand-label", x: mid, y: braceTop + 30, "text-anchor": "middle" },
      svg("tspan", { class: "strong" }, `${hand === "left" ? "Left" : "Right"} ${pct(h.acc)}`),
      h.acc != null && was != null && svg("tspan", { class: `delta ${h.acc >= was ? "up" : "down"}`, dx: 4 }, deltaText(h.acc - was)),
      svg("tspan", {}, `  ${signedMs(h.mean)} ms ±${msText(h.sd)}`)));
  }
  return root;
}

function histogram(values, windows, { width, height = 140, reach, label, tip, names, plain = false }) {
  const bins = Math.max(24, Math.min(64, Math.round(width / 8)));
  const step = (reach * 2) / bins;
  const counts = new Array(bins).fill(0);
  for (const v of values) counts[Math.floor((Math.max(-reach, Math.min(reach - 1e-9, v)) + reach) / step)] += 1;
  const top = Math.max(1, ...counts);
  const pad = { l: 4, r: 4, t: 28, b: 24 };
  const plotW = width - pad.l - pad.r;
  const plotH = height - pad.t - pad.b;
  const base = pad.t + plotH;
  const x = (v) => pad.l + ((v + reach) / (reach * 2)) * plotW;
  const barW = Math.max(2, plotW / bins - 2.5);
  const root = svg("svg", { class: "chart histogram", viewBox: `0 0 ${width} ${height}`, width: "100%", role: "img", "aria-label": label });
  const bars = svg("g", { class: "bars" });
  const hits = svg("g");
  counts.forEach((c, i) => {
    const center = -reach + (i + 0.5) * step;
    const h = c ? Math.max(3, (c / top) * plotH) : 0;
    const bx = pad.l + i * (plotW / bins) + 1.25;
    const bar = svg("rect", { class: plain ? "plain" : `j${windowOf(center, windows)}`, x: bx, y: base - h, width: barW, height: h + 3, rx: Math.min(3, barW / 2), style: `--i: ${Math.abs(i - bins / 2)}` });
    bars.append(bar);
    const hit = svg("rect", { class: "hit", x: bx - 1, y: pad.t - 10, width: barW + 2.5, height: plotH + 10 });
    const from = -reach + i * step;
    hit.addEventListener("pointerenter", (e) => {
      bar.classList.add("on");
      tip.show(e, `${c} ${c === 1 ? "hit" : "hits"}`, `${signedMs(from)} to ${signedMs(from + step)} ms`, !plain && names[windowOf(center, windows) - 1]);
    });
    hit.addEventListener("pointerleave", () => {
      bar.classList.remove("on");
      tip.hide();
    });
    hits.append(hit);
  });
  root.append(bars, svg("rect", { class: "baseline", x: pad.l - 2, y: base, width: plotW + 4, height: 2.5, rx: 1.25 }));
  let lastLabel = -Infinity;
  for (const v of [-reach, -windows[1], -windows[0], 0, windows[0], windows[1], reach]) {
    const px = x(v);
    if (Math.abs(v) < reach) root.append(svg("path", { class: "tick", d: `M${px} ${base + 3}v4` }));
    if (px - lastLabel < 28) continue;
    lastLabel = px;
    root.append(svg("text", { class: "tick-label", x: Math.max(pad.l + 8, Math.min(width - pad.r - 8, px)), y: height - 4, "text-anchor": "middle" }, v === 0 ? "0" : `${v > 0 ? "" : "−"}${Math.round(Math.abs(v))}`));
  }
  root.append(svg("text", { class: "edge-label", x: pad.l, y: 10 }, "early"), svg("text", { class: "edge-label", x: width - pad.r, y: 10, "text-anchor": "end" }, "late"));
  if (values.length) {
    const m = values.reduce((a, b) => a + b, 0) / values.length;
    const mx = x(Math.max(-reach, Math.min(reach, m)));
    const right = mx < width - 90;
    const flag = svg("g", { class: "pennant" });
    flag.append(svg("path", { class: "pole", d: `M${mx} ${base}V${pad.t - 20}` }));
    flag.append(svg("path", { class: "cloth", d: right ? `M${mx} ${pad.t - 20}l14 4.5-14 4.5z` : `M${mx} ${pad.t - 20}l-14 4.5 14 4.5z` }));
    flag.append(svg("text", { class: "flag-label", x: mx + (right ? 18 : -18), y: pad.t - 12, "text-anchor": right ? "start" : "end" }, `avg ${signedMs(m)}`));
    root.append(flag);
  }
  root.append(hits);
  return root;
}

function cssColors() {
  const style = getComputedStyle(document.documentElement);
  const get = (n) => style.getPropertyValue(n).trim();
  return { j: [1, 2, 3, 4, 5, 6].map((i) => get(`--j${i}`)), rule: get("--osu-b3"), muted: get("--osu-f1"), ink: get("--ink"), accent: get("--accent") };
}

function scatter(d, tip) {
  const releases = analysis.lane === "releases";
  const points = releases ? d.releases.map(([t, o, c]) => [t, o, d.mode === "stable" ? 0 : windowOf(o / 1.5, d.windows), c]) : d.hits;
  const reach = releases ? d.windows[3] * 1.5 : d.windows[3];
  const wrap = el("div", { class: "scatter" });
  const canvas = el("canvas", { height: 180, role: "img", "aria-label": releases ? "Long note releases over the play" : "Presses over the play" });
  const ring = el("i", { class: "ring", hidden: "" });
  wrap.append(canvas, ring);
  const pad = { l: 36, r: 10, t: 12, b: 22 };
  let layout = null;
  wrap.draw = () => {
    const w = canvas.clientWidth;
    if (!w) return;
    const h = canvas.clientHeight || 180;
    const dpr = devicePixelRatio || 1;
    canvas.width = w * dpr;
    canvas.height = h * dpr;
    const ctx = canvas.getContext("2d");
    ctx.scale(dpr, dpr);
    const c = cssColors();
    const plotW = w - pad.l - pad.r;
    const plotH = h - pad.t - pad.b;
    const x = (t) => pad.l + (t / Math.max(1, d.length)) * plotW;
    const y = (o) => pad.t + plotH / 2 + (Math.max(-reach, Math.min(reach, o)) / reach) * (plotH / 2);
    layout = { x, y };
    ctx.font = `10px "Tracker Round", sans-serif`;
    ctx.lineWidth = 1;
    ctx.fillStyle = c.muted;
    ctx.textAlign = "right";
    for (const wv of d.windows.slice(0, 2)) {
      const edge = releases ? wv * 1.5 : wv;
      for (const s of [-1, 1]) {
        ctx.strokeStyle = c.rule;
        ctx.beginPath();
        ctx.moveTo(pad.l, Math.round(y(s * edge)) + 0.5);
        ctx.lineTo(w - pad.r, Math.round(y(s * edge)) + 0.5);
        ctx.stroke();
      }
    }
    if (d.practice) {
      const [a, b] = [x(d.practice.from), x(d.practice.to)];
      ctx.fillStyle = c.accent;
      ctx.globalAlpha = 0.14;
      ctx.fillRect(a, pad.t, Math.max(2, b - a), plotH);
      ctx.globalAlpha = 1;
      ctx.fillStyle = c.accent;
      const fits = a + 4 + ctx.measureText("practice").width <= w - pad.r;
      ctx.textAlign = fits ? "left" : "right";
      ctx.fillText("practice", fits ? a + 4 : b - 4, pad.t + 10);
      ctx.fillStyle = c.muted;
      ctx.textAlign = "right";
    }
    ctx.fillText("early", pad.l - 8, pad.t + 8);
    ctx.fillText("late", pad.l - 8, pad.t + plotH);
    if (!releases) {
      ctx.strokeStyle = c.j[5];
      ctx.globalAlpha = 0.5;
      for (const [t] of d.misses) {
        const mx = Math.round(x(t)) + 0.5;
        ctx.beginPath();
        ctx.moveTo(mx, pad.t);
        ctx.lineTo(mx, pad.t + plotH);
        ctx.stroke();
      }
      ctx.globalAlpha = 1;
    }
    const r = points.length > 3000 ? 1.4 : points.length > 1000 ? 1.8 : 2.4;
    ctx.globalAlpha = 0.9;
    for (const [t, o, j] of points) {
      ctx.fillStyle = j ? c.j[Math.min(6, j) - 1] : c.muted;
      ctx.beginPath();
      ctx.arc(x(t), y(o), r, 0, Math.PI * 2);
      ctx.fill();
    }
    ctx.globalAlpha = 1;
    ctx.strokeStyle = c.ink;
    ctx.lineWidth = 2;
    ctx.lineCap = "round";
    ctx.beginPath();
    ctx.moveTo(pad.l - 4, y(0));
    ctx.lineTo(w - pad.r + 4, y(0));
    ctx.stroke();
    ctx.fillStyle = c.muted;
    ctx.textAlign = "left";
    ctx.fillText(clock(d.offset), pad.l, h - 5);
    ctx.textAlign = "center";
    ctx.fillText(clock(d.offset + d.length / 2), pad.l + plotW / 2, h - 5);
    ctx.textAlign = "right";
    ctx.fillText(clock(d.offset + d.length), w - pad.r, h - 5);
  };
  // the nearest point to the pointer, not the one under it
  canvas.addEventListener("pointermove", (e) => {
    if (!layout) return;
    const box = canvas.getBoundingClientRect();
    const px = e.clientX - box.left;
    const py = e.clientY - box.top;
    let best = null;
    let bestD = 24 * 24;
    for (const p of points) {
      const dx = layout.x(p[0]) - px;
      const dy = layout.y(p[1]) - py;
      if (dx * dx + dy * dy < bestD) {
        bestD = dx * dx + dy * dy;
        best = p;
      }
    }
    if (!best) {
      ring.hidden = true;
      return tip.hide();
    }
    ring.hidden = false;
    ring.style.left = `${layout.x(best[0])}px`;
    ring.style.top = `${layout.y(best[1])}px`;
    const f = d.columns[best[3]];
    tip.show(e, `${signedMs(best[1])} ms`, clock(d.offset + best[0]), `column ${best[3] + 1}, ${f.hand} ${f.finger}`, releases ? "release" : d.names[best[2] - 1]);
  });
  canvas.addEventListener("pointerleave", () => {
    ring.hidden = true;
    tip.hide();
  });
  return wrap;
}

// monotone cubic: smooth, but never overshoots the points, so it stays under 100%
function curve(pts) {
  const n = pts.length;
  const f = (v) => v.toFixed(1);
  if (n < 3) return pts.map(([x, y], i) => `${i ? "L" : "M"}${f(x)} ${f(y)}`).join("");
  const slope = pts.slice(1).map(([x, y], i) => (y - pts[i][1]) / (x - pts[i][0]));
  const m = pts.map((_, i) => {
    if (i === 0) return slope[0];
    if (i === n - 1) return slope[n - 2];
    const [a, b] = [slope[i - 1], slope[i]];
    return a * b <= 0 ? 0 : (3 * a * b) / (2 * Math.max(a, b) + Math.min(a, b));
  });
  let d = `M${f(pts[0][0])} ${f(pts[0][1])}`;
  for (let i = 1; i < n; i++) {
    const [[x0, y0], [x1, y1]] = [pts[i - 1], pts[i]];
    const h = (x1 - x0) / 3;
    d += `C${f(x0 + h)} ${f(y0 + m[i - 1] * h)} ${f(x1 - h)} ${f(y1 - m[i] * h)} ${f(x1)} ${f(y1)}`;
  }
  return d;
}

function riverView(d, width, tip) {
  const sections = d.sections;
  const accs = sections.map((s) => s.acc).filter((a) => a != null);
  if (accs.length < 3) return null;
  const floor = Math.max(0, Math.floor(Math.min(...accs) - 1));
  const pad = { l: 36, r: 6 };
  const plotW = width - pad.l - pad.r;
  const step = plotW / sections.length;
  const cx = (i) => pad.l + (i + 0.5) * step;
  const accTop = 10;
  const accBottom = 84;
  const ay = (a) => accTop + (1 - (a - floor) / (100 - floor)) * (accBottom - accTop);
  const surface = 104;
  const bed = 152;
  const height = 176;
  const root = svg("svg", { class: "chart stream", viewBox: `0 0 ${width} ${height}`, width: "100%", role: "img", "aria-label": "Accuracy, density and misses over the play" });
  if (d.practice) {
    const tx = (t) => pad.l + (t / Math.max(1, d.length)) * plotW;
    const [a, b] = [tx(d.practice.from), tx(d.practice.to)];
    root.append(svg("rect", { class: "practice-band", x: a, y: accTop - 6, width: Math.max(2, b - a), height: bed - accTop + 6, rx: 4 }));
    // about the label's width: near the end of the song it goes inside the band's right edge
    const fits = a + 50 <= width - pad.r;
    root.append(svg("text", { class: "practice-label", x: fits ? a + 4 : b - 4, y: accTop + 4, "text-anchor": fits ? "start" : "end" }, "practice"));
  }
  for (const v of [floor, 100]) {
    root.append(svg("path", { class: "grid", d: `M${pad.l} ${ay(v)}H${width - pad.r}` }));
    root.append(svg("text", { class: "tick-label", x: pad.l - 6, y: ay(v) + 3, "text-anchor": "end" }, `${v}%`));
  }
  const top = Math.max(1, ...sections.map((s) => s.nps));
  const wy = (n) => bed - (n / top) * (bed - surface);
  const pts = sections.map((s, i) => [cx(i), wy(s.nps)]);
  const water = `M${pad.l} ${pts[0][1]}L${curve(pts).slice(1)}L${width - pad.r} ${pts[pts.length - 1][1]}`;
  root.append(svg("path", { class: "flow", d: `${water}V${bed}H${pad.l}Z` }));
  root.append(svg("path", { class: "flow-edge", d: water }));
  root.append(svg("text", { class: "tick-label", x: pad.l - 6, y: surface + 4, "text-anchor": "end" }, `${Math.round(top)}/s`));
  root.append(svg("path", { class: "bed-line", d: `M${pad.l - 6} ${bed + 2}q12-4 24 0${"t24 0".repeat(Math.ceil(plotW / 24))}` }));
  const sunk = svg("g", { class: "sunk-notes" });
  d.misses.forEach(([t], i) => {
    const mx = pad.l + (t / Math.max(1, d.length)) * plotW;
    const turn = ((i * 47) % 70) - 35;
    sunk.append(svg("rect", { x: mx - 5, y: bed - 6 - ((i * 13) % 3) * 3, width: 10, height: 5, rx: 2.5, transform: `rotate(${turn} ${mx} ${bed - 4})` }));
  });
  root.append(sunk);
  root.append(svg("path", { class: "acc-line", d: curve(sections.map((s, i) => s.acc != null && [cx(i), ay(s.acc)]).filter(Boolean)) }));
  root.append(svg("text", { class: "tick-label", x: pad.l, y: height - 2 }, clock(d.offset)));
  root.append(svg("text", { class: "tick-label", x: width - pad.r, y: height - 2, "text-anchor": "end" }, clock(d.offset + d.length)));
  const cross = svg("path", { class: "cross", hidden: "" });
  const dot = svg("circle", { class: "dot", r: 4.5, hidden: "" });
  root.append(cross, dot);
  const hit = svg("rect", { class: "hit", x: pad.l, y: 0, width: plotW, height: bed + 6 });
  hit.addEventListener("pointermove", (e) => {
    const box = root.getBoundingClientRect();
    const px = ((e.clientX - box.left) / box.width) * width;
    const i = Math.max(0, Math.min(sections.length - 1, Math.floor((px - pad.l) / step)));
    const s = sections[i];
    cross.setAttribute("d", `M${cx(i)} ${accTop - 4}V${bed}`);
    cross.removeAttribute("hidden");
    if (s.acc != null) {
      dot.setAttribute("cx", cx(i));
      dot.setAttribute("cy", ay(s.acc));
      dot.removeAttribute("hidden");
    } else dot.setAttribute("hidden", "");
    tip.show(e, s.acc != null ? pct(s.acc) : "No notes", `${clock(d.offset + s.at)}–${clock(d.offset + s.at + d.sectionLength)}`, `${s.nps} notes a second`, s.misses ? `${s.misses} ${s.misses === 1 ? "miss" : "misses"}` : null);
  });
  hit.addEventListener("pointerleave", () => {
    cross.setAttribute("hidden", "");
    dot.setAttribute("hidden", "");
    tip.hide();
  });
  root.append(hit);
  return root;
}

const PATTERN_GLYPHS = {
  Singles: `<rect x="8" y="9" width="14" height="6" rx="3"/>`,
  Chords: `<rect x="1" y="9" width="13" height="6" rx="3"/><rect x="16" y="9" width="13" height="6" rx="3"/>`,
  Jacks: `<rect x="8" y="3" width="14" height="6" rx="3"/><rect x="8" y="15" width="14" height="6" rx="3"/>`,
  "Long notes": `<rect x="8" y="1" width="14" height="22" rx="3"/>`,
};

function patternsView(d) {
  return el("div", { class: "patterns" },
    el("div", { class: "prow head" }, el("span"), el("span"), el("span", {}, "Notes"), el("span", {}, "Accuracy"), el("span", {}, "Timing")),
    d.patterns.map((p, i) => {
      const glyph = el("span", { class: "pglyph" });
      glyph.innerHTML = `<svg viewBox="0 0 30 24" width="30" height="24" aria-hidden="true">${PATTERN_GLYPHS[p.name] ?? ""}</svg>`;
      return el("div", { class: "prow", style: `--wobble: url(#ink${(i % 3) + 1})` },
        glyph,
        el("b", {}, p.name),
        el("span", {}, p.notes),
        el("span", {}, pct(p.acc)),
        el("span", {}, p.hits ? `${signedMs(p.mean)} ms` : "–"));
    }));
}
