// agent.mjs — the conversational engine: the tool loop (agentTurn), user
// content assembly (attachments, reply quotes, artifact ctx), and the full
// generate() pipeline that turns a prompt into turns.
import { copyFileSync, existsSync, mkdirSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { basename, dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import {
  ASSETS_DIR, ATTACH_DIR, GEN_DIR, MAX_TOKENS, MOCK, MODEL, OUT_DIR, PORT, PROVIDER,
  STAGE_DIR, WIDGETS_DIR, outFile,
} from "./config.mjs";
import {
  activeThread, cardTurn, markRevising, modelCtx, nextSeq, setJob,
  store, writeIndex, uuidv7,
} from "./state.mjs";
import {
  anthropic, askModel, chatReply, classifyIntent, labelOf,
} from "./model.mjs";
import {
  checkCandidate, errorCard, fixImports, headOf, mockModule, verFile, writeVersion,
} from "./pipeline.mjs";
import { fileMeta, fileRecord, mockDocx, mockFiles, mockImages, renderFile } from "./files.mjs";
import { officeMarkdown } from "./office.mjs";
import {
  PHRASES, agentSystem, execTool, imageFileOf, officeArt, phraseFor, toolActivity, toolList,
  agentStream, serverSearch, setAgentStream, setServerSearch,
} from "./tools.mjs";
import { scopeSection } from "./attachments.mjs";
import { v1PromptSection } from "./v1/agent.mjs";
import { plugins as v1Plugins } from "./v1/broker.mjs";

// The streamed start of a generate_app input → its placeholder's title,
// description and size (layout before code in the tool's schema).
function appPlaceholderFrom(turn, json) {
  const str = (k) => json.match(new RegExp(`"${k}"\\s*:\\s*"((?:[^"\\\\]|\\\\.)*)"`))?.[1];
  const num = (k) => Number(json.match(new RegExp(`"${k}"\\s*:\\s*(\\d+)`))?.[1]) || 0;
  const unesc = (v) => v?.replace(/\\(.)/g, "$1");
  const title = unesc(str("title"));

  if (title) turn.art.title = title.slice(0, 60);
  const desc = unesc(str("desc"));

  if (desc) turn.art.desc = desc.slice(0, 140);
  const w = num("width"), h = num("height");

  if (w >= 60 && h >= 60) { turn.art.w = Math.min(w, 2048); turn.art.h = Math.min(h, 2048); }
}

// A placeholder no generate_app / office_create call adopted (the call
// failed to parse, the round errored) leaves the chat.
function dropPendingApp(job, thread) {
  for (const k of ["pendingApp", "pendingOffice"]) {
    const t = job[k];

    if (!t) continue;
    job[k] = null;
    const i = thread.turns.indexOf(t);

    if (i >= 0) thread.turns.splice(i, 1);
  }
}

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

  // the chat's attachments and the hints of the `@` scopes in play
  const scope = (await scopeSection(thread, prompt).catch(() => "")) + v1PromptSection(v1Plugins, prompt, job.extra?.v1Attachments);
  let reply = "";
  // As in Grok Bot, the reply is held back behind "Typing..." and lands
  // whole; a note the model writes before calling tools ("Checking X
  // for …") lands at once as its own message.
  const label = labelOf(modelCtx.getStore());

  for (let i = 0; i < 8; i++) {
    // First round: thinking; after tool results: working (Grok Bot's
    // fallback between steps). A thinking block flips it back.
    const think = () => setJob(job, "thinking", phraseFor(job, "thinking"));

    if (i === 0) think();
    else setJob(job, "working", phraseFor(job, `working${i}`, PHRASES.working));
    let msg, params;
    const t0 = Date.now();
    let events = 0, tPrep = 0, tFirst = 0;

    try {
      params = {
        model: MODEL,
        max_tokens: MAX_TOKENS,
        system: agentSystem(thread) + scope,
        tools: await toolList(thread, job),
        messages,
      };

      // The last round must answer: a turn that is still calling tools
      // when the rounds run out would otherwise end with no reply.
      if (i === 7) params.tool_choice = { type: "none" };
      tPrep = Date.now() - t0;

      if (agentStream) {
        const stream = anthropic.messages.stream(params);
        // A revise_app being written covers the card it revises as soon as
        // its name is known: the attached one at once, then the `name` read
        // off the streamed input.
        let revise = null;
        // A generate_app being written gets its placeholder card at once
        // (toolGenerateApp adopts it), sized as soon as its layout streams.
        let app = null;
        // An office_create being written gets its file's placeholder too,
        // shaped by the extension of its streamed `path`.
        let office = null;
        stream.on("streamEvent", (event) => {
          events++;

          if (!tFirst) tFirst = Date.now() - t0;

          if (event.type === "content_block_delta" && app?.index === event.index && event.delta?.type === "input_json_delta") {
            app.json += event.delta.partial_json || "";

            if (app.json.length < 4000) appPlaceholderFrom(app.turn, app.json);

            return;
          }

          if (event.type === "content_block_delta" && office?.index === event.index && event.delta?.type === "input_json_delta") {
            office.json += event.delta.partial_json || "";
            const m = office.json.match(/"path"\s*:\s*"((?:[^"\\]|\\.)*)"/);

            if (m) {
              office.turn.art = officeArt(m[1].split("/").pop());
              office = null;
            }

            return;
          }

          if (event.type === "content_block_delta" && revise?.index === event.index && event.delta?.type === "input_json_delta") {
            revise.json += event.delta.partial_json || "";
            const m = revise.json.match(/"name"\s*:\s*"(R\d+)"/);

            if (m) {
              markRevising(job, cardTurn(thread, m[1], null));
              revise = null;
            }

            return;
          }

          if (event.type !== "content_block_start") return;
          const block = event.content_block;

          if (block.type === "tool_use" && block.name === "generate_app" && !job.pendingApp) {
            job.pendingApp = { id: uuidv7(), role: "ai", building: true, label, art: { title: null, desc: "", w: 0, h: 0 } };
            thread.turns.push(job.pendingApp);
            app = { index: event.index, json: "", turn: job.pendingApp };
          }

          if (block.type === "tool_use" && block.name === "office_create" && !job.pendingOffice) {
            job.pendingOffice = { id: uuidv7(), role: "ai", building: true, label: "office", art: officeArt("") };
            thread.turns.push(job.pendingOffice);
            office = { index: event.index, json: "", turn: job.pendingOffice };
          }

          if (block.type === "tool_use" && block.name === "revise_app") {
            if (job.revise) markRevising(job, cardTurn(thread, job.revise.name, job.revise.base_v));
            revise = { index: event.index, json: "" };
          }

          if (block.type === "tool_use") setJob(job, ...toolActivity(block.name, block.input));
          else if (block.type === "server_tool_use") setJob(job, "searching", "Digging around");
          else if (block.type === "text") setJob(job, "streaming", "Typing...");
          else if (block.type === "thinking" || block.type === "redacted_thinking") think();
        });
        msg = await stream.finalMessage();
      } else {
        msg = await anthropic.messages.create(params);
      }
    } catch (e) {
      dropPendingApp(job, thread);
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
      `  · call ${i}: ${Date.now() - t0}ms stop=${msg.stop_reason} blocks=${msg.content.map((b) => b.type).join(",")}` +
        ` (prep ${tPrep}ms, first event ${tFirst}ms${agentStream ? "" : ", not streamed"})` +
        ` · ${params.model} system ${Math.round(params.system.length / 1000)}k, ${params.tools.length} tools ${Math.round(JSON.stringify(params.tools).length / 1000)}k`,
    );

    // pause_turn = a server-side tool (web_search) is still mid-run — keep
    // polling with the same messages; tool_use = a client tool needs us.
    if (msg.stop_reason === "pause_turn") continue;
    const uses = msg.content.filter((b) => b.type === "tool_use");
    const said = msg.content.filter((b) => b.type === "text").map((b) => b.text).join("").trim();

    if (uses.length && said) {
      thread.turns.push({ id: uuidv7(), role: "ai", text: said, label });
      writeIndex();
    }

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
      const r = await execTool(job, thread, u);

      // A failed tool call, so a run that "couldn't" do something says why.
      if (r?.is_error) console.log(`  ✗ ${u.name}: ${String(r.content || "").replace(/\s+/g, " ").slice(0, 200)}`);
      results.push({ type: "tool_result", tool_use_id: u.id, ...r });
    }

    messages.push({ role: "user", content: results });
    dropPendingApp(job, thread);

    if (msg.stop_reason === "max_tokens") break;
  }

  return reply;
}

// A turn nobody typed (a routine's, routines.mjs): the chat shows `shown`
// as its message, the model reads `prompt`. It runs in `thread` whichever
// chat is active, and never makes it the active one.
// What the tools left on the job rides the reply's turn: Drive files and app
// cards (`links`), and a v1 answer's next steps (`suggest`, 9.8).
const linkFields = (job) => ({
  ...(job.linkTurn?.links.length ? { links: job.linkTurn.links } : {}),
  ...(job.linkTurn?.suggest?.length ? { suggest: job.linkTurn.suggest } : {}),
});

export async function backgroundTurn(job, thread, shown, prompt) {
  thread.turns.push({ id: uuidv7(), role: "user", text: shown });
  writeIndex();
  const label = labelOf(modelCtx.getStore());
  let text;

  try {
    text = !MOCK && PROVIDER === "anthropic" ? await agentTurn(job, prompt, thread) : await chatReply(prompt, thread, job);
  } finally {
    // Drive files and app cards the tools made ride the reply, even after a crash
    const links = linkFields(job);

    if (text || links.links || links.suggest) thread.turns.push({ id: uuidv7(), role: "ai", text: text || "", label, ...links });
    writeIndex();
    setJob(job, "done", "replied");
    job.done = true;
    job.result = "reply";
  }
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
    // full source so it can iterate on it. An image card hands over the
    // image itself: its source is only a frame around it.
    const mm = a.uri.match(/^module:\/\/(R\d+)(?:@v(\d+))?$/);
    const img = mm && imageFileOf(mm[1], Number(mm[2]) || null);

    if (img) {
      try {
        blocks.push({ type: "image", source: { type: "base64", media_type: "image/png", data: readFileSync(join(ASSETS_DIR, img)).toString("base64") } });
        blocks.push({ type: "text", text: `(the image above is image card ${mm[1]}${mm[2] ? ` v${mm[2]}` : ""}. To change it, call generate_image with edit_of: "${mm[1]}" and a prompt that says only what to change.)` });
      } catch {}

      continue;
    }

    const path = mm
      ? mm[2]
        ? verFile(mm[1], Number(mm[2]))
        : [GEN_DIR, WIDGETS_DIR]
            .map((d) => join(d, `${mm[1]}.tsx`))
            .find(existsSync)
      : a.uri.startsWith("module://")
        ? null
        : decodeURIComponent(a.uri.slice(7));

    if (!path) continue;

    try {
      if (a.kind === "image") {
        // vision caps at ~5MB/image — a huge upload's feed derivative
        // (≤1120px) is what the model sees; the original stays for display
        const modelPath = (a.size || 0) > 4_500_000 && a.feed ? decodeURIComponent(String(a.feed).slice(7)) : path;
        blocks.push({ type: "image", source: { type: "base64", media_type: IMG_TYPE[modelPath.split(".").pop().toLowerCase()] || "image/png", data: readFileSync(modelPath).toString("base64") } });
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
    // "/mock images n [ar]": n numbered tiles; aspect as `16x9`, `4:3`, `16/9`
    // or a decimal (1.78). Without one the tiles cycle every shape, so a
    // single message exercises portrait, landscape and square rows.
    const im = prompt.trim().match(/^\/mock\s+images?\s+(\d+)(?:\s+(\d+(?:\.\d+)?)\s*[x:/]\s*(\d+(?:\.\d+)?)|\s+(\d+(?:\.\d+)?))?/i);

    if (im) {
      const n = Math.max(1, Math.min(24, +im[1] || 1));
      const ar = im[3] ? [+im[2], +im[3]] : im[4] ? [+im[4], 1] : null;
      const files = mockImages(dir, n, ar).map((p) => fileRecord(p, `${rel}/${p.split("/").pop()}`));

      // Grok's test: the images ride the user turn as attachments (FileTile
      // row) AND the reply as files (MediaGallery) — both render paths get
      // exercised by one command.
      const attachments = files.map((f) => ({
        uri: `http://127.0.0.1:${PORT}/file/raw/${f.path.split("/").map(encodeURIComponent).join("/")}`,
        thumb: `http://127.0.0.1:${PORT}/file/raw/${(f.feed || f.path).split("/").map(encodeURIComponent).join("/")}`,
        name: f.name, ext: f.ext, kind: "image", size: f.size, width: f.width, height: f.height,
      }));

      if (thread.turns.length === 0) thread.title = `Mock images ×${n}`;
      thread.turns.push({ id: uuidv7(), role: "user", text: prompt, ...job.extra, attachments });
      thread.turns.push({
        id: uuidv7(),
        role: "ai",
        text: `${n} mock image${n === 1 ? "" : "s"}${ar ? ` at ${ar[0]}:${ar[1]}` : " across cycled aspects"} — sized ${files.map((f) => `${f.width}×${f.height}`).slice(0, 6).join(", ")}${n > 6 ? ", …" : ""}.`,
        label: "mock",
        files,
      });
      writeIndex();
      setJob(job, "done", `${n} images`);
      job.done = true;
      job.result = "reply";

      return;
    }

    // the office samples are the real BetterOffice demo docs (server/office/
    // seeds/ — betteroffice-demo.docx, showcase.xlsx, presentation.pptx), so
    // the cards exercise the editor on real-world content; when a seed is
    // absent we still emit the generated fallback file
    const SEEDS = join(dirname(fileURLToPath(import.meta.url)), "office", "seeds");
    const OFFICE = { docx: "betteroffice-demo.docx", xlsx: "showcase.xlsx", pptx: "presentation.pptx" };

    // The pptx fallback depends on the optional BetterOffice engine — absent,
    // /mock still works and the card simply isn't produced.
    const fallback = {
      docx: () => mockDocx(dir),
      xlsx: () => mockXlsx(dir),
      pptx: () => mockPptx(dir).catch((e) => (console.error(`  mock pptx: ${String(e?.message || e).slice(0, 120)}`), null)),
    };

    const emitOffice = async (ext, paths) => {
      const s = join(SEEDS, OFFICE[ext]);

      if (existsSync(s)) {
        const p = join(dir, OFFICE[ext]);
        copyFileSync(s, p);
        paths.push(p);
      } else {
        const p = await fallback[ext]();

        if (p) paths.push(p);
      }
    };

    // a screen recording as the video sample, twice (gallery of two)
    const emitMovs = (paths) => {
      const movs = readdirSync(ATTACH_DIR).filter((f) => f.endsWith(".mov"));

      if (movs.length) {
        copyFileSync(join(ATTACH_DIR, movs[0]), join(dir, "demo.mov"));
        copyFileSync(join(ATTACH_DIR, movs[0]), join(dir, "replay.mov"));
        paths.push(join(dir, "demo.mov"), join(dir, "replay.mov"));
      }
    };

    const reply = (text, files) => {
      if (thread.turns.length === 0) thread.title = "Mock files";
      thread.turns.push({ id: uuidv7(), role: "user", text: prompt, ...job.extra });
      thread.turns.push({ id: uuidv7(), role: "ai", text, label: "mock", files });
      writeIndex();
      setJob(job, "done", "replied");
      job.done = true;
      job.result = "reply";
    };

    // "/mock <ext>" — one kind only. docx/xlsx/pptx come from the office
    // seeds (or their generated fallbacks), mov from the recordings, the
    // rest filter mockFiles down to the extension. Aliases: markdown, text,
    // image(s), audio, video, doc, xls, ppt, rust, vcard, sheet, deck.
    const word = (prompt.trim().match(/^\/mock\s+([a-z0-9]+)\s*$/i) || [])[1]?.toLowerCase();

    if (word && word !== "files" && word !== "media") {
      const ALIAS = {
        markdown: "md", text: "txt", rust: "rs", vcard: "vcf", doc: "docx",
        xls: "xlsx", ppt: "pptx", sheet: "xlsx", deck: "pptx", video: "mov",
        image: "png", images: "png", document: "docx", word: "docx",
        excel: "xlsx", spreadsheet: "xlsx", powerpoint: "pptx",
        presentation: "pptx", slides: "pptx",
      };

      const exts = word === "audio" ? ["m4a", "wav", "aiff"] : [ALIAS[word] || word];

      const KNOWN = new Set([
        "pdf", "md", "html", "txt", "json", "csv", "rs", "tsx", "png", "svg",
        "gif", "webp", "vcf", "mov", "m4a", "wav", "aiff", "docx", "xlsx", "pptx",
      ]);

      const paths = [];
      let text;

      if (exts.some((e) => !KNOWN.has(e))) {
        text = `No mock for \`${word}\` — try one of: ${[...KNOWN].sort().join(", ")} (or \`files\`, \`media\`, \`images n\`, \`audio\`).`;
      } else {
        mkdirSync(dir, { recursive: true }); // emitOffice/emitMovs copy before mockFiles runs

        for (const e of exts) {
          if (OFFICE[e]) await emitOffice(e, paths);
          else if (e === "mov") emitMovs(paths);
        }

        paths.push(...mockFiles(dir, new Set(exts.filter((e) => !OFFICE[e] && e !== "mov"))));
        text = paths.length
          ? `Mock ${word}: ${paths.map((p) => p.split("/").pop()).join(", ")}.`
          : `Couldn't produce a ${word} mock here — the tools that kind needs aren't on this machine.`;
      }

      if (thread.turns.length === 0) thread.title = `Mock ${word}`;
      reply(text, paths.map((p) => fileRecord(p, `${rel}/${p.split("/").pop()}`)));

      return;
    }

    const paths = mockFiles(dir);

    for (const ext of Object.keys(OFFICE)) await emitOffice(ext, paths);
    emitMovs(paths);
    const files = paths.map((p) => fileRecord(p, `${rel}/${p.split("/").pop()}`));
    // an image whose file is gone: the gallery shows "Image unavailable"
    const firstImg = files.findIndex((f) => f.kind === "image");
    files.splice(firstImg + 1, 0, { path: `${rel}/missing.png`, name: "missing.png", ext: "png", size: 0, kind: "image", variant: "media" });

    // "/mock media": just the gallery (images · videos) and the diagram
    if (/media/i.test(prompt)) files.splice(0, files.length, ...files.filter((f) => f.kind === "image" || f.kind === "video"));

    if (thread.turns.length === 0) thread.title = "Mock files";
    thread.turns.push({ id: uuidv7(), role: "user", text: prompt, ...job.extra });
    thread.turns.push({
      id: uuidv7(),
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

  // "/upload" ("/uploadx"): the message's attachments come straight back as
  // file cards — a real upload through every viewer, no model call.
  if (!job.retry && /^\/uploadx?\b/i.test(prompt.trim())) {
    const rel = `m${Date.now().toString(36)}`;
    const dir = join(OUT_DIR, rel);
    mkdirSync(dir, { recursive: true });
    const names = new Set();

    const files = (job.extra?.attachments || [])
      .filter((a) => (a.uri || "").startsWith("file://"))
      .flatMap((a) => {
        const src = decodeURIComponent(a.uri.slice(7));

        if (!existsSync(src)) return [];
        const name = basename(a.name || src).replace(/[/\\]/g, "_");
        // two uploads of one name can't collide in the folder
        const unique = names.has(name) ? `${names.size}-${name}` : name;
        names.add(unique);
        const p = join(dir, unique);
        copyFileSync(src, p);

        return [fileRecord(p, `${rel}/${unique}`)];
      });

    const text = files.length
      ? `Echoed back ${files.length === 1 ? "your file" : `${files.length} files`} — nothing went to the model: ${files.map((f) => f.name).join(", ")}.`
      : "Attach a file and /upload echoes it back as a viewable card — nothing goes to the model.";

    if (thread.turns.length === 0) thread.title = "Upload echo";
    thread.turns.push({ id: uuidv7(), role: "user", text: prompt, ...job.extra });
    thread.turns.push({ id: uuidv7(), role: "ai", text, label: "upload", files });
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
    thread.turns.push({ id: uuidv7(), role: "user", text: prompt, ...job.extra });
    // Drive files the tools made or changed (drive.mjs) ride the reply's
    // turn — their own turn when there is no reply, even after a crash.
    const links = () => linkFields(job);
    let text;

    try {
      text = await agentTurn(job, prompt, thread);
    } catch (e) {
      if (job.linkTurn?.links.length || job.linkTurn?.suggest?.length) {
        thread.turns.push({ id: uuidv7(), role: "ai", text: "", label: labelOf(modelCtx.getStore()), ...links() });
        writeIndex();
      }

      throw e;
    }

    if (text || job.linkTurn?.links.length || job.linkTurn?.suggest?.length)
      thread.turns.push({ id: uuidv7(), role: "ai", text: text || "", label: labelOf(modelCtx.getStore()), ...links() });
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
      setJob(job, "thinking", phraseFor(job, "thinking"));

      if (thread.turns.length === 0) thread.title = prompt.slice(0, 42);
      const text = await chatReply(prompt, thread, job);
      thread.turns.push({ id: uuidv7(), role: "user", text: prompt, ...job.extra });
      thread.turns.push({ id: uuidv7(), role: "ai", text, label: labelOf(modelCtx.getStore()) });
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
      thread.turns.push({ id: uuidv7(), role: "user", text: prompt, ...job.extra });
    }

    // The fresh card lands with its version (below); until then a
    // reply-to-iterate covers the card it revises (Rox's Try Again covers
    // the pressed card itself).
    aiTurn = {
      id: uuidv7(),
      role: "ai",
      name,
      label: labelOf(modelCtx.getStore()),
      retries: (prior?.retries || 0) + 1,
    };

    if (prior?.art) aiTurn.art = prior.art;

    if (!job.retry) markRevising(job, cardTurn(thread, name, baseV));
  } else {
    name = `R${nextSeq()}`;

    if (thread.turns.length === 0) thread.title = prompt.slice(0, 42);
    thread.turns.push({ id: uuidv7(), role: "user", text: prompt, ...job.extra });
    aiTurn = { id: uuidv7(), role: "ai", name, label: labelOf(modelCtx.getStore()) };
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
    setJob(job, "checking", "Checking the code");
    err = await checkCandidate(name);

    return err === null;
  };

  if (useLlm)
    for (let attempt = 0; attempt < 3 && !ok; attempt++) {
      if (attempt > 0)
        setJob(
          job,
          "retrying",
          "Fixing the code",
        );
      else setJob(job, "coding", "Writing some code");

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
    setJob(job, "fallback", "Trying another way");
    ok = await tryCode(mockModule(prompt)).catch(() => false);
  }

  aiTurn.label = used;
  aiTurn.caption = ok && used === MODEL ? lastCaption : "";

  if (!ok) console.error(`✗ ${name}: generation failed\n${err}`);
  setJob(job, "publishing", "Putting it in place");
  // Write the module FIRST, then pause so the dev server's watcher registers
  // it before index.tsx starts importing it — otherwise the rebuild races the
  // new file and the device shows [UNRESOLVED_IMPORT]. A published module is
  // the live one — the shelf and pop-out wrappers import widgets/<name> — so
  // when that file exists the retry must patch it, not a stale generated copy.
  // Versioned write: generated/<name>/v<k>.tsx is immutable, the bare
  // <name>.tsx (and a published widget copy) mirror the new head.
  const v = writeVersion(name, ok ? lastCode : errorCard(err || "unknown"), baseV, prompt);
  aiTurn.v = v;

  if (!thread.turns.includes(aiTurn)) thread.turns.push(aiTurn);
  await new Promise((r) => setTimeout(r, 500));
  writeIndex(); // one live write → one clean hot patch on the device
  const line = `${name}.tsx via ${used}`;
  console.log(`${ok ? "✓" : "✗"} ${line} (${prompt.slice(0, 60)})`);
  setJob(job, "done", `${ok ? "rendered" : "failed"} — ${line}`);
  job.done = true;
  job.result = line;
}

// ————— canned modules for MOCK mode / offline —————
