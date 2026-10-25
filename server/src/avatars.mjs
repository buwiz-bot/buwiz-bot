// avatars.mjs — bot avatar generation (image model) + osascript helper.
import { execFile, execFileSync } from "node:child_process";
import { existsSync, readFileSync, unlinkSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { createHash } from "node:crypto";
import { BASE_URL, MODEL, OPENCODEX, PROVIDER } from "./config.mjs";
import { anthropic } from "./model.mjs";

export const osa = (script) =>
  new Promise((ok, fail) =>
    execFile("osascript", ["-e", script], (err, out) => (err ? fail(err) : ok(out.trim()))),
  );
export const keyOf = (name) => {
  if (process.env[name]) return process.env[name];
  try { return (process.env[name] = execFileSync("secret", ["get", name], { encoding: "utf8" }).trim()); } catch { return ""; }
};
// Renders an image with Gemini's image model (falls back to OpenAI's
// gpt-image-1, then a Claude-drawn SVG rasterized by QuickLook); keys come
// from the environment or `secret`. Returns the backend that produced it.
// Which chat models may draw images. opencodex reports output modalities
// per model, and its /v1/images relay runs on the ChatGPT (OpenAI) login,
// so every OpenAI-family model reaches an image model through it.
let drawers = { at: 0, ids: new Map() };
export async function imageModels() {
  if (Date.now() - drawers.at < 60_000) return drawers.ids;
  try {
    const j = await (await fetch(`${OPENCODEX}/v1/models`)).json();
    drawers = {
      at: Date.now(),
      ids: new Map(j.data.map((m) => [
        m.id.replace(/--fast$/, ""),
        (m.capabilities?.output_modalities || []).includes("image") || m.owned_by === "openai",
      ])),
    };
  } catch {}
  return drawers.ids;
}
export async function imageCapable(model) {
  return !!(await imageModels()).get(String(model || "").replace(/--fast$/, ""));
}

// Edits the image at `src` as `prompt` says (gpt-image on the ChatGPT
// login, through opencodex's relay) into `out` — the rest of the picture
// stays as it was. → "chatgpt", or throws.
export async function editImage(src, prompt, out, { size = "1024x1024" } = {}) {
  const r = await fetch(`${OPENCODEX}/v1/images/edits`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      prompt,
      size,
      images: [{ image_url: `data:image/png;base64,${readFileSync(src).toString("base64")}` }],
    }),
  });
  const j = await r.json().catch(() => ({}));
  const data = j?.data?.[0]?.b64_json;
  if (!data) throw new Error(`image edit failed: ${j?.error?.message || r.status}`);
  writeFileSync(out + ".raw", Buffer.from(data, "base64"));
  execFileSync("sips", ["-s", "format", "png", out + ".raw", "--out", out], { stdio: "ignore" });
  unlinkSync(out + ".raw");
  return "chatgpt";
}

export async function generateImage(prompt, out, { svgHint, aspect = "1:1", size = "1024x1024", background } = {}) {
  const errs = [];
  // opencodex's images relay (gpt-image on the ChatGPT login) — no API
  // credits needed.
  try {
    const r = await fetch(`${OPENCODEX}/v1/images/generations`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ prompt, size, n: 1, ...(background ? { background } : {}) }),
    });
    const j = await r.json();
    const data = j?.data?.[0]?.b64_json;
    if (data) {
      writeFileSync(out + ".raw", Buffer.from(data, "base64"));
      execFileSync("sips", ["-s", "format", "png", out + ".raw", "--out", out], { stdio: "ignore" });
      unlinkSync(out + ".raw");
      return "chatgpt";
    }
    errs.push(`opencodex: ${j?.error?.message || r.status}`);
  } catch (e) { errs.push(`opencodex: ${e?.message || e}`); }
  const gem = keyOf("GEMINI_API_KEY");
  if (gem) {
    try {
      const r = await fetch(
        `https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-flash-image:generateContent?key=${gem}`,
        {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({
            contents: [{ parts: [{ text: prompt }] }],
            generationConfig: { responseModalities: ["IMAGE"], imageConfig: { aspectRatio: aspect } },
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
    } catch (e) { errs.push(`gemini: ${e?.message || e}`); }
  }
  const oai = keyOf("OPENAI_API_KEY");
  if (oai) {
    try {
      const r = await fetch("https://api.openai.com/v1/images/generations", {
        method: "POST",
        headers: { "content-type": "application/json", authorization: `Bearer ${oai}` },
        body: JSON.stringify({ model: "gpt-image-1", size, quality: "medium", prompt }),
      });
      const j = await r.json();
      if (r.ok && j?.data?.[0]?.b64_json) {
        writeFileSync(out, Buffer.from(j.data[0].b64_json, "base64"));
        return "openai";
      }
      errs.push(`openai: ${j?.error?.message || r.status}`);
    } catch (e) { errs.push(`openai: ${e?.message || e}`); }
  }
  // No image model with quota: Claude illustrates it as SVG, QuickLook
  // rasterizes it.
  if (!anthropic)
    throw new Error(`no image model available (add GEMINI or OPENAI quota)${errs.length ? ` — ${errs.join("; ")}` : ""}`);
  if (errs.length)
    console.log(`  image: falling back to Claude SVG (${errs.map((e) => e.split("\n")[0].slice(0, 160)).join("; ")})`);
  const m = await anthropic.messages.create({
    model: MODEL,
    max_tokens: 8000,
    messages: [{
      role: "user",
      content: `${svgHint || `Draw this as a single polished SVG illustration (viewBox 0 0 512 512, full-bleed, rich gradients): ${prompt}.`} Reply with only the <svg>…</svg> markup.`,
    }],
  });
  const text = m.content.map((c) => c.text || "").join("");
  const svg = /<svg[\s\S]*<\/svg>/.exec(text)?.[0];
  if (!svg) throw new Error("the illustrator returned no SVG");
  // QuickLook renders SVG through WebKit (gradients, filters intact)
  const svgPath = out.replace(/\.png$/, "") + ".svg";
  writeFileSync(svgPath, svg);
  execFileSync("qlmanage", ["-t", "-s", "1024", "-o", dirname(svgPath), svgPath], { stdio: "ignore" });
  execFileSync("mv", [svgPath + ".png", out]);
  unlinkSync(svgPath);
  return "claude-svg";
}

export async function generateAvatarImage(description, out) {
  return generateImage(
    `A square profile avatar image, centered subject filling the frame, clean simple background: ${description}`,
    out,
    {
      svgHint: `Draw a polished, detailed flat-illustration profile avatar as a single SVG (viewBox 0 0 512 512, square, full-bleed colored background, the subject centered and filling most of the frame, rich shading with gradients). Subject: ${description}.`,
    },
  );
}
