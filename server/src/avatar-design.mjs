// avatar-design.mjs — the model side of avatar design: the designer's model
// call (Anthropic or the OpenAI-compatible path), the avatar image it may
// look at, and the old kit picker (/avatar/gen). The candidate logic and
// the checks are avatar-shape.mjs's.
import { execFileSync } from "node:child_process";
import { readFileSync, realpathSync, statSync, unlinkSync } from "node:fs";
import { isAbsolute, join, relative } from "node:path";
import { fileURLToPath } from "node:url";
import { AVATAR_DIR, AVATAR_MODEL, BASE_URL, MOCK, MODEL, PROVIDER } from "./config.mjs";
import { anthropic } from "./model.mjs";
import { modelCtx } from "./state.mjs";
import { DESIGN_SYSTEM, KIT_SHAPES, SWATCHES, designCandidates, mockCandidates } from "./avatar-shape.mjs";

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
export function avatarImageBlock(uri) {
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
export async function askDesigner(messages, model = "") {
  if (PROVIDER === "anthropic") {
    const create = () => anthropic.messages.create({ model: AVATAR_MODEL, max_tokens: 6000, system: DESIGN_SYSTEM, messages });
    const fast = model.endsWith("--fast");
    const m = model ? await modelCtx.run({ model: fast ? model.slice(0, -"--fast".length) : model, fast }, create) : await create();
    return textOf(m);
  }
  const r = await fetch(`${BASE_URL}/chat/completions`, {
    method: "POST",
    headers: { Authorization: `Bearer ${process.env.OPENROUTER_API_KEY}`, "Content-Type": "application/json" },
    body: JSON.stringify({
      model: model || AVATAR_MODEL,
      messages: [{ role: "system", content: DESIGN_SYSTEM }, ...messages.map((m) => ({ ...m, content: oaiContent(m.content) }))],
      max_tokens: 6000,
    }),
  });
  if (!r.ok) throw new Error(`llm-api ${r.status}: ${await r.text()}`);
  return (await r.json()).choices?.[0]?.message?.content || "";
}

/** POST /avatar/design's body → its candidates. */
export async function designAvatar({ prompt, image, name, model } = {}) {
  if (typeof prompt !== "string" || !prompt.trim()) throw new Error("describe the bot or how it should look");
  const block = image ? avatarImageBlock(image) : null;
  if (MOCK) return mockCandidates({ image: !!block });
  return designCandidates(
    { prompt: prompt.trim(), name: typeof name === "string" ? name.trim() : "", image: block },
    {
      ask: (messages) => askDesigner(messages, typeof model === "string" ? model.trim() : ""),
      // the fallback never fails the request
      pick: (p) => pickKitAvatar(p).catch(() => ({ shape: "blob", color: "#1084FE" })),
    },
  );
}
