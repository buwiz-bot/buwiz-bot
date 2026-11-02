// routes/bots.mjs — extracted from app-server.mjs
import { execFile, execFileSync } from "node:child_process";
import { copyFileSync, existsSync, statSync, unlinkSync } from "node:fs";
import { join } from "node:path";
import { GEN_DIR, AVATAR_DIR, ATTACH_DIR } from "../config.mjs";
import { feedVariant, imageSize, isAnimated } from "../files.mjs";
import { uuidv7, nextThreadSeq, db, store, persist, threadSeq, turnsJson, writeIndex, botDefaults } from "../state.mjs";
import { integrationAdd, integrationRemove, integrationsOf, memoryAdd, memoryRemove, memoryUpdate, setBotProfile } from "../botctx.mjs";
import { osa, generateAvatarImage } from "../avatars.mjs";
import { KIT_SHAPES, validateAvatar } from "../avatar-shape.mjs";
import { designAvatar, designPicture, pickKitAvatar } from "../avatar-design.mjs";

// Sets a request's avatar fields on `bot` (a new one or one there): a kit
// shape clears the outline, an outline without a shape makes it "custom".
// Only the fields given are checked (a bot's older colour stays as it is).
function applyAvatar(bot, { shape, color, outline }) {
  const hasOutline = outline !== undefined && outline !== null && outline !== "";
  const s = shape || (hasOutline ? "custom" : bot.shape);
  const c = color || bot.color;
  const o = s === "custom" ? (hasOutline ? (typeof outline === "string" ? outline.trim() : outline) : bot.outline) : undefined;
  const errs = validateAvatar({
    shape: shape || hasOutline || s === "custom" ? s : KIT_SHAPES[0],
    color: color ? c : "#1084FE",
    outline: o,
  });
  if (errs.length) throw new Error(errs.join("; "));
  bot.shape = s;
  bot.color = c;
  if (o) bot.outline = o;
  else delete bot.outline;
}

export default async function handle(req, res) {
  // A bot is a named agent persona (blob avatar: shape + color) that owns
  // one thread — Grok Bot's model.
  if (req.method === "POST" && req.url === "/bot/new") {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      try {
        const { name, shape, color, outline } = JSON.parse(body || "{}");
        if (!name?.trim()) throw new Error("bot needs a name");
        const av = { shape: "blob", color: "#1084FE" };
        applyAvatar(av, { shape, color, outline });
        const tid = `t${nextThreadSeq()}`;
        const id = `b${Date.now().toString(36)}${Math.floor(Math.random() * 46656).toString(36)}`;
        store.list.unshift({ id: tid, uid: uuidv7(), title: name.trim(), turns: [] });
        // its own context starts seeded (description, instructions) and
        // empty (memories, integrations) — everything else fills on demand
        const bot = {
          id,
          name: name.trim(),
          shape: av.shape,
          color: av.color,
          ...(av.outline ? { outline: av.outline } : {}),
          threadId: tid,
          ...botDefaults(name.trim()),
          memories: [],
          integrations: [],
        };
        store.bots.push(bot);
        store.active = tid;
        writeIndex();
        persist();
        res.setHeader("content-type", "application/json");
        res.end(JSON.stringify({ ok: true, bot, turns: turnsJson() }));
      } catch (e) {
        res.statusCode = 400;
        res.end(JSON.stringify({ ok: false, error: String(e?.message || e) }));
      }
    });
    return true;
  }
  if (req.method === "POST" && req.url === "/bot/update") {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      try {
        const { id, name, shape, color, outline, pinned, section, hidden, image, crop, model, effort, fast, label, description, instructions } = JSON.parse(body || "{}");
        const bot = store.bots.find((b) => b.id === id);
        if (!bot) throw new Error(`unknown bot ${id}`);
        if (shape || color || outline !== undefined) applyAvatar(bot, { shape, color, outline });
        if (name?.trim() && name.trim() !== bot.name) {
          // untouched defaults follow a rename; edited ones stay
          const was = botDefaults(bot.name), now = botDefaults(name.trim());
          if (bot.description === was.description) bot.description = now.description;
          if (bot.instructions === was.instructions) bot.instructions = now.instructions;
          bot.name = name.trim();
        }
        if (pinned !== undefined) bot.pinned = !!pinned;
        if (hidden !== undefined) bot.hidden = !!hidden;
        if (image !== undefined) bot.image = String(image);
        if (crop !== undefined) bot.crop = crop;
        if (model !== undefined) bot.model = String(model);
        if (effort !== undefined) bot.effort = String(effort);
        if (fast !== undefined) bot.fast = !!fast;
        // Grok Bot's "Add a label": a short tagline under the name.
        setBotProfile(bot, { label, description, instructions });
        if (section !== undefined) {
          bot.section = String(section).trim();
          store.sections ??= [];
          if (bot.section && !store.sections.includes(bot.section)) store.sections.push(bot.section);
        }
        const thread = store.list.find((t) => t.id === bot.threadId);
        if (thread) thread.title = bot.name;
        persist();
        res.setHeader("content-type", "application/json");
        res.end(JSON.stringify({ ok: true }));
      } catch (e) {
        res.statusCode = 400;
        res.end(JSON.stringify({ ok: false, error: String(e?.message || e) }));
      }
    });
    return true;
  }
  // A bot's memories: {id, op:"add", text} | {id, op:"update", memoryId,
  // text} | {id, op:"remove", memoryId}.
  if (req.method === "POST" && req.url === "/bot/memory") {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      res.setHeader("content-type", "application/json");
      try {
        const { id, op, text, memoryId } = JSON.parse(body || "{}");
        const bot = store.bots.find((b) => b.id === id);
        if (!bot) throw new Error(`unknown bot ${id}`);
        if (op === "add") memoryAdd(bot, text);
        else if (op === "update") memoryUpdate(bot, String(memoryId || ""), text);
        else if (op === "remove") memoryRemove(bot, String(memoryId || ""));
        else throw new Error(`unknown op ${op}`);
        res.end(JSON.stringify({ ok: true, memories: bot.memories }));
      } catch (e) {
        res.statusCode = 400;
        res.end(JSON.stringify({ ok: false, error: String(e?.message || e) }));
      }
    });
    return true;
  }
  // A bot's integrations: {id, op:"add"|"remove", integration} — integration
  // is an mcp server id, plugin name or skill id ("kind:id" or {id,kind}
  // to disambiguate).
  if (req.method === "POST" && req.url === "/bot/integration") {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      res.setHeader("content-type", "application/json");
      try {
        const { id, op, integration } = JSON.parse(body || "{}");
        const bot = store.bots.find((b) => b.id === id);
        if (!bot) throw new Error(`unknown bot ${id}`);
        if (op === "add") integrationAdd(bot, integration, "user");
        else if (op === "remove") integrationRemove(bot, integration);
        else throw new Error(`unknown op ${op}`);
        res.end(JSON.stringify({ ok: true, integrations: integrationsOf(bot) }));
      } catch (e) {
        res.statusCode = 400;
        res.end(JSON.stringify({ ok: false, error: String(e?.message || e) }));
      }
    });
    return true;
  }
  // Deleting a bot removes its thread AND the artifacts its turns point at:
  // the generated module, its /state rows, its apps meta. Published Mini
  // Apps survive — their live copy is widgets/<name>.tsx, untouched here.
  if (req.method === "POST" && req.url === "/bot/delete") {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      try {
        const { id } = JSON.parse(body || "{}");
        const bot = store.bots.find((b) => b.id === id);
        if (!bot) throw new Error(`unknown bot ${id}`);
        const thread = store.list.find((t) => t.id === bot.threadId);
        const published = new Set(
          db.prepare("SELECT name FROM widgets").all().map((w) => w.name),
        );
        for (const u of thread?.turns || []) {
          const name = u.role === "ai" ? u.name : "";
          if (!/^R\d+$/.test(name || "") || published.has(name)) continue;
          try {
            unlinkSync(join(GEN_DIR, `${name}.tsx`));
          } catch {}
          db.prepare("DELETE FROM kv WHERE name=?").run(name);
          db.prepare("DELETE FROM apps WHERE name=?").run(name);
        }
        store.bots = store.bots.filter((b) => b.id !== id);
        store.list = store.list.filter((t) => t.id !== bot.threadId);
        // never leave the app threadless — /threads dereferences one
        if (!store.list.length)
          store.list.push({ id: `t${nextThreadSeq()}`, uid: uuidv7(), title: "new chat", turns: [] });
        if (store.active === bot.threadId) store.active = store.list[0].id;
        writeIndex();
        res.setHeader("content-type", "application/json");
        res.end(JSON.stringify({ ok: true }));
      } catch (e) {
        res.statusCode = 400;
        res.end(JSON.stringify({ ok: false, error: String(e?.message || e) }));
      }
    });
    return true;
  }
  // Composer attachments (Grok's + menu): the native picker, several files;
  // images are normalized to PNG for the model, documents kept as they are.
  if (req.method === "POST" && req.url === "/attach/pick") {
    res.setHeader("content-type", "application/json");
    osa('set fs to choose file with prompt "Attach photos & files" with multiple selections allowed\nset out to ""\nrepeat with f in fs\nset out to out & POSIX path of f & linefeed\nend repeat\nreturn out').then(async (list) => {
      // Staging a file is what the composer waits on, so it stays cheap:
      // PNG/JPEG/GIF/WebP are copied as they are (only HEIC/TIFF/BMP, which
      // the model can't read, become PNG — a JPEG turned PNG was ~6x the
      // bytes), there is no level-9 re-deflate (~1s a screenshot, ~6s a
      // photo), and the thumbnail and feed run as child processes, every
      // file at once, so the server's loop stays free meanwhile.
      const run = (cmd, args, timeout = 20000) =>
        new Promise((ok) => execFile(cmd, args, { timeout }, (err) => ok(!err)));
      const files = await Promise.all(list.split("\n").filter(Boolean).map(async (src) => {
        const name = src.split("/").pop();
        const ext = (name.match(/\.([^.]+)$/)?.[1] || "").toLowerCase();
        const image = /^(png|jpe?g|gif|heic|webp|tiff?|bmp)$/.test(ext);
        const video = /^(mov|mp4|m4v|webm|avi|mkv)$/.test(ext);
        const keep = /^(png|jpe?g|gif|webp)$/.test(ext);
        const id = `${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 6)}`;
        const out = join(ATTACH_DIR, image ? `${id}.${keep ? ext : "png"}` : `${id}-${name}`);
        if (image && !keep) await run("sips", ["-s", "format", "png", src, "--out", out]);
        else copyFileSync(src, out);
        const animated = image && isAnimated(out, ext);
        let thumb = "", feed = "";
        if (image) {
          // the feed keeps the file's format (a photo stays JPEG), and
          // counts only when smaller — or as a static first frame of motion
          const fext = keep && ext !== "gif" ? ext : "png";
          const t = join(ATTACH_DIR, `${id}.thumb.png`);
          const f = join(ATTACH_DIR, `${id}.feed.${fext}`);
          const feedArgs = fext === "png" ? ["-s", "format", "png", "-Z", "1120", out, "--out", f] : ["-Z", "1120", out, "--out", f];
          const [okT, okF] = await Promise.all([
            run("sips", ["-s", "format", "png", "-Z", "256", out, "--out", t]),
            run("sips", feedArgs).then((ok) => ok || run("magick", [`${out}[0]`, f])),
          ]);
          if (okT && existsSync(t)) thumb = t;
          if (okF && existsSync(f) && (animated || statSync(f).size < statSync(out).size)) feed = f;
          else if (existsSync(f)) unlinkSync(f);
        } else if (video) {
          if ((await run("qlmanage", ["-t", "-s", "256", "-o", ATTACH_DIR, out], 15000)) && existsSync(`${out}.png`)) thumb = `${out}.png`;
        }
        const dims = image ? imageSize(out) : video && thumb ? imageSize(thumb) : {};
        return {
          uri: `file://${out}`,
          thumb: thumb ? `file://${thumb}` : "",
          feed: feed ? `file://${feed}` : "",
          name,
          ext,
          kind: image ? "image" : video ? "video" : "file",
          size: existsSync(out) ? statSync(out).size : 0,
          ...(animated ? { animated: true } : {}),
          ...dims,
        };
      }));
      res.end(JSON.stringify({ ok: true, files }));
    })
      .catch(() => res.end(JSON.stringify({ ok: false, cancelled: true, files: [] })));
    return true;
  }
  if (req.method === "POST" && req.url.startsWith("/avatar/image/")) {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", async () => {
      res.setHeader("content-type", "application/json");
      try {
        const kind = req.url.slice("/avatar/image/".length);
        const out = join(AVATAR_DIR, `${Date.now().toString(36)}.png`);
        let name = "";
        if (kind === "gen") {
          const { description } = JSON.parse(body || "{}");
          if (!description?.trim()) throw new Error("describe the avatar");
          await generateAvatarImage(description.trim(), out);
          name = description.trim();
        } else if (kind === "pick") {
          const picked = await osa(
            'POSIX path of (choose file with prompt "Choose an avatar image" of type {"public.image"})',
          ).catch(() => "");
          if (!picked) return res.end(JSON.stringify({ ok: false, cancelled: true }));
          name = picked.split("/").pop();
          execFileSync("sips", ["-s", "format", "png", picked, "--out", out], { stdio: "ignore" });
        } else if (kind === "paste") {
          await osa(
            `set f to open for access POSIX file "${out}" with write permission
             write (the clipboard as «class PNGf») to f
             close access f`,
          );
          name = "Pasted image";
        } else throw new Error(`unknown avatar source ${kind}`);
        const dims = execFileSync("sips", ["-g", "pixelWidth", "-g", "pixelHeight", out], { encoding: "utf8" });
        const w = Number(/pixelWidth: (\d+)/.exec(dims)?.[1] || 0);
        const h = Number(/pixelHeight: (\d+)/.exec(dims)?.[1] || 0);
        res.end(JSON.stringify({ ok: true, uri: `file://${out}`, name, w, h }));
      } catch (e) {
        res.statusCode = 400;
        res.end(JSON.stringify({ ok: false, error: String(e?.message || e) }));
      }
    });
    return true;
  }
  // the old picker: one kit shape and swatch for a description
  if (req.method === "POST" && req.url === "/avatar/gen") {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", async () => {
      try {
        const { description } = JSON.parse(body || "{}");
        if (!description?.trim()) throw new Error("describe the avatar");
        const { shape, color } = await pickKitAvatar(description.trim());
        res.setHeader("content-type", "application/json");
        res.end(JSON.stringify({ ok: true, shape, color }));
      } catch (e) {
        res.statusCode = 400;
        res.end(JSON.stringify({ ok: false, error: String(e?.message || e) }));
      }
    });
    return true;
  }
  // The designer: {prompt, image?, name?, model?} → 3 candidates {name, shape,
  // color, outline?, why}. `image` is a file:// URI /avatar/image/pick or
  // /paste returned; with one, candidate 1 traces its subject.
  if (req.method === "POST" && req.url === "/avatar/design") {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", async () => {
      res.setHeader("content-type", "application/json");
      try {
        const { prompt, image, name, model } = JSON.parse(body || "{}");
        const candidates = await designAvatar({ prompt, image, name, model });
        res.end(JSON.stringify({ ok: true, candidates }));
      } catch (e) {
        res.statusCode = 400;
        res.end(JSON.stringify({ ok: false, error: String(e?.message || e) }));
      }
    });
    return true;
  }
  // POST /avatar/picture {image?, prompt?, model?} — Picture mode: a
  // "Bot"-style picture (the style prompt, plus what the user typed on
  // top), drawn by the picked model → {ok, uri, name, w, h, via}, the crop
  // editor's draft.
  if (req.method === "POST" && req.url === "/avatar/picture") {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", async () => {
      res.setHeader("content-type", "application/json");
      try {
        const { prompt, image, model } = JSON.parse(body || "{}");
        res.end(JSON.stringify({ ok: true, ...(await designPicture({ prompt, image, model })) }));
      } catch (e) {
        res.statusCode = 400;
        res.end(JSON.stringify({ ok: false, error: String(e?.message || e) }));
      }
    });
    return true;
  }
  return false;
}
