// avatars.mjs — bot avatar generation (image model) + osascript helper.
import { execFile, execFileSync } from "node:child_process";
import { existsSync, unlinkSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { createHash } from "node:crypto";
import { BASE_URL, MODEL, PROVIDER } from "./config.mjs";

export const osa = (script) =>
  new Promise((ok, fail) =>
    execFile("osascript", ["-e", script], (err, out) => (err ? fail(err) : ok(out.trim()))),
  );
export const keyOf = (name) => {
  if (process.env[name]) return process.env[name];
  try { return (process.env[name] = execFileSync("secret", ["get", name], { encoding: "utf8" }).trim()); } catch { return ""; }
};
// Renders the avatar with Gemini's image model (falls back to OpenAI's
// gpt-image-1); keys come from the environment or `secret`.
export async function generateAvatarImage(description, out) {
  const prompt = `A square profile avatar image, centered subject filling the frame, clean simple background: ${description}`;
  const gem = keyOf("GEMINI_API_KEY");
  if (gem) {
    const r = await fetch(
      `https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-flash-image:generateContent?key=${gem}`,
      {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({
          contents: [{ parts: [{ text: prompt }] }],
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
      return;
    }
  }
  const oai = keyOf("OPENAI_API_KEY");
  if (oai) {
    const r = await fetch("https://api.openai.com/v1/images/generations", {
      method: "POST",
      headers: { "content-type": "application/json", authorization: `Bearer ${oai}` },
      body: JSON.stringify({ model: "gpt-image-1", size: "1024x1024", quality: "medium", prompt }),
    });
    const j = await r.json();
    if (r.ok && j?.data?.[0]?.b64_json) {
      writeFileSync(out, Buffer.from(j.data[0].b64_json, "base64"));
      return;
    }
  }
  // No image model with quota: Claude illustrates it as SVG, ImageMagick
  // rasterizes it.
  if (!anthropic) throw new Error("no image model available (add GEMINI or OPENAI quota)");
  const m = await anthropic.messages.create({
    model: MODEL,
    max_tokens: 8000,
    messages: [{
      role: "user",
      content: `Draw a polished, detailed flat-illustration profile avatar as a single SVG (viewBox 0 0 512 512, square, full-bleed colored background, the subject centered and filling most of the frame, rich shading with gradients). Subject: ${description}. Reply with only the <svg>…</svg> markup.`,
    }],
  });
  const text = m.content.map((c) => c.text || "").join("");
  const svg = /<svg[\s\S]*<\/svg>/.exec(text)?.[0];
  if (!svg) throw new Error("the illustrator returned no SVG");
  // QuickLook renders SVG through WebKit (gradients, filters intact)
  const svgPath = out.replace(/\.png$/, ".svg");
  writeFileSync(svgPath, svg);
  execFileSync("qlmanage", ["-t", "-s", "1024", "-o", AVATAR_DIR, svgPath], { stdio: "ignore" });
  execFileSync("mv", [svgPath + ".png", out]);
  unlinkSync(svgPath);
}
