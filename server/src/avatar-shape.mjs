// avatar-shape.mjs — what a bot's avatar body may be, and the avatar
// designer's candidate logic. Pure: no config, no store, no model client
// (routes/bots.mjs wires those in), so the unit tests import it as is.
//
// A body is a kit shape (the app's mark engine draws 13 of them) or
// "custom": an SVG outline in a 0 0 100 100 box, y down, drawn with M/L/C/Q/Z
// only. The engine fills it with one colour and adds the eyes itself.

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

/** The problems with an avatar recipe {shape, color, outline}, [] when it
 *  is one the engine can draw. `outline` counts only for shape "custom". */
export function validateAvatar({ shape, color, outline } = {}) {
  const errs = [];
  if (shape !== "custom" && !KIT_SHAPES.includes(shape))
    errs.push(`shape must be one of ${KIT_SHAPES.join(", ")} or custom`);
  if (typeof color !== "string" || !COLOR.test(color)) errs.push("color must be #RRGGBB");
  if (shape === "custom") errs.push(...outlineErrors(outline));
  return errs;
}

// ---------------------------------------------------------------------------
// The designer: a model proposes 3 candidates, each checked here.

export const DESIGN_SYSTEM = `You design avatars for Buwiz Bot, a chat app where every bot has one.

What the app draws: its "mark" engine renders ONE solid silhouette filled with ONE colour, then adds two eyes of its own and animates the mark (it moves, blinks and looks around). So draw only the outer silhouette: no face, no eyes, no mouth, no holes, no lines or details inside the shape. It must read at 24 pixels.

A body is a kit shape or a custom outline.

Kit shapes (shape = the name):
${KIT_SHAPES.map((k) => `- ${k}: ${KIT[k]}`).join("\n")}

Custom outline (shape = "custom", outline = SVG path data):
- A 0 0 100 100 box, x to the right, y DOWN (y = 0 is the top edge).
- Only the commands M, L, C, Q and Z (upper case absolute, lower case relative) and plain decimal numbers. No H, V, S, T or A, no exponents.
- ONE closed shape: a single M at the start, Z at the end, no second subpath, no holes, never crossing itself.
- Centred, filling roughly 55-80% of the box (its longer side spans about 55-80 units), every coordinate within 0..100.
- No part thinner than about 10 units: no thin spikes, stems, tails, whiskers, antennae, legs or handles.
- Every point of the edge visible from the shape's centre: no deep notches or bays that fold back on themselves.
- A simple, recognisable silhouette, like a cookie cutter or a bold app glyph: 4 to 16 segments, smooth C or Q curves for organic forms.
- Keep a roomy, solid area in the upper middle: the eyes go there.

Colour: "#RRGGBB". Saturated, mid-brightness colours that read on both light and dark backgrounds, e.g. #FF263C red, #FF6700 orange, #FF9800 amber, #00C972 green, #00BCA6 teal, #1084FE blue, #9159FE purple, #FF309B pink, #936439 brown, #777777 grey. Avoid near-black, pastel and very light colours. #FFFFFF is special: the adaptive swatch, white on dark backgrounds and black on light ones; use it for a monochrome look.

Reply with JSON only (no prose, no code fences):
{"candidates":[{"name":"…","shape":"…","color":"#RRGGBB","outline":"M … Z","why":"…"}]}
Exactly 3 candidates, each a distinct idea. name: a short label for the design (1-3 words). why: one short sentence on how it fits the bot. Give outline only when shape is "custom".`;

/** The user turn's text for a design request. */
export function designRequest({ prompt, name, image }) {
  return [
    name ? `The bot's name: ${name}` : "",
    `The request (what the bot is or does, or how it should look): ${prompt}`,
    image
      ? "The image attached is the subject. Candidate 1 MUST be that subject made into an avatar: shape \"custom\", an outline tracing the subject's silhouette (simplified to the rules), in the subject's main colour. The request says what to keep or change. Candidates 2 and 3 are other takes: another custom outline or a fitting kit shape."
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
    return {
      name: String(x.name || "").trim().slice(0, 40),
      shape,
      color,
      ...(shape === "custom" ? { outline } : {}),
      why: String(x.why || "").trim().slice(0, 240),
    };
  });
}

// The problems with one round of candidates as a set, and each one's.
function check(cands, image) {
  const each = cands.map((c) => validateAvatar(c));
  const set = [];
  if (cands.length !== 3) set.push(`there must be exactly 3 candidates (got ${cands.length})`);
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
export async function designCandidates({ prompt, name, image }, { ask, pick }) {
  const text = designRequest({ prompt, name, image: !!image });
  const messages = [{ role: "user", content: image ? [image, { type: "text", text }] : text }];
  let reply = await ask(messages);
  let cands = parseCandidates(reply);
  let { each, set } = check(cands, image);
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
    ({ each } = check(cands, image));
    cands = cands.filter((_, k) => !each[k].length);
    // top up from the first round's good ones
    for (const c of firstGood) {
      if (cands.length >= 3) break;
      if (!cands.some((x) => x.shape === c.shape && x.color === c.color && x.outline === c.outline)) cands.push(c);
    }
  }
  cands = cands.slice(0, 3);
  if (!image && cands.length && !cands.some((c) => KIT_SHAPES.includes(c.shape))) {
    const k = await kitFallback(prompt, pick);
    cands = [...cands.slice(0, 2), k];
  }
  if (!cands.length) cands = [await kitFallback(prompt, pick)];
  return cands;
}

async function kitFallback(prompt, pick) {
  const { shape, color } = await pick(prompt);
  return { name: KIT[shape] ? shape[0].toUpperCase() + shape.slice(1) : "Blob", shape, color, why: "A kit shape picked for the description." };
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
  const kit = { name: "Blue Blob", shape: "blob", color: "#1084FE", why: "A friendly ball in the default blue." };
  const cat = { name: "Cat Head", shape: "custom", color: "#FF9800", outline: MOCK_OUTLINES.cat, why: "A bold cat silhouette." };
  const pebble = { name: "Pebble", shape: "custom", color: "#00C972", outline: MOCK_OUTLINES.pebble, why: "A calm, rounded pebble." };
  const receipt = { name: "Receipt", shape: "custom", color: "#9159FE", outline: MOCK_OUTLINES.receipt, why: "A receipt with a torn foot." };
  return image ? [{ ...cat, name: "Image Subject", why: "The image's subject as a silhouette." }, kit, pebble] : [kit, receipt, cat];
}
