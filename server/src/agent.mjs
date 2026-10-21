// agent.mjs — the conversational engine: the tool loop (agentTurn), user
// content assembly (attachments, reply quotes, artifact ctx), and the full
// generate() pipeline that turns a prompt into turns.
import { copyFileSync, existsSync, mkdirSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { basename, dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { execFile } from "node:child_process";
import {
  ATTACH_DIR, GEN_DIR, MAX_TOKENS, MOCK, MODEL, OUT_DIR, PROVIDER,
  STAGE_DIR, WIDGETS_DIR, outFile,
} from "./config.mjs";
import {
  activeThread, db, jobs, modelCtx, nextSeq, seq, setJob, store, writeIndex,
  writeShelf,
} from "./state.mjs";
import {
  anthropic, askModel, chatReply, classifyIntent, labelOf,
} from "./model.mjs";
import {
  checkCandidate, errorCard, fixImports, headOf, mockModule, writeVersion,
} from "./pipeline.mjs";
import { fileMeta, mockDocx, mockFiles, renderFile } from "./files.mjs";
import { officeMarkdown } from "./office.mjs";
import {
  TOOLS, agentSystem, execTool, mcpToolDefs, toolActivity, toolList,
  agentStream, serverSearch, setAgentStream, setServerSearch,
} from "./tools.mjs";

export async function agentTurn(job, prompt, thread) {
  const messages = [
    ...thread.turns.slice(-10).map((t) =>
      t.role === "user"
        ? { role: "user", content: t.text }
        : {
            role: "assistant",
            content: t.name ? `[mounted card ${t.name}]` : t.text || "",
          },
    ),
    { role: "user", content: await userContent(prompt, job.extra, job.ctx) },
  ];
  let reply = "";
  for (let i = 0; i < 8; i++) {
    const thinkingDetail = i === 0 ? `${labelOf(modelCtx.getStore())} is thinking` : "continuing…";
    setJob(job, "thinking", thinkingDetail);
    let msg;
    const t0 = Date.now();
    let events = 0;
    try {
      const params = {
        model: MODEL,
        max_tokens: MAX_TOKENS,
        system: agentSystem(),
        tools: await toolList(thread),
        messages,
      };
      if (agentStream) {
        const stream = anthropic.messages.stream(params);
        stream.on("streamEvent", (event) => {
          events++;
          if (event.type !== "content_block_start") return;
          const block = event.content_block;
          if (block.type === "tool_use") setJob(job, ...toolActivity(block.name, block.input));
          else if (block.type === "server_tool_use") setJob(job, "searching", "Searching the web");
          else if (block.type === "text") setJob(job, "streaming", "Replying");
          else if (block.type === "thinking" || block.type === "redacted_thinking")
            setJob(job, "thinking", thinkingDetail);
        });
        msg = await stream.finalMessage();
      } else {
        msg = await anthropic.messages.create(params);
      }
    } catch (e) {
      const m = String(e?.message || "");
      if (serverSearch && /web_search|server_tool|tool_use/i.test(m)) {
        console.error(`  server web_search rejected (${m.slice(0, 120)}) — using client search`);
        setServerSearch(false);
        continue;
      }
      if (agentStream && events === 0) {
        console.error(`  Messages streaming unavailable (${m.slice(0, 160)}) — using create`);
        setAgentStream(false);
        continue;
      }
      throw new Error(`llm-api ${e.status ?? ""}: ${m}`);
    }
    messages.push({ role: "assistant", content: msg.content });
    console.log(
      `  · call ${i}: ${Date.now() - t0}ms stop=${msg.stop_reason} blocks=${msg.content.map((b) => b.type).join(",")}`,
    );
    // pause_turn = a server-side tool (web_search) is still mid-run — keep
    // polling with the same messages; tool_use = a client tool needs us.
    if (msg.stop_reason === "pause_turn") continue;
    const uses = msg.content.filter((b) => b.type === "tool_use");
    if (msg.stop_reason !== "tool_use" || !uses.length) {
      reply = msg.content
        .filter((b) => b.type === "text")
        .map((b) => b.text)
        .join("")
        .trim();
      break;
    }
    const results = [];
    for (const u of uses) {
      setJob(job, ...toolActivity(u.name, u.input));
      results.push({
        type: "tool_result",
        tool_use_id: u.id,
        ...(await execTool(job, thread, u)),
      });
    }
    messages.push({ role: "user", content: results });
    if (msg.stop_reason === "max_tokens") break;
  }
  return reply;
}

// A reply on an artifact: the file itself becomes context — images as image
// blocks, text/code/json/html/csv verbatim, xlsx as tab-separated sheets,
// docx as its text; anything else (audio, video, pdf…) is described.
export const IMG_TYPE = { png: "image/png", jpg: "image/jpeg", jpeg: "image/jpeg", gif: "image/gif", webp: "image/webp" };
export async function fileBody(p, m) {
  if (["markdown", "text", "json", "html"].includes(m.kind) || m.ext === "svg") return readFileSync(p, "utf8");
  if (m.kind === "table") return ((await renderFile(p)).sheets || []).map((s) => `# sheet: ${s.name}\n` + s.rows.map((r) => r.join("\t")).join("\n")).join("\n\n");
  if (m.kind === "docx") return (await renderFile(p)).text || "";
  if (m.kind === "pptx") return await officeMarkdown(p);
  return null;
}
export async function artifactBlocks(rels) {
  const blocks = [];
  for (const rel of (rels || []).slice(0, 8)) {
    const p = outFile(String(rel));
    if (!p || !existsSync(p)) continue;
    const m = fileMeta(p);
    try {
      if (m.kind === "image" && IMG_TYPE[m.ext]) {
        blocks.push({ type: "image", source: { type: "base64", media_type: IMG_TYPE[m.ext], data: readFileSync(p).toString("base64") } });
        blocks.push({ type: "text", text: `(the image above is ${m.name}, ${m.width}×${m.height})` });
        continue;
      }
      const body = await fileBody(p, m);
      if (body != null) blocks.push({ type: "text", text: `<file name="${m.name}">\n${body.slice(0, 60000)}\n</file>` });
      else blocks.push({ type: "text", text: `<file name="${m.name}" kind="${m.kind}" size="${m.size}">(binary ${m.kind} file; its contents can't be shown as text)</file>` });
    } catch {}
  }
  return blocks;
}

// The model's view of a user turn: the quoted message it replies to, then
// the text, with attached images as image blocks and documents inlined.
export async function userContent(prompt, extra, ctx = []) {
  const quote = extra?.reply?.text ? `> ${extra.reply.text.replace(/\n/g, "\n> ")}\n\n` : "";
  const text = quote + (prompt || "(see the attachments)");
  const files = (extra?.attachments || []).filter(
    (a) => a.uri?.startsWith("file://") || a.uri?.startsWith("module://"),
  );
  if (!files.length && !ctx.length) return text;
  const blocks = [...ctx];
  for (const a of files) {
    // module://R<n>[@v<k>] resolves to the artifact's TSX (that exact
    // version when pinned) — reply-attaching a mini app hands the model its
    // full source so it can iterate on it.
    const mm = a.uri.match(/^module:\/\/(R\d+)(?:@v(\d+))?$/);
    const path = mm
      ? mm[2]
        ? join(GEN_DIR, mm[1], `v${mm[2]}.tsx`)
        : [GEN_DIR, WIDGETS_DIR]
            .map((d) => join(d, `${mm[1]}.tsx`))
            .find(existsSync)
      : a.uri.startsWith("module://")
        ? null
        : decodeURIComponent(a.uri.slice(7));
    if (!path) continue;
    try {
      if (a.kind === "image") {
        blocks.push({ type: "image", source: { type: "base64", media_type: "image/png", data: readFileSync(path).toString("base64") } });
      } else {
        const m = fileMeta(path, a.name);
        const body = await fileBody(path, m);
        if (body != null) blocks.push({ type: "text", text: `<file name="${a.name}">\n${body.slice(0, 60000)}\n</file>` });
        else blocks.push({ type: "text", text: `<file name="${a.name}" kind="${m.kind}" size="${m.size}">(binary ${m.kind} file; its contents can't be shown as text)</file>` });
      }
    } catch {}
  }
  return [...blocks, { type: "text", text }];
}

export async function generate(job, prompt) {
  // The job stays on the thread it was sent from — you can switch threads
  // while a generation is still running and it lands where it belongs.
  const thread =
    store.list.find((t) => t.id === job.thread) || activeThread();
  let name, aiTurn;
  // "/mock files": a canned reply carrying one file of every kind — tests the
  // viewers without a model call
  if (!job.retry && /^\/mock\b/i.test(prompt.trim())) {
    const rel = `m${Date.now().toString(36)}`;
    const dir = join(OUT_DIR, rel);
    const paths = mockFiles(dir);
    // the office samples are the real BetterOffice demo docs (server/office/
    // seeds/ — betteroffice-demo.docx, showcase.xlsx, presentation.pptx), so
    // the cards exercise the editor on real-world content; when a seed is
    // absent we still emit the generated fallback file
    const SEEDS = join(dirname(fileURLToPath(import.meta.url)), "office", "seeds");
    // The pptx fallback depends on the optional BetterOffice engine — absent,
    // /mock still works and the card simply isn't produced.
    const fallback = {
      docx: () => mockDocx(dir),
      xlsx: () => mockXlsx(dir),
      pptx: () => mockPptx(dir).catch((e) => (console.error(`  mock pptx: ${String(e?.message || e).slice(0, 120)}`), null)),
    };
    for (const [ext, file] of [["docx", "betteroffice-demo.docx"], ["xlsx", "showcase.xlsx"], ["pptx", "presentation.pptx"]]) {
      const s = join(SEEDS, file);
      if (existsSync(s)) {
        const p = join(dir, file);
        copyFileSync(s, p);
        paths.push(p);
      } else {
        const p = await fallback[ext]();
        if (p) paths.push(p);
      }
    }
    // a screen recording as the video sample, twice (gallery of two)
    const movs = readdirSync(ATTACH_DIR).filter((f) => f.endsWith(".mov"));
    if (movs.length) {
      copyFileSync(join(ATTACH_DIR, movs[0]), join(dir, "demo.mov"));
      copyFileSync(join(ATTACH_DIR, movs[0]), join(dir, "replay.mov"));
      paths.push(join(dir, "demo.mov"), join(dir, "replay.mov"));
    }
    const files = paths.map((p) => {
      const m = fileMeta(p);
      return { path: `${rel}/${p.split("/").pop()}`, ...m, ...(m.poster ? { poster: `${rel}/${m.poster}` } : {}) };
    });
    // an image whose file is gone: the gallery shows "Image unavailable"
    const firstImg = files.findIndex((f) => f.kind === "image");
    files.splice(firstImg + 1, 0, { path: `${rel}/missing.png`, name: "missing.png", ext: "png", size: 0, kind: "image", variant: "media" });
    // "/mock media": just the gallery (images · videos) and the diagram
    if (/media/i.test(prompt)) files.splice(0, files.length, ...files.filter((f) => f.kind === "image" || f.kind === "video"));
    if (thread.turns.length === 0) thread.title = "Mock files";
    thread.turns.push({ role: "user", text: prompt, ...(job.extra || {}) });
    thread.turns.push({
      role: "ai",
      text: `Here are ${files.length} sample files — one of each kind the viewer handles.\n\n\`\`\`mermaid\nflowchart LR\n  A[You ask] --> B[Kiffy makes the file] --> C[You click to view]\n\`\`\``,
      label: "mock",
      files,
    });
    writeIndex();
    setJob(job, "done", "replied");
    job.done = true;
    job.result = "reply";
    return;
  }
  // A module://R<n>[@v<k>] attachment = "iterate on this artifact": the run
  // produces a new version of it (reply-to-revise), not a new artifact.
  const attachRev = (job.extra?.attachments || [])
    .map((a) => (a.uri || "").match(/^module:\/\/(R\d+)(?:@v(\d+))?$/) || null)
    .find(Boolean);
  if (attachRev) job.revise = { name: attachRev[1], base_v: Number(attachRev[2]) || null };
  const useTools = !MOCK && PROVIDER === "anthropic";
  if (!job.retry && useTools) {
    // Tool-call agent — the model decides: text answer, tool calls, widget
    // cards, or a composition (search → build). Cards land as they pass the
    // compiler; the final text answer lands last.
    if (thread.turns.length === 0) thread.title = prompt.slice(0, 42);
    thread.turns.push({ role: "user", text: prompt, ...(job.extra || {}) });
    const text = await agentTurn(job, prompt, thread);
    if (text) thread.turns.push({ role: "ai", text, label: labelOf(modelCtx.getStore()) });
    writeIndex();
    console.log(`✓ turn via ${labelOf(modelCtx.getStore())} (${prompt.slice(0, 60)}) — ${job.cards || 0} card(s)`);
    setJob(job, "done", job.cards ? `${job.cards} card(s)` : "replied");
    job.done = true;
    job.result = job.cards ? "cards" : "reply";
    return;
  }
  if (!job.retry) {
    // Conversation ≠ generation — "hi" gets a text reply, not a greeting card.
    // a turn carrying files is a question about them: the widget generator
    // never sees file contents, so it always answers in chat
    // a bare module attach still wants generation — it's a revise request.
    const nonModule = (job.extra?.attachments || []).some(
      (a) => !a.uri?.startsWith("module://"),
    );
    const hasFiles = !!job.ctx?.length || nonModule;
    const intent = job.revise
      ? "ui"
      : !hasFiles
        ? await classifyIntent(prompt)
        : "chat";
    if (intent === "chat") {
      setJob(job, "thinking", `${labelOf(modelCtx.getStore())} is replying`);
      if (thread.turns.length === 0) thread.title = prompt.slice(0, 42);
      const text = await chatReply(prompt, thread, job);
      thread.turns.push({ role: "user", text: prompt, ...(job.extra || {}) });
      thread.turns.push({ role: "ai", text, label: labelOf(modelCtx.getStore()) });
      writeIndex();
      console.log(`✓ reply (${prompt.slice(0, 60)})`);
      setJob(job, "done", "replied");
      job.done = true;
      job.result = "reply";
      return;
    }
  }
  // Revises (Try-again, or a module:// attachment = reply-to-iterate) mint a
  // NEW version of the same artifact: the old card keeps its version and a
  // fresh card lands. baseV records the branch point — the card's own
  // version for a retry, the explicit @v / current pointer for an attach.
  let baseV = null;
  if (job.retry || job.revise) {
    name = job.retry || job.revise.name;
    const prior =
      thread.turns.find((t) => t.role === "ai" && t.name === name && t.widget) ||
      thread.turns.find((t) => t.role === "ai" && t.name === name);
    baseV = job.retry
      ? prior?.v ?? headOf(name)
      : (job.revise.base_v ?? headOf(name)) ?? prior?.v ?? null;
    if (!job.retry) {
      // A reply-to-iterate carries a real user prompt; Try-again reuses the
      // card's original one — no duplicate user turn.
      if (thread.turns.length === 0) thread.title = prompt.slice(0, 42);
      thread.turns.push({ role: "user", text: prompt, ...(job.extra || {}) });
    }
    aiTurn = {
      role: "ai",
      name,
      label: labelOf(modelCtx.getStore()),
      retries: (prior?.retries || 0) + 1,
    };
    thread.turns.push(aiTurn);
  } else {
    name = `R${nextSeq()}`;
    if (thread.turns.length === 0) thread.title = prompt.slice(0, 42);
    thread.turns.push({ role: "user", text: prompt, ...(job.extra || {}) });
    aiTurn = { role: "ai", name, label: labelOf(modelCtx.getStore()) };
    thread.turns.push(aiTurn);
  }

  // llm → llm with the compiler error → canned mock, whichever compiles first.
  const useLlm =
    !MOCK &&
    (PROVIDER === "anthropic" || !!process.env.OPENROUTER_API_KEY);
  let lastCode;
  let lastCaption = "";
  let err = null;
  let ok = false;
  let used = useLlm ? MODEL : "mock";

  const tryCode = async (code) => {
    lastCode = fixImports(code);
    // check in staging — nothing lands in generated/ until it compiles
    writeFileSync(join(STAGE_DIR, `${name}.tsx`), lastCode);
    setJob(job, "checking", `nsh build + typecheck ${name}.tsx`);
    err = await checkCandidate(name);
    return err === null;
  };

  if (useLlm)
    for (let attempt = 0; attempt < 3 && !ok; attempt++) {
      if (attempt > 0)
        setJob(
          job,
          "retrying",
          `compile error — feeding it back to the model (attempt ${attempt + 1})`,
        );
      else setJob(job, "thinking", `${labelOf(modelCtx.getStore())} is working on it`);
      try {
        const r = await askModel(job, prompt, err, lastCode, name);
        lastCaption = r.caption || "";
        ok = await tryCode(r.code);
        if (!ok)
          console.error(
            `  attempt ${attempt + 1} rejected by nsh check: ${err?.split("\n")[0]}`,
          );
      } catch (e) {
        err = String(e?.message || e);
        console.error(`  llm call failed: ${err.split("\n")[0]}`);
        if (err.startsWith("llm-api")) break; // API dead — don't retry it
      }
    }

  if (!ok) {
    used = useLlm ? "mock-fallback" : "mock";
    setJob(job, "fallback", "model path failed — building a canned module");
    ok = await tryCode(mockModule(prompt)).catch(() => false);
  }
  aiTurn.label = used;
  aiTurn.caption = ok && used === MODEL ? lastCaption : "";
  if (!ok) console.error(`✗ ${name}: generation failed\n${err}`);
  setJob(job, "publishing", `hot-patching ${name}.tsx into the app`);
  // Write the module FIRST, then pause so the dev server's watcher registers
  // it before index.tsx starts importing it — otherwise the rebuild races the
  // new file and the device shows [UNRESOLVED_IMPORT]. A published module is
  // the live one — the shelf and pop-out wrappers import widgets/<name> — so
  // when that file exists the retry must patch it, not a stale generated copy.
  // Versioned write: generated/<name>/v<k>.tsx is immutable, the bare
  // <name>.tsx (and a published widget copy) mirror the new head.
  const v = writeVersion(name, ok ? lastCode : errorCard(err || "unknown"), baseV, prompt);
  aiTurn.v = v;
  await new Promise((r) => setTimeout(r, 500));
  writeIndex(); // one live write → one clean hot patch on the device
  const line = `${name}.tsx via ${used}`;
  console.log(`${ok ? "✓" : "✗"} ${line} (${prompt.slice(0, 60)})`);
  setJob(job, "done", `${ok ? "rendered" : "failed"} — ${line}`);
  job.done = true;
  job.result = line;
}

// ————— canned modules for MOCK mode / offline —————
