// routes/chat.mjs — extracted from app-server.mjs
import { execFile, execFileSync } from "node:child_process";
import { existsSync, readFileSync, statSync, unlinkSync } from "node:fs";
import { join } from "node:path";
import { GEN_DIR, MODEL, MOCK, WIDGETS_DIR } from "../config.mjs";
import { nextThreadSeq, modelCtx, jobs, allocJobId, setJob, jobJson, db, store, seq, threadSeq, activeThread, turnsJson, writeIndex } from "../state.mjs";
import { choiceFor } from "../model.mjs";
import { headOf, versionsOf } from "../pipeline.mjs";
import { artifactBlocks, generate } from "../agent.mjs";

export default async function handle(req, res) {
  if (req.method === "POST" && req.url === "/chat") {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", async () => {
      try {
        const { prompt: rawPrompt, reply, attachments } = JSON.parse(body || "{}");
        const prompt = rawPrompt || (attachments?.length ? "" : null);
        if (prompt == null) throw new Error("missing prompt");
        const job = {
          id: allocJobId(),
          thread: store.active,
          seq: 0,
          phase: "queued",
          detail: "",
          done: false,
          result: null,
          extra: {
            ...(reply?.text ? { reply: { i: reply.i, text: String(reply.text).slice(0, 400), ...(reply.files?.length ? { files: reply.files.slice(0, 8).map(String) } : {}) } } : {}),
            ...(attachments?.length ? { attachments } : {}),
          },
        };
        jobs.set(job.id, job);
        if (jobs.size > 25) jobs.delete(jobs.keys().next().value);
        res.setHeader("content-type", "application/json");
        res.end(JSON.stringify({ ok: true, id: job.id }));
        // a reply on an artifact: read the file(s) into the model's context first
        artifactBlocks(job.extra.reply?.files)
          .then((ctx) => {
            job.ctx = ctx;
            return modelCtx.run(choiceFor(job.thread), () => generate(job, prompt)); // files land after the response; nsh dev hot-patches
          })
          .catch((e) => {
            console.error(`✗ job ${job.id} crashed:`, e);
            job.done = true;
            setJob(job, "done", `generator error — ${e.message}`);
          });
      } catch (e) {
        res.statusCode = 400;
        res.end(JSON.stringify({ ok: false, error: String(e?.message || e) }));
      }
    });
    return true;
  }
  if (req.method === "GET" && req.url === "/threads") {
    res.setHeader("content-type", "application/json");
    return res.end(
      JSON.stringify({
        active: activeThread().id,
        model: MOCK ? "mock" : MODEL,
        list: store.list.map((t) => {
          const bot = store.bots.find((b) => b.threadId === t.id);
          return {
            id: t.id,
            title: t.title,
            count: t.turns.length,
            hidden: !!t.hidden,
            archived: !!t.archived,
            lastApp: [...t.turns].reverse().find((x) => x.name)?.name || "",
            bot: bot
              ? { id: bot.id, name: bot.name, shape: bot.shape, color: bot.color }
              : null,
          };
        }),
        sections: store.sections || [],
        bots: store.bots.map((b) => {
          const t = store.list.find((x) => x.id === b.threadId);
          // the subtitle / hover popover: the latest message, flattened
          const last = t && [...t.turns].reverse().find((x) => x.text || x.caption);
          return {
            id: b.id,
            name: b.name,
            shape: b.shape,
            color: b.color,
            threadId: b.threadId,
            pinned: !!b.pinned,
            section: b.section || "",
            hidden: !!b.hidden,
            image: b.image || "",
            crop: b.crop || null,
            model: b.model || "",
            effort: b.effort || "",
            fast: !!b.fast,
            preview: String(last?.text || last?.caption || "").replace(/[#*`>_]/g, "").replace(/\s+/g, " ").trim().slice(0, 160),
          };
        }),
      }),
    );
  }
  if (req.method === "POST" && req.url === "/thread/new") {
    const id = `t${nextThreadSeq()}`;
    store.list.unshift({ id, title: "new chat", turns: [] });
    store.active = id;
    writeIndex();
    res.setHeader("content-type", "application/json");
    return res.end(JSON.stringify({ ok: true, id }));
  }
  if (req.method === "GET" && req.url === "/turns") {
    res.setHeader("content-type", "application/json");
    return res.end(
      JSON.stringify({ active: activeThread().id, turns: turnsJson() }),
    );
  }
  // Chat-row context menu state: hide/show and archive/unarchive are plain
  // flags on the thread (rename rides the same patch like /bot/update).
  if (req.method === "POST" && req.url === "/thread/update") {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      try {
        const { id, title, hidden, archived } = JSON.parse(body || "{}");
        const thread = store.list.find((t) => t.id === id);
        if (!thread) throw new Error(`unknown thread ${id}`);
        if (title?.trim()) thread.title = title.trim();
        if (hidden !== undefined) thread.hidden = !!hidden;
        if (archived !== undefined) thread.archived = !!archived;
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
  // Deleting a chat drops its thread and sweeps the un-published modules
  // its turns pointed at — same rule /bot/delete applies to a bot's thread.
  // A bot that owned the thread keeps its persona, just loses the chat.
  if (req.method === "POST" && req.url === "/thread/delete") {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      try {
        const { id } = JSON.parse(body || "{}");
        const thread = store.list.find((t) => t.id === id);
        if (!thread) throw new Error(`unknown thread ${id}`);
        const published = new Set(
          db.prepare("SELECT name FROM widgets").all().map((w) => w.name),
        );
        for (const u of thread.turns || []) {
          const name = u.role === "ai" ? u.name : "";
          if (!/^R\d+$/.test(name || "") || published.has(name)) continue;
          try {
            unlinkSync(join(GEN_DIR, `${name}.tsx`));
          } catch {}
          db.prepare("DELETE FROM kv WHERE name=?").run(name);
          db.prepare("DELETE FROM apps WHERE name=?").run(name);
        }
        for (const b of store.bots) if (b.threadId === id) b.threadId = null;
        store.list = store.list.filter((t) => t.id !== id);
        // never leave the app threadless — /threads dereferences one
        if (!store.list.length)
          store.list.push({ id: `t${nextThreadSeq()}`, title: "new chat", turns: [] });
        if (store.active === id) store.active = store.list[0].id;
        writeIndex();
        res.setHeader("content-type", "application/json");
        res.end(JSON.stringify({ ok: true, active: store.active }));
      } catch (e) {
        res.statusCode = 400;
        res.end(JSON.stringify({ ok: false, error: String(e?.message || e) }));
      }
    });
    return true;
  }
  if (req.method === "POST" && req.url === "/thread/select") {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      try {
        const { id } = JSON.parse(body || "{}");
        const thread = store.list.find((t) => t.id === id);
        if (!thread) throw new Error(`unknown thread ${id}`);
        store.active = id;
        writeIndex();
        res.setHeader("content-type", "application/json");
        res.end(JSON.stringify({ ok: true, turns: turnsJson(thread) }));
      } catch (e) {
        res.statusCode = 400;
        res.end(JSON.stringify({ ok: false, error: String(e?.message || e) }));
      }
    });
    return true;
  }
  if (req.method === "GET" && req.url.startsWith("/status")) {
    const id = Number(new URL(req.url, "http://x").searchParams.get("id"));
    const job = jobs.get(id);
    res.setHeader("content-type", "application/json");
    return res.end(
      jobJson(job || { id, seq: 0, phase: "unknown", detail: "", done: true }),
    );
  }
  if (req.method === "POST" && req.url === "/copy") {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      try {
        const { text } = JSON.parse(body || "{}");
        execFileSync("pbcopy", [], { input: String(text || "") });
        res.setHeader("content-type", "application/json");
        res.end(JSON.stringify({ ok: true }));
      } catch (e) {
        res.statusCode = 400;
        res.end(JSON.stringify({ ok: false, error: String(e?.message || e) }));
      }
    });
    return true;
  }
  if (req.method === "POST" && req.url === "/react") {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      try {
        const { name, key, r } = JSON.parse(body || "{}");
        if (typeof key === "string" && key.startsWith("idx:")) {
          const turn = activeThread().turns[Number(key.slice(4))];
          if (turn) turn.reaction = r || undefined;
        } else {
          for (const t of store.list)
            for (const turn of t.turns)
              if (turn.role === "ai" && turn.name === name)
                turn.reaction = r || undefined;
        }
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
  // POST /turn/delete {key} — removes a conversation exchange: the AI turn
  // and the user prompt that produced it. key is "name:<R#>" (module cards)
  // or "idx:<n>" (text replies — index into the active thread). Only the
  // conversation changes: the module file stays on disk and a published
  // mini-app stays in Mini Apps until removed there.
  if (req.method === "POST" && req.url === "/turn/delete") {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      try {
        const { key } = JSON.parse(body || "{}");
        const thread = activeThread();
        let i = -1;
        if (typeof key === "string" && key.startsWith("name:")) {
          const name = key.slice(5);
          i = thread.turns.findIndex(
            (t) => t.role === "ai" && t.name === name,
          );
        } else if (typeof key === "string" && key.startsWith("idx:")) {
          const n = Number(key.slice(4));
          if (thread.turns[n]) i = n;
        }
        if (i < 0) throw new Error("no such turn");
        const drop = [i];
        // name: keys delete the exchange (card + the prompt that made it);
        // idx: keys delete just that one message, either role.
        if (typeof key === "string" && key.startsWith("name:")) {
          const prev = thread.turns[i - 1];
          if (prev?.role === "user") drop.unshift(i - 1);
        }
        thread.turns = thread.turns.filter((_, j) => !drop.includes(j));
        writeIndex();
        res.setHeader("content-type", "application/json");
        res.end(JSON.stringify({ ok: true, dropped: drop.length }));
      } catch (e) {
        res.statusCode = 400;
        res.end(JSON.stringify({ ok: false, error: String(e?.message || e) }));
      }
    });
    return true;
  }
  if (req.method === "POST" && req.url === "/retry") {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      try {
        const { name } = JSON.parse(body || "{}");
        let thread, prompt;
        for (const t of store.list) {
          // Prefer the published turn — reusing a widget mounts it in the
          // thread too, and retrying must patch widgets/<name>, not a stale
          // generated copy.
          const i =
            t.turns.findIndex(
              (x) => x.role === "ai" && x.name === name && x.widget,
            ) ??
            -1;
          const idx =
            i >= 0
              ? i
              : t.turns.findIndex((x) => x.role === "ai" && x.name === name);
          if (idx < 0) continue;
          thread = t;
          for (let m = idx - 1; m >= 0; m--)
            if (t.turns[m].role === "user") {
              prompt = t.turns[m].text;
              break;
            }
        }
        if (!thread || !prompt) throw new Error(`no prompt found for ${name}`);
        const job = {
          id: allocJobId(),
          thread: thread.id,
          retry: name,
          seq: 0,
          phase: "queued",
          detail: "",
          done: false,
          result: null,
        };
        jobs.set(job.id, job);
        res.setHeader("content-type", "application/json");
        res.end(JSON.stringify({ ok: true, id: job.id }));
        generate(job, prompt).catch((e) => {
          console.error(`✗ job ${job.id} crashed:`, e);
          job.done = true;
          setJob(job, "done", `generator error — ${e.message}`);
        });
      } catch (e) {
        res.statusCode = 400;
        res.end(JSON.stringify({ ok: false, error: String(e?.message || e) }));
      }
    });
    return true;
  }
  if (req.method === "POST" && req.url === "/share") {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      try {
        const { name } = JSON.parse(body || "{}");
        const file = [GEN_DIR, WIDGETS_DIR]
          .map((d) => join(d, `${name}.tsx`))
          .find(existsSync);
        if (!/^R\d+$/.test(name) || !file)
          throw new Error(`no such module ${name}`);
        execFile("open", ["-R", file], () => {});
        res.setHeader("content-type", "application/json");
        res.end(JSON.stringify({ ok: true }));
      } catch (e) {
        res.statusCode = 400;
        res.end(JSON.stringify({ ok: false, error: String(e?.message || e) }));
      }
    });
    return true;
  }
  if (req.method === "POST" && req.url === "/share/thread") {
    const thread = activeThread();
    const text = thread.turns
      .map((t) =>
        t.role === "user"
          ? `You: ${t.text}`
          : `Gen UI (${t.name}.tsx via ${t.label}):\n${
              existsSync(join(GEN_DIR, `${t.name}.tsx`))
                ? readFileSync(join(GEN_DIR, `${t.name}.tsx`), "utf8")
                : ""
            }`,
      )
      .join("\n\n");
    execFileSync("pbcopy", [], { input: text });
    res.setHeader("content-type", "application/json");
    return res.end(JSON.stringify({ ok: true }));
  }
  // ————— publish / widgets shelf / persisted state —————
  // The right pane's Details / Library: the viewed thread's generated apps
  // (newest first) with their caption, Mini Apps state and file age — the
  // app names its thread; the server's own active thread is wherever the
  // last generation ran, which leaked one bot's apps into another's pane.
  if (req.method === "GET" && req.url.split("?")[0] === "/thread/apps") {
    const id = new URL(req.url, "http://localhost").searchParams.get("thread");
    const t = (id && store.list.find((x) => x.id === id)) || activeThread();
    const published = new Set(db.prepare("SELECT name FROM widgets").all().map((w) => w.name));
    const metas = new Map(
      db.prepare("SELECT name,title,file FROM apps").all().map((r) => [r.name, r]),
    );
    const tomb = new Set(
      db.prepare("SELECT name FROM tombstones").all().map((r) => r.name),
    );
    const seen = new Set();
    const apps = [];
    for (let i = t.turns.length - 1; i >= 0; i--) {
      const u = t.turns[i];
      if (u.role !== "ai" || !u.name || seen.has(u.name) || tomb.has(u.name))
        continue;
      seen.add(u.name);
      // A name with no file anywhere was deleted before tombstones
      // existed — same zombie, same filter.
      const gone =
        !existsSync(join(GEN_DIR, `${u.name}.tsx`)) &&
        !existsSync(join(WIDGETS_DIR, `${u.name}.tsx`));
      if (gone) continue;
      let prompt = "";
      for (let j = i - 1; j >= 0; j--) if (t.turns[j].role === "user") { prompt = t.turns[j].text || ""; break; }
      const file = join(GEN_DIR, `${u.name}.tsx`);
      const caption = u.caption && !/error|implicit|rejected/i.test(u.caption) ? u.caption : "";
      const meta = metas.get(u.name) || {};
      apps.push({
        name: u.name,
        title: meta.title || u.label || caption || prompt.slice(0, 60) || u.name,
        file: meta.file || `${u.name}.tsx`,
        subtitle: prompt.replace(/\s+/g, " ").slice(0, 120),
        published: published.has(u.name),
        mtime: existsSync(file) ? statSync(file).mtimeMs : 0,
        v: headOf(u.name),
        versions: versionsOf(u.name),
      });
    }
    res.setHeader("content-type", "application/json");
    return res.end(JSON.stringify({ apps }));
  }
  return false;
}
