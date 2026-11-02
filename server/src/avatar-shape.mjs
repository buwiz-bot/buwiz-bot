// avatar-shape.mjs — what a bot's avatar body may be, and the avatar
// designer's candidate logic. Pure: no config, no store, no model client
// (routes/bots.mjs wires those in), so the unit tests import it as is.
//
// A body is a kit shape (the app's mark engine draws 13 of them) or
// "custom": an SVG outline in a 0 0 100 100 box, y down, drawn with M/L/C/Q/Z
// only. The engine fills it with one colour and adds the eyes itself.
import { readFileSync } from "node:fs";

/** The kit: name → what it looks like (the designer's prompt reads these). */
export const KIT = {
  blob: "a ball",
  cloud: "a cloud",
  square: "a rounded square",
  sparkle: "a 4-point star",
  clover: "4 leaves",
  heart: "a heart",
  flower: "a star flower",
  teardrop: "a teardrop",
  tablet: "a wide pill",
  wedge: "a rounded triangle",
  house: "a house",
  star: "a 6-point star",
  hex: "a hexagon",
};
export const KIT_SHAPES = Object.keys(KIT);
/** The app's swatches (the old /avatar/gen picks among these). */
export const SWATCHES = [
  "#FFFFFF", "#936439", "#FF263C", "#FF6700", "#FF9800", "#00C972",
  "#00BCA6", "#1084FE", "#9159FE", "#FF309B", "#777777",
];
export const OUTLINE_MAX = 4000;

const COLOR = /^#[0-9A-Fa-f]{6}$/;
const OUTLINE_CHARS = /^[MLCQZmlcqz0-9.,\-\s]*$/;
const ARGS = { M: 2, L: 2, C: 6, Q: 4, Z: 0 };
// curve sampling for the geometry checks
const STEPS = { C: 16, Q: 10 };

/** Flattens M/L/C/Q/Z path data (absolute and relative, implicit repeats)
 *  into a polygon. → {points:[[x,y]], coords:[[x,y]] (every point the data
 *  names, control points too), subpaths, closed} or throws on bad syntax. */
export function flattenPath(d) {
  const tokens = String(d).match(/[MLCQZmlcqz]|-?(?:\d+\.?\d*|\.\d+)/g) || [];
  // the tokens must account for every non-separator character ("1.2.3" is
  // two numbers in SVG; reject such run-ons rather than guess)
  if (tokens.join("").length !== String(d).replace(/[\s,]/g, "").length || /\.\d*\./.test(d))
    throw new Error("outline has a malformed number");
  const points = [], coords = [];
  let cur = [0, 0], start = [0, 0], cmd = "", subpaths = 0, closed = false, i = 0;
  const num = () => {
    const t = tokens[i];
    if (t === undefined || /[A-Za-z]/.test(t)) throw new Error(`outline command ${cmd} is missing numbers`);
    i++;
    return Number(t);
  };
  const pt = (rel) => {
    const x = num(), y = num();
    const p = rel ? [cur[0] + x, cur[1] + y] : [x, y];
    coords.push(p);
    return p;
  };
  while (i < tokens.length) {
    const t = tokens[i];
    if (/[A-Za-z]/.test(t)) {
      cmd = t;
      i++;
    } else if (!cmd) throw new Error("outline must start with M");
    else if (cmd === "Z" || cmd === "z") throw new Error("outline has numbers after Z");
    // after a moveto's first pair, more pairs are linetos
    else if (cmd === "M") cmd = "L";
    else if (cmd === "m") cmd = "l";
    const up = cmd.toUpperCase(), rel = cmd !== up;
    if (!points.length && up !== "M") throw new Error("outline must start with M");
    if (up === "M") {
      if (subpaths) throw new Error("outline must be one shape (a single M)");
      subpaths++;
      cur = start = pt(rel);
      points.push(cur);
    } else if (up === "Z") {
      closed = true;
      cur = start;
    } else {
      if (closed) throw new Error("outline must be one shape (nothing after Z)");
      if (up === "L") {
        cur = pt(rel);
        points.push(cur);
      } else {
        const p0 = cur;
        const ctl = [];
        for (let k = 0; k < ARGS[up] / 2; k++) ctl.push(pt(rel));
        const n = STEPS[up];
        for (let s = 1; s <= n; s++) {
          const u = s / n, v = 1 - u;
          const [a, b, c, e] = up === "C" ? [v * v * v, 3 * v * v * u, 3 * v * u * u, u * u * u] : [v * v, 2 * v * u, u * u, 0];
          const q = up === "C" ? [p0, ...ctl] : [p0, ...ctl, [0, 0]];
          points.push([
            a * q[0][0] + b * q[1][0] + c * q[2][0] + e * q[3][0],
            a * q[0][1] + b * q[1][1] + c * q[2][1] + e * q[3][1],
          ]);
        }
        cur = ctl.at(-1);
      }
    }
  }
  if (!points.length) throw new Error("outline is empty");
  return { points, coords, subpaths, closed };
}

const areaCentroid = (poly) => {
  let a = 0, cx = 0, cy = 0;
  for (let k = 0; k < poly.length; k++) {
    const [x0, y0] = poly[k], [x1, y1] = poly[(k + 1) % poly.length];
    const f = x0 * y1 - x1 * y0;
    a += f;
    cx += (x0 + x1) * f;
    cy += (y0 + y1) * f;
  }
  return { area: Math.abs(a / 2), c: a ? [cx / (3 * a), cy / (3 * a)] : poly[0] };
};

// even-odd point in polygon
const inside = (poly, x, y) => {
  let hit = false;
  for (let k = 0, j = poly.length - 1; k < poly.length; j = k++) {
    const [xi, yi] = poly[k], [xj, yj] = poly[j];
    if (yi > y !== yj > y && x < ((xj - xi) * (y - yi)) / (yj - yi) + xi) hit = !hit;
  }
  return hit;
};

// How often a ray from c at angle th crosses the polygon's edges.
const crossings = (poly, c, th) => {
  const dx = Math.cos(th), dy = Math.sin(th);
  let n = 0;
  for (let k = 0; k < poly.length; k++) {
    const [ax, ay] = poly[k], [bx, by] = poly[(k + 1) % poly.length];
    const ex = bx - ax, ey = by - ay;
    const den = dx * ey - dy * ex;
    if (Math.abs(den) < 1e-12) continue;
    const wx = ax - c[0], wy = ay - c[1];
    const t = (wx * ey - wy * ex) / den; // along the ray
    const u = (wx * dy - wy * dx) / den; // along the edge
    if (t > 1e-9 && u >= 0 && u < 1) n++;
  }
  return n;
};

// Parts thinner than ~6 units: rasterize at 1 unit, open with a disk of
// radius 2.5 (erode, then dilate); what the opening removes is thin. A
// sharp corner loses a few cells too, so only a sizeable removed patch
// counts.
const THIN_RADIUS = 2.5, THIN_CELLS = 40;
function thinPart(poly) {
  const W = 112, O = -6; // cells cover -6..106
  const at = (x, y) => y * W + x;
  const fill = new Uint8Array(W * W);
  for (let y = 0; y < W; y++) for (let x = 0; x < W; x++) fill[at(x, y)] = inside(poly, x + O + 0.5, y + O + 0.5) ? 1 : 0;
  const disk = [];
  const r = Math.ceil(THIN_RADIUS);
  for (let y = -r; y <= r; y++) for (let x = -r; x <= r; x++) if (x * x + y * y <= THIN_RADIUS * THIN_RADIUS) disk.push([x, y]);
  const morph = (src, keep) => {
    const out = new Uint8Array(W * W);
    for (let y = r; y < W - r; y++)
      for (let x = r; x < W - r; x++) {
        const all = disk.every(([a, b]) => src[at(x + a, y + b)]);
        const any = disk.some(([a, b]) => src[at(x + a, y + b)]);
        out[at(x, y)] = keep === "all" ? (all ? 1 : 0) : any ? 1 : 0;
      }
    return out;
  };
  const opened = morph(morph(fill, "all"), "any");
  const seen = new Uint8Array(W * W);
  for (let s = 0; s < W * W; s++) {
    if (!fill[s] || opened[s] || seen[s]) continue;
    let size = 0;
    const stack = [s];
    seen[s] = 1;
    while (stack.length) {
      const q = stack.pop();
      size++;
      const x = q % W, y = (q - x) / W;
      for (const [nx, ny] of [[x + 1, y], [x - 1, y], [x, y + 1], [x, y - 1]]) {
        if (nx < 0 || ny < 0 || nx >= W || ny >= W) continue;
        const n = at(nx, ny);
        if (fill[n] && !opened[n] && !seen[n]) (seen[n] = 1), stack.push(n);
      }
    }
    if (size > THIN_CELLS) return true;
  }
  return false;
}

/** The problems with an outline's path data, [] when it is a good one. */
export function outlineErrors(outline) {
  if (typeof outline !== "string" || !outline.trim()) return ["a custom shape needs an outline"];
  if (outline.length > OUTLINE_MAX) return [`outline is longer than ${OUTLINE_MAX} characters`];
  if (!OUTLINE_CHARS.test(outline)) return ["outline may hold only M/L/C/Q/Z commands and numbers"];
  let f;
  try {
    f = flattenPath(outline);
  } catch (e) {
    return [e.message];
  }
  const errs = [];
  if (f.coords.some(([x, y]) => !(x >= -5 && x <= 105 && y >= -5 && y <= 105)))
    errs.push("outline coordinates must stay within -5..105");
  const poly = f.points.filter((p, k, a) => k === 0 || Math.hypot(p[0] - a[k - 1][0], p[1] - a[k - 1][1]) > 1e-6);
  const [first, last] = [poly[0], poly.at(-1)];
  const ends = Math.hypot(first[0] - last[0], first[1] - last[1]);
  if (!f.closed && ends > 1) errs.push("outline must be closed (end with Z)");
  if (ends <= 1e-6 && poly.length > 1) poly.pop();
  if (poly.length < 3) return [...errs, "outline encloses no area"];
  const { area, c } = areaCentroid(poly);
  if (area < 2000) errs.push(`outline covers ${Math.round(area / 100)}% of the box; it must cover at least 20%`);
  const xs = poly.map((p) => p[0]), ys = poly.map((p) => p[1]);
  const w = Math.max(...xs) - Math.min(...xs), h = Math.max(...ys) - Math.min(...ys);
  const aspect = h ? w / h : Infinity;
  if (!(aspect >= 0.4 && aspect <= 2.5)) errs.push(`outline's width/height is ${aspect.toFixed(2)}; it must be between 0.4 and 2.5`);
  if (area >= 1) {
    // offset so no ray runs exactly through a vertex of a tidy outline
    let bad = 0;
    for (let k = 0; k < 64; k++) if (crossings(poly, c, ((k + 0.37) * 2 * Math.PI) / 64) !== 1) bad++;
    if (bad) errs.push("outline must be star-shaped from its centre (every ray from the centre crosses the edge once): no crossings, holes, or bays that fold back");
    // (the raster covers the allowed box only: run it on an otherwise good one)
    else if (!errs.length && thinPart(poly)) errs.push("outline has a part thinner than ~6 units");
  }
  return errs;
}

export const LAYERS_MAX = 8;

/** The problems with a rich avatar's painted layers and eye colour: up to
 *  8 closed paths in the outline's 0..100 box, each with a fill (and an
 *  opacity 0..1), and the eyes' colour. */
export function layerErrors(layers, eyes) {
  const errs = [];
  if (!Array.isArray(layers) || !layers.length) return ["a rich avatar needs at least one layer (the face)"];
  if (layers.length > LAYERS_MAX) errs.push(`at most ${LAYERS_MAX} layers`);
  layers.forEach((l, k) => {
    const n = `layer ${k + 1}`;
    if (!l || typeof l !== "object") return errs.push(`${n} must be {path, fill}`);
    if (typeof l.fill !== "string" || !COLOR.test(l.fill)) errs.push(`${n}'s fill must be #RRGGBB`);
    if (l.opacity !== undefined && !(typeof l.opacity === "number" && l.opacity >= 0 && l.opacity <= 1))
      errs.push(`${n}'s opacity must be 0..1`);
    const d = l.path;
    if (typeof d !== "string" || !d.trim()) return errs.push(`${n} needs a path`);
    if (d.length > OUTLINE_MAX) return errs.push(`${n}'s path is longer than ${OUTLINE_MAX} characters`);
    if (!OUTLINE_CHARS.test(d)) return errs.push(`${n}'s path may hold only M/L/C/Q/Z commands and numbers`);
    let f;
    try {
      f = flattenPath(d);
    } catch (e) {
      return errs.push(`${n}: ${e.message}`);
    }
    if (f.coords.some(([x, y]) => !(x >= -5 && x <= 105 && y >= -5 && y <= 105))) errs.push(`${n}'s coordinates must stay within -5..105`);
    const poly = f.points;
    if (poly.length < 3 || Math.abs(areaCentroid(poly).area) < 4) errs.push(`${n} encloses no area`);
  });
  if (typeof eyes !== "string" || !COLOR.test(eyes)) errs.push("eyes must be #RRGGBB");
  return errs;
}

/** The problems with an avatar recipe {shape, color, outline, layers?,
 *  eyes?}, [] when it is one the engine can draw. `outline` counts only
 *  for shape "custom"; `layers` (with `eyes`) make it a rich avatar, which
 *  only a custom outline can be. */
export function validateAvatar({ shape, color, outline, layers, eyes } = {}) {
  const errs = [];
  if (shape !== "custom" && !KIT_SHAPES.includes(shape))
    errs.push(`shape must be one of ${KIT_SHAPES.join(", ")} or custom`);
  if (typeof color !== "string" || !COLOR.test(color)) errs.push("color must be #RRGGBB");
  if (shape === "custom") errs.push(...outlineErrors(outline));
  const rich = (Array.isArray(layers) && layers.length) || eyes !== undefined;
  if (rich) {
    if (shape !== "custom") errs.push("only a custom outline can have layers");
    else errs.push(...layerErrors(layers, eyes));
  }
  return errs;
}

// ---------------------------------------------------------------------------
// The designer: a model proposes 3 candidates, each checked here.

// The system prompt lives in templates/ (beside the bundle when packaged),
// so the Rust hub's /avatar/design reads the very same text.
export const DESIGN_SYSTEM = readFileSync(new URL("./templates/avatar-design-system.txt", import.meta.url), "utf8").replace(/\n$/, "");

/** The user turn's text for a design request. */
export function designRequest({ prompt, name, image, rich = false }) {
  if (rich) return richRequest({ prompt, name, image });
  return [
    name ? `The bot's name: ${name}` : "",
    prompt
      ? `The request (what the bot is or does, or how it should look): ${prompt}`
      : "The request: none. Make the picture's subject into an avatar, in its own main colour, and name each candidate after it.",
    image
      ? "The image attached is the subject. Candidate 1 MUST be that subject made into an avatar: shape \"custom\", an outline tracing the subject's silhouette (simplified to the rules), in #FFFFFF unless the request asks for colour (then the subject's main colour). The request says what to keep or change. Candidates 2 and 3 are other takes: another custom outline or a fitting kit shape."
      : "Candidates may be kit shapes or custom outlines; at least one must be a kit shape. Use a custom outline when the bot has an obvious emblem (a receipt, a leaf, a cat's head).",
  ].filter(Boolean).join("\n");
}

/** The candidates in a model's reply, tidied (not yet validated); [] when
 *  there is no JSON to read. */
export function parseCandidates(text) {
  const s = String(text || "").replace(/```(?:json)?/gi, "");
  const tries = [];
  const a = s.indexOf("{"), b = s.lastIndexOf("}");
  if (a >= 0 && b > a) tries.push(s.slice(a, b + 1));
  const c = s.indexOf("["), d = s.lastIndexOf("]");
  if (c >= 0 && d > c) tries.push(s.slice(c, d + 1));
  let list = null;
  for (const t of tries) {
    try {
      const j = JSON.parse(t);
      list = Array.isArray(j) ? j : Array.isArray(j?.candidates) ? j.candidates : j && typeof j === "object" && j.shape ? [j] : null;
      if (list) break;
    } catch {}
  }
  return (list || []).filter((x) => x && typeof x === "object").map((x) => {
    const outline = typeof x.outline === "string" ? x.outline.trim().replace(/\s+/g, " ") : "";
    let shape = String(x.shape || "").trim().toLowerCase();
    if (!shape && outline) shape = "custom";
    const color = String(x.color || x.colour || "").trim().toUpperCase();
    const layers = Array.isArray(x.layers)
      ? x.layers.filter((l) => l && typeof l === "object").map((l) => ({
          path: typeof l.path === "string" ? l.path.trim().replace(/\s+/g, " ") : "",
          fill: String(l.fill || l.color || "").trim().toUpperCase(),
          ...(typeof l.opacity === "number" ? { opacity: l.opacity } : {}),
        }))
      : null;
    const eyes = typeof x.eyes === "string" ? x.eyes.trim().toUpperCase() : undefined;
    return {
      name: String(x.name || "").trim().slice(0, 40),
      shape,
      color,
      ...(shape === "custom" ? { outline } : {}),
      ...(shape === "custom" && layers ? { layers } : {}),
      ...(shape === "custom" && eyes !== undefined ? { eyes } : {}),
      why: String(x.why || "").trim().slice(0, 240),
    };
  });
}

// The problems with one round of candidates as a set, and each one's.
function check(cands, image, rich = false) {
  const each = cands.map((c) =>
    rich && !(c.layers?.length) ? [...validateAvatar(c), "a rich candidate needs layers and eyes"] : validateAvatar(c),
  );
  const set = [];
  if (cands.length !== 3) set.push(`there must be exactly 3 candidates (got ${cands.length})`);
  if (rich) return { each, set };
  if (image && cands[0]?.shape !== "custom")
    set.push("candidate 1 must be a custom outline tracing the image's subject");
  if (!image && cands.length && !cands.some((c, k) => KIT_SHAPES.includes(c.shape) && !each[k].length))
    set.push("at least one candidate must be a kit shape");
  return { each, set };
}

/** Three candidates for `{prompt, name?, image?}`. `ask(messages)` is the
 *  model call (Anthropic-shaped messages, the system prompt is the
 *  caller's) → the reply's text; `pick(prompt)` → {shape, color}, a kit
 *  choice for when nothing usable comes back. `image` is the user turn's
 *  image block, if any. Candidates that fail the checks are retried once,
 *  with the errors listed, then dropped. */
export async function designCandidates({ prompt, name, image, rich = false }, { ask, pick }) {
  const text = designRequest({ prompt, name, image: !!image, rich });
  const messages = [{ role: "user", content: image ? [image, { type: "text", text }] : text }];
  let reply = await ask(messages);
  let cands = parseCandidates(reply);
  let { each, set } = check(cands, image, rich);
  const firstGood = cands.filter((_, k) => !each[k].length);
  if (set.length || each.some((e) => e.length)) {
    const lines = [
      ...set,
      ...each.flatMap((e, k) => (e.length ? [`candidate ${k + 1} (${cands[k].name || cands[k].shape || "unnamed"}): ${e.join("; ")}`] : [])),
    ];
    if (!cands.length) lines.unshift("the reply had no JSON candidates");
    messages.push(
      { role: "assistant", content: String(reply || "").trim() || "(empty)" },
      { role: "user", content: `These problems need fixing:\n- ${lines.join("\n- ")}\nReply with all 3 candidates again, in the same order, fixed. JSON only.` },
    );
    reply = await ask(messages);
    cands = parseCandidates(reply);
    ({ each } = check(cands, image, rich));
    cands = cands.filter((_, k) => !each[k].length);
    // top up from the first round's good ones
    for (const c of firstGood) {
      if (cands.length >= 3) break;
      if (!cands.some((x) => x.shape === c.shape && x.color === c.color && x.outline === c.outline)) cands.push(c);
    }
  }
  cands = cands.slice(0, 3);
  if (rich) {
    if (!cands.length) throw new Error("no rich design came back that the engine can draw; try again or another model");
    return cands;
  }
  if (!image && cands.length && !cands.some((c) => KIT_SHAPES.includes(c.shape))) {
    const k = await kitFallback(prompt, pick);
    cands = [...cands.slice(0, 2), k];
  }
  if (!cands.length) cands = [await kitFallback(prompt, pick)];
  return cands;
}

async function kitFallback(prompt, pick) {
  // The kit picker's shape, in the primary ink: Design defaults to #FFFFFF.
  const { shape } = await pick(prompt);
  return { name: KIT[shape] ? shape[0].toUpperCase() + shape.slice(1) : "Blob", shape, color: "#FFFFFF", why: "A kit shape picked for the description." };
}

// MOCK mode: fixed candidates, valid outlines.
export const MOCK_OUTLINES = {
  // a pebble: a rounded, slightly flattened ball
  pebble: "M50 16 C73 16 88 31 88 52 C88 73 72 86 50 86 C28 86 12 73 12 52 C12 31 27 16 50 16 Z",
  // a cat's head: two broad ears, round jowls
  cat: "M18 44 L22 14 L42 28 Q50 26 58 28 L78 14 L82 44 C88 66 72 86 50 86 C28 86 12 66 18 44 Z",
  // a receipt: a tall card with a zigzag foot
  receipt: "M26 14 L74 14 L74 80 L66 88 L58 80 L50 88 L42 80 L34 88 L26 80 Z",
};
export function mockCandidates({ image } = {}) {
  const kit = { name: "Blob", shape: "blob", color: "#FFFFFF", why: "A friendly ball in the primary ink." };
  const cat = { name: "Cat Head", shape: "custom", color: "#FFFFFF", outline: MOCK_OUTLINES.cat, why: "A bold cat silhouette." };
  const pebble = { name: "Pebble", shape: "custom", color: "#FFFFFF", outline: MOCK_OUTLINES.pebble, why: "A calm, rounded pebble." };
  const receipt = { name: "Receipt", shape: "custom", color: "#FFFFFF", outline: MOCK_OUTLINES.receipt, why: "A receipt with a torn foot." };
  return image ? [{ ...cat, name: "Image Subject", why: "The image's subject as a silhouette." }, kit, pebble] : [kit, receipt, cat];
}

// --- Rich: a multi-colour mark avatar (a shell, painted layers, eyes) -------

/** The rich designer's extra instructions (appended to DESIGN_SYSTEM). */
export const RICH_SYSTEM = readFileSync(new URL("./templates/avatar-rich-system.txt", import.meta.url), "utf8").replace(/\n$/, "");

/** The user turn's text for a rich design request. */
export function richRequest({ prompt, name, image }) {
  return [
    name ? `The bot's name: ${name}` : "",
    prompt ? `The request: ${prompt}` : "The request: none. Make the picture's subject into a rich avatar.",
    image
      ? "The image attached is the subject. Keep its identity: its main colour on the shell, its hair or head outline, ears or headgear, its face colour, its eye colour."
      : "Design the character the request describes.",
    "Give exactly 3 rich candidates (different takes), each with shape \"custom\", outline, color (the shell), layers and eyes.",
  ].filter(Boolean).join("\n");
}

/** MOCK rich candidates: valid, hand-written. */
export const MOCK_RICH = [
  {
    name: "Green Giant", shape: "custom", color: "#3E8E2F",
    outline: "M20 44 L24 18 L36 26 L42 12 L52 22 L62 10 L66 24 L78 18 L82 44 C88 64 76 88 50 90 C24 88 12 64 20 44 Z",
    layers: [
      { path: "M22 40 Q50 16 80 40 L76 46 Q50 30 26 46 Z", fill: "#1E2A16" },
      { path: "M26 50 C26 36 74 36 76 50 C78 70 66 86 50 86 C34 86 24 70 26 50 Z", fill: "#F5EBD6" },
      { path: "M30 66 C30 62 38 62 38 66 C38 70 30 70 30 66 Z", fill: "#F2C3B5", opacity: 0.6 },
      { path: "M62 66 C62 62 70 62 70 66 C70 70 62 70 62 66 Z", fill: "#F2C3B5", opacity: 0.6 },
    ],
    eyes: "#58D63A", why: "A green shell with a dark fringe and a cream face.",
  },
  {
    name: "Mask Hero", shape: "custom", color: "#D9232E",
    outline: "M50 10 C74 10 86 26 86 46 C86 66 72 84 58 90 Q50 93 42 90 C28 84 14 66 14 46 C14 26 26 10 50 10 Z",
    layers: [
      { path: "M24 40 C30 30 44 32 46 44 C46 54 32 58 26 52 Z", fill: "#111111" },
      { path: "M76 40 C70 30 56 32 54 44 C54 54 68 58 74 52 Z", fill: "#111111" },
    ],
    eyes: "#FFFFFF", why: "A red mask with black eye patches.",
  },
  {
    name: "Ginger Cat", shape: "custom", color: "#F08A24",
    outline: "M18 44 L22 14 L42 28 Q50 26 58 28 L78 14 L82 44 C88 66 72 86 50 86 C28 86 12 66 18 44 Z",
    layers: [{ path: "M34 64 C34 56 66 56 66 64 C66 76 58 82 50 82 C42 82 34 76 34 64 Z", fill: "#FBEBD3" }],
    eyes: "#2B1D12", why: "An orange cat with a cream muzzle.",
  },
];
