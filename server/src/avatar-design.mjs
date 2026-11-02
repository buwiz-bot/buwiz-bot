// avatar-design.mjs — the model side of avatar design: the designer's model
// call (Anthropic or the OpenAI-compatible path), the avatar image it may
// look at, and the old kit picker (/avatar/gen). The candidate logic and
// the checks are avatar-shape.mjs's.
import { execFileSync } from "node:child_process";
import { readFileSync, realpathSync, statSync, unlinkSync, writeFileSync } from "node:fs";
import { isAbsolute, join, relative } from "node:path";
import { fileURLToPath } from "node:url";
import { AVATAR_DIR, AVATAR_MODEL, BASE_URL, MOCK, MODEL, PROVIDER } from "./config.mjs";
import { anthropic } from "./model.mjs";
import { editImage, generateImage, keyOf } from "./avatars.mjs";
import { modelCtx } from "./state.mjs";
import { DESIGN_SYSTEM, KIT_SHAPES, MOCK_RICH, RICH_SYSTEM, SWATCHES, designCandidates, mockCandidates } from "./avatar-shape.mjs";

const textOf = (m) => (m.content || []).filter((b) => b.type === "text").map((b) => b.text).join("");

/** The old /avatar/gen choice: a kit shape and a swatch for a description
 *  (blob, #1084FE when the model's answer is off the lists). */
export async function pickKitAvatar(description) {
  let shape = "blob", color = "#1084FE";
  if (PROVIDER === "anthropic" && anthropic) {
    const m = await anthropic.messages.create({
      model: MODEL,
      max_tokens: 120,
      system:
        "Pick an avatar shape and color for a blob character from a description. " +
        `Reply ONLY with JSON {"shape":<one of ${KIT_SHAPES.join("|")}>,"color":<one of ${SWATCHES.join("|")}>}.`,
      messages: [{ role: "user", content: description }],
    });
    const j = JSON.parse((textOf(m).match(/\{[\s\S]*\}/) || ["{}"])[0]);
    if (KIT_SHAPES.includes(j.shape)) shape = j.shape;
    if (SWATCHES.includes(j.color)) color = j.color;
  }
  return { shape, color };
}

/** The image block for a `file://` URI that /avatar/image/pick or /paste
 *  returned; throws for anything outside the avatar folder. */
/** The checked path of an avatar image URI (from /avatar/image/pick or
 *  /avatar/image/paste, inside AVATAR_DIR, no symlink out), or throws. */
export function avatarImagePath(uri) {
  const refuse = () => {
    throw new Error("image must be an avatar image from /avatar/image/pick or /avatar/image/paste");
  };
  // (a URL parser folds "..", so look for it in the URI as sent)
  if (typeof uri !== "string" || !uri.startsWith("file://") || /\/(\.|%2e){2}(\/|$)/i.test(uri)) refuse();
  let path;
  try {
    path = fileURLToPath(uri);
  } catch {
    refuse();
  }
  const inside = (root, p) => {
    const rel = relative(root, p);
    return !!rel && !rel.startsWith("..") && !isAbsolute(rel);
  };
  if (!inside(AVATAR_DIR, path)) refuse();
  let real;
  try {
    real = realpathSync(path);
  } catch {
    throw new Error("image not found");
  }
  // and where it really is: no symlink out
  if (!inside(realpathSync(AVATAR_DIR), real) || !statSync(real).isFile()) refuse();
  return real;
}

/** An avatar image URI (see avatarImagePath) as a Messages image block. */
export function avatarImageBlock(uri) {
  const real = avatarImagePath(uri);
  let bytes = readFileSync(real);
  // the Messages API takes up to 5 MB an image: a big photo goes down to 1024
  if (bytes.length > 3_500_000) {
    const small = join(AVATAR_DIR, `design-${Date.now().toString(36)}.png`);
    execFileSync("sips", ["-s", "format", "png", "-Z", "1024", real, "--out", small], { stdio: "ignore" });
    bytes = readFileSync(small);
    unlinkSync(small);
  }
  const media_type =
    bytes[0] === 0x89 && bytes[1] === 0x50 ? "image/png"
    : bytes[0] === 0xff && bytes[1] === 0xd8 ? "image/jpeg"
    : bytes.subarray(0, 3).toString("latin1") === "GIF" ? "image/gif"
    : bytes.subarray(8, 12).toString("latin1") === "WEBP" ? "image/webp"
    : null;
  if (!media_type) throw new Error("image is not a PNG, JPEG, GIF or WebP");
  return { type: "image", source: { type: "base64", media_type, data: bytes.toString("base64") } };
}

// chat/completions wants image_url parts instead of base64 image sources
const oaiContent = (c) =>
  typeof c === "string"
    ? c
    : c.map((b) => (b.type === "image" ? { type: "image_url", image_url: { url: `data:${b.source.media_type};base64,${b.source.data}` } } : b));

/** The designer's model call: Anthropic-shaped messages → the reply text.
 *  `model` is the picker's id ("gpt-5.5", "claude-haiku-5-5--fast"),
 *  routed through opencodex as a bot's chosen model is; "" is Design's
 *  default, AVATAR_MODEL (GEN_AVATAR_MODEL), never the chat's. */
export async function askDesigner(messages, model = "", { system = DESIGN_SYSTEM, maxTokens = 6000 } = {}) {
  if (PROVIDER === "anthropic") {
    const create = () => anthropic.messages.create({ model: AVATAR_MODEL, max_tokens: maxTokens, system, messages });
    const fast = model.endsWith("--fast");
    const m = model ? await modelCtx.run({ model: fast ? model.slice(0, -"--fast".length) : model, fast }, create) : await create();
    return textOf(m);
  }
  const r = await fetch(`${BASE_URL}/chat/completions`, {
    method: "POST",
    headers: { Authorization: `Bearer ${process.env.OPENROUTER_API_KEY}`, "Content-Type": "application/json" },
    body: JSON.stringify({
      model: model || AVATAR_MODEL,
      messages: [{ role: "system", content: system }, ...messages.map((m) => ({ ...m, content: oaiContent(m.content) }))],
      max_tokens: maxTokens,
    }),
  });
  if (!r.ok) throw new Error(`llm-api ${r.status}: ${await r.text()}`);
  return (await r.json()).choices?.[0]?.message?.content || "";
}

/** POST /avatar/design's body → its candidates. */
export async function designAvatar({ prompt, image, name, model, style } = {}) {
  const rich = style === "rich";
  const text = typeof prompt === "string" ? prompt.trim() : "";
  // a picture alone is enough: the model makes its subject into an avatar
  if (!text && !image) throw new Error("describe the bot or add a picture");
  const block = image ? avatarImageBlock(image) : null;
  if (MOCK) return rich ? MOCK_RICH.map((c) => ({ ...c })) : mockCandidates({ image: !!block });
  return designCandidates(
    { prompt: text, name: typeof name === "string" ? name.trim() : "", image: block, rich },
    {
      ask: (messages) =>
        askDesigner(messages, typeof model === "string" ? model.trim() : "", rich ? { system: `${DESIGN_SYSTEM}\n\n${RICH_SYSTEM}`, maxTokens: 12000 } : undefined),
      // the fallback never fails the request
      pick: (p) => pickKitAvatar(p).catch(() => ({ shape: "blob", color: "#1084FE" })),
    },
  );
}

// --- Picture mode: a "Bot"-style picture from the image model ---------------

// The style prompt lives in templates/, like Design's system prompt.
export const PICTURE_STYLE = readFileSync(new URL("./templates/avatar-picture-style.txt", import.meta.url), "utf8").replace(/\n$/, "");

/** The image model's instructions: the style, then what the user typed on
 *  top of it (it adds to the style; it never replaces it). With no
 *  picture, the character is the user's description. */
export function pictureRequest({ prompt = "", image = false } = {}) {
  const extra = String(prompt || "").trim();
  const style = image ? PICTURE_STYLE : PICTURE_STYLE.replace(/^Turn the subject of the reference picture into a minimal flat "Bot" avatar\. Use the picture only to recognise who the subject is; do not copy its pose, framing or background\./, "Draw the character described below as a minimal flat \"Bot\" avatar.");
  return [style, extra ? `${image ? "Also, from the user" : "The character"}: ${extra}` : ""].filter(Boolean).join("\n\n");
}

const SVG_RULES =
  'Draw it as one SVG: viewBox="0 0 1024 1024", full-bleed, the background covering every pixel. Flat shapes and simple linear or radial gradients only; no text, no <image>, no filters, no scripts. Reply with only the <svg>…</svg> markup.';

/** POST /avatar/picture {image?, prompt?, model?} → a new picture in
 *  AVATAR_DIR: {uri, name, w, h, via}. The chosen chat model (the model
 *  picker's, else Design's default) draws it as SVG, which QuickLook
 *  rasterizes; when that fails, the image models (the opencodex relay,
 *  then Gemini) try. The picture becomes a photo avatar (the crop
 *  editor's Set avatar), so its colours are its own. */
export async function designPicture({ prompt, image, model } = {}) {
  const text = typeof prompt === "string" ? prompt.trim() : "";
  if (!text && !image) throw new Error("add a picture or describe the character");
  const src = image ? avatarImagePath(image) : null;
  const out = join(AVATAR_DIR, `${Date.now().toString(36)}-bot.png`);
  let via = "mock";
  if (MOCK) {
    // no model: the reference stands in (or a fixed square)
    execFileSync("sips", ["-s", "format", "png", "-Z", "1024", src || MOCK_PICTURE, "--out", out], { stdio: "ignore" });
  } else {
    const ask = pictureRequest({ prompt: text, image: !!src });
    const errs = [];
    try {
      via = await svgPicture(ask, src ? avatarImageBlock(image) : null, typeof model === "string" ? model.trim() : "", out);
    } catch (e) {
      errs.push(`${model || AVATAR_MODEL}: ${e?.message || e}`);
      try {
        via = src ? await editAvatarImage(src, ask, out) : await generateImage(ask, out);
      } catch (e2) {
        errs.push(String(e2?.message || e2));
        throw new Error(`couldn't draw the picture (${errs.join("; ")})`);
      }
    }
  }
  const dims = execFileSync("sips", ["-g", "pixelWidth", "-g", "pixelHeight", out], { encoding: "utf8" });
  return {
    via,
    uri: `file://${out}`,
    name: "Bot picture",
    w: Number(/pixelWidth: (\d+)/.exec(dims)?.[1] || 0),
    h: Number(/pixelHeight: (\d+)/.exec(dims)?.[1] || 0),
  };
}

/** The chat model draws the picture as SVG; QuickLook (WebKit) renders it
 *  to `out`. → the model's id. */
async function svgPicture(ask, block, model, out) {
  const content = [...(block ? [block] : []), { type: "text", text: `${ask}\n\n${SVG_RULES}` }];
  const reply = await askDesigner([{ role: "user", content }], model, {
    system: "You are an illustrator who draws clean vector avatars as SVG.",
    maxTokens: 16000,
  });
  const svg = /<svg[\s\S]*<\/svg>/.exec(reply)?.[0];
  if (!svg) throw new Error("the model returned no SVG");
  if (/<script|<image|<foreignObject|href\s*=\s*["']https?:/i.test(svg)) throw new Error("the SVG used scripts or outside images");
  const svgPath = out.replace(/\.png$/, "") + ".svg";
  writeFileSync(svgPath, svg);
  try {
    execFileSync("qlmanage", ["-t", "-s", "1024", "-o", AVATAR_DIR, svgPath], { stdio: "ignore" });
    execFileSync("mv", [svgPath + ".png", out]);
  } finally {
    try { unlinkSync(svgPath); } catch {}
  }
  return model || AVATAR_MODEL;
}

const MOCK_PICTURE = fileURLToPath(new URL("../test/fixtures/avatar-mock.png", import.meta.url));

/** Edits `src` as `prompt` says into `out`: the opencodex image relay
 *  first, then Gemini's image model when a key is configured. */
async function editAvatarImage(src, prompt, out) {
  const errs = [];
  try {
    return await editImage(src, prompt, out);
  } catch (e) {
    errs.push(`opencodex: ${e?.message || e}`);
  }
  const gem = keyOf("GEMINI_API_KEY");
  if (gem) {
    try {
      const bytes = readFileSync(src);
      const r = await fetch(
        `https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-flash-image:generateContent?key=${gem}`,
        {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({
            contents: [{ parts: [{ text: prompt }, { inlineData: { mimeType: "image/png", data: bytes.toString("base64") } }] }],
            generationConfig: { responseModalities: ["IMAGE"], imageConfig: { aspectRatio: "1:1" } },
          }),
        },
      );
      const j = await r.json();
      const data = j?.candidates?.[0]?.content?.parts?.find((p) => p.inlineData)?.inlineData?.data;
      if (data) {
        writeFileSync(out + ".raw", Buffer.from(data, "base64"));
        execFileSync("sips", ["-s", "format", "png", out + ".raw", "--out", out], { stdio: "ignore" });
        unlinkSync(out + ".raw");
        return "gemini";
      }
      errs.push(`gemini: ${j?.error?.message || "no image"}`);
    } catch (e) {
      errs.push(`gemini: ${e?.message || e}`);
    }
  }
  throw new Error(`no image model could draw it (${errs.join("; ")})`);
}
