// model.mjs — provider clients and the model calls: Anthropic (routed
// through opencodex per-thread), OpenAI-compatible, and the canned replies.
import Anthropic from "@anthropic-ai/sdk";
import { BASE_URL, MAX_TOKENS, MOCK, MODEL, OPENCODEX, PROVIDER } from "./config.mjs";
import { activeThread, getLinearKey, jobs, metaGet, modelCtx, persist, setJob, store, db } from "./state.mjs";

const anthropicDirect =
  PROVIDER === "anthropic"
    ? new Anthropic({
        baseURL: process.env.ANTHROPIC_BASE_URL || OPENCODEX,
        apiKey: process.env.ANTHROPIC_API_KEY || "opencodex",
      })
    : null;
let opencodex = null;
const withEffort = (p, c) => (c?.effort ? { ...p, output_config: { effort: c.effort }, reasoning_effort: c.effort } : p);
const routed = (params) => {
  const c = modelCtx.getStore();
  // The default model still takes the bot's chosen effort.
  if (!c?.model) return [anthropicDirect, withEffort(params, c)];
  opencodex ??= new Anthropic({ baseURL: OPENCODEX, apiKey: "opencodex" });
  return [opencodex, withEffort({ ...params, model: c.model + (c.fast ? "--fast" : "") }, c)];
};
export const anthropic = anthropicDirect && {
  messages: {
    create: (p, o) => { const [c, q] = routed(p); return c.messages.create(q, o); },
    stream: (p, o) => { const [c, q] = routed(p); return c.messages.stream(q, o); },
  },
};
export const choiceFor = (threadId) => {
  const b = store.bots.find((x) => x.threadId === threadId);
  const m = metaGet("model");
  // A bot on the Default model keeps its own effort / fast choice.
  const base = b?.model
    ? { model: b.model, effort: b.effort || "", fast: !!b.fast }
    : m
      ? { model: m, effort: b?.effort || metaGet("effort") || "", fast: b ? !!b.fast : metaGet("fast") === "1" }
      : b?.effort || b?.fast
        ? { model: "", effort: b.effort || "", fast: !!b.fast }
        : null;
  // Status labels credit the bot, not the engine: "Grok is thinking" in a
  // bot's thread, "claude-haiku-5-5 is thinking" in a plain session.
  return b?.name ? { ...(base || {}), name: b.name } : base;
};
export const labelOf = (c) =>
  c?.name || (c?.model ? c.model + (c.fast ? "--fast" : "") : MODEL);
export function sourcesBlock() {
  const srcs = [
    `  timeclock — local clock-in/out tracker stored in the app's sqlite db
     GET  /data/timeclock → {status:"in"|"out", since:iso|null, todayMin:number, sessions:[{in,out,min}]}
     POST /data/timeclock/in  — clock in    POST /data/timeclock/out — clock out`,
    `  reminders — the user's macOS Reminders
     GET  /data/reminders → [{name,list,due,flagged}] (or {error:"…"})
     POST /data/reminders/add {name,list?,due?}    POST /data/reminders/complete {name}`,
  ];
  if (getLinearKey())
    srcs.push(`  linear — the user's Linear issues (assigned, else recent workspace issues). Every issue has uid (internal id for mutations), id (FER-12 label), stateId.
     GET  /data/linear/issues → {scope:"assigned"|"workspace", issues:[{uid,id,title,state,stateId,priority,assignee,team,url}]}
     GET  /data/linear/states → {teams:[{team,states:[{id,name,type}]}]} (transition targets per team)
     POST /data/linear/update {uid, stateId?, priority?}  — change state/priority
     POST /data/linear/assign {uid}                       — assign to me
     POST /data/linear/comment {uid, body}                — comment on it
     Interactive Linear widgets SHOULD: list issues, let a tap select one, then offer per-team state transitions (GET states once), assign-to-me, and a small comment input; refetch issues after any mutation.`);
  return srcs.join("\n");
}

export const systemFor = (name) => `You write Native Shell (nsh) modules: ONE TSX file, rendered as native UI (UIKit/AppKit). You are writing module ${name}.
Reply format: optionally ONE short markdown caption line (may use **bold**, \`code\`, $math$ — rendered natively above your widget), then the module inside a single \`\`\`tsx fenced block. No other prose, no second code block.

${contractFor(name)}`;

// The module contract — shared by the one-shot generator and the tool-call
// agent. ${name} is the literal the module uses as its /state key: the real
// name for the legacy path, "__NAME__" for the agent (substituted on write).
export const contractFor = (name) => `Contract:
- Exactly one default export: \`export default function R()\` returning JSX. Never call mount().
- IMPORT EVERY NAME YOU USE: signal, Text, View, Scroll, Input come from "nsh" (add every one you reference); List from "nsh/list"; Canvas from "nsh/canvas". Never import react/react-native/npm packages.
- <If when={cond}> and <Each of={array}>{(item, i) => …}</Each> are globals, no import. 'of' takes an ARRAY — pass the value, e.g. of={items()} or of={board()}, never the function itself. Each's row parameters are ACCESSORS — call them: item().field, i(). Never use them as plain values: wrong is 'item * 2' or 'item + "px"', right is 'item() * 2'.
- GAME/ANIMATION LOOPS: setInterval/setTimeout/clearInterval are globals returning number. Wrap timers in effect+cleanup (both imported from "nsh"): effect(() => { const t = setInterval(tick, 300); cleanup(() => clearInterval(t)); }); — tick mutates signals, UI follows.
- GRIDS/CANVAS GAMES: prefer <Canvas draw={(ctx, size) => {...}} style={{ width: W, height: H }} /> from "nsh/canvas" over hundreds of Views — it redraws automatically whenever a signal read inside draw() changes. ctx is CanvasRenderingContext2D (fillRect, fillStyle, etc.).
- Components run ONCE. State: const x = signal(0); read x() in JSX; write x.set(v)/x.update(fn) in handlers. Buttons: <View onPress={fn} activeOpacity={0.7}>.
- TEXT FIELDS: <Input value={x()} onChangeText={(t) => x.set(t)} /> — onChangeText gets the text itself. Every other event handler gets ONE object: onChange={(e) => x.set(e.value)}, onLayout={({ bounds }) => w.set(bounds.size.width)}.
- Every visible string must be inside <Text> — bare strings in <View> render nothing.
- Style with Tailwind: class="…" using only common utilities: flex, flex-row, flex-1, gap-N, p-N, px-N, py-N, m-N, mt-N, mb-N, rounded*, border, border-*, bg-*, text-* (size AND color), font-*, items-*, justify-*, self-*, w-N, h-N, max-w-*, min-h-*, overflow-hidden, opacity-*. Or style={{ camelCase }} — flexbox, column default, numbers in points.
- PERSISTED STATE: the app has a database — every widget that produces data worth remembering MUST persist it. Scores, streaks, session counts, check-ins, notes, settings, timers' completions: if the widget counts or collects anything, save it. fetch ONLY http://127.0.0.1:8787/state/${name} (all other hosts are sandbox-blocked). On mount restore once: const S = "${name}"; fetch(\`http://127.0.0.1:8787/state/\${S}\`).then(r=>r.json()).then(d=>{ if (d && d.state) /* set your signals from d.state fields */ }). Save via POST (replaces the stored object): fetch(\`http://127.0.0.1:8787/state/\${S}\`, { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ score: score(), done: done() }) }). Put saves in a save() called from the same handlers that mutate state; never save during render. A pomodoro persists { sessions, minutesFocused }; a habit tracker { streak, lastCheckIn }; a game { best, plays }.
- MULTI-SURFACE SYNC: the same widget may run in the chat AND its own window at once — they share the database, not memory. After the mount restore, poll so other surfaces' writes appear: let snap = ""; effect(() => { const t = setInterval(async () => { try { const d = await fetch(\`http://127.0.0.1:8787/state/\${S}\`).then(r=>r.json()); const s = JSON.stringify(d?.state||{}); if (s !== "{}" && s !== snap) { snap = s; /* re-apply d.state fields to your signals */ } } catch {} }, 2000); cleanup(() => clearInterval(t)); }); — set snap = the same JSON inside your save() too, so your own writes don't echo back. Clocks/elapsed timers must ALSO tick on wall-second boundaries (first tick via setTimeout(1000 - Date.now() % 1000)) so two surfaces show the same second.
- DATA SOURCES: real external data is proxied at http://127.0.0.1:8787/data/… (GET returns JSON; POST takes a JSON body; all other hosts are sandbox-blocked — never fetch elsewhere). When the request needs real data, fetch the right source on mount, keep an {error}-aware fallback (small "couldn't load" text + a retry button), and offer refresh (a button and/or a setInterval poll). Available:
${sourcesBlock()}
- KEYBOARD: import { keymap } from "nsh/keymap"; inside the component call keymap.bind({ left: () => move(-1), right: () => move(1), down: () => step(), up: () => rotate(), space: () => drop() }, { repeat: true }). Keystrokes: letters/numbers/space/enter/escape/tab/arrows/f1-f24, optional mod+/cmd+/ctrl+/alt+/shift+ prefix. Bindings auto-unbind with the component and don't fire while a text input eats the key. Reserved keys (cmd+q etc.) throw — skip them. A binding fires while ITS OWN window has the focus: inside the chat card the chat's input still owns typing keys (arrows/space included), so a keyboard-driven app MUST ALSO render tap controls for the chat surface — the user can send it to its own window (the ↗ action), where every bound key goes to it. Games SHOULD use it: arrows + space beat tap buttons.
- CLIPBOARD: import { Clipboard } from "nsh/clipboard"; Clipboard.setString(text) copies natively — use it for copy-to-clipboard affordances (a small ⧉ button on values, ids, results).
- POP-OUT SIZE (optional): the same module may be opened in its own floating window — resizable, normal z-order. Its WIDTH there is what you declare: export const popoutWidth = 520; (plain number, 240–720). Canvas apps/games may fix BOTH axes: export const popoutSize = { w: 320, h: 560 }; — the window is exactly that, no measuring. Without either: 360pt wide, height hugs content. Pop-out rules: (a) the top ~26pt of the window is an invisible drag strip — keep essential controls out of it (traffic lights sit in its left ~80pt); (b) lists that can grow tall MUST wrap in a bounded Scroll (max-h-96 or a fixed height) — an unbounded list measures its full content height and the window swallows the screen; (c) keep a min-h on the root so a transient empty/loading render never collapses the window.
- THEME (Sand design system — match it exactly): pop-out windows follow the system light/dark appearance — put dark: variants on every bg/text/border color; a hardcoded light card glows on a dark desktop. Surfaces: bg-white dark:bg-[#161616] (pane) · bg-[#f5f5f5] dark:bg-[#1f1f1f] (elevated card) · bg-black/5 dark:bg-white/5 (inset/track). Text: text-[#1c1c1c] dark:text-[#f5f5f5] (primary) · /60 alpha for secondary, /40 for tertiary (e.g. text-[#1c1c1c99] dark:text-[#f5f5f599]). Hairline borders: border-black/10 dark:border-white/10. Accent: #1680f3 light / #1884fc dark. Radius: 6 small, 10 medium, 14 card, 18 bubble/pill-ish. Spacing on a 4pt grid (p-2=8, p-3=12). Type: 12–14px body, 600 weight for emphasis — SF look, no exotic fonts. Canvas code may read colorScheme() from "nsh/device" inside draw() — reading it there redraws on theme change.
- No props, no imports beyond nsh modules, no fetch other than /state and /data: render standalone inside a ~340pt-wide chat card (it may also run in its own window).
- Make it a small, polished, interactive widget for the user's request: cards, charts as View bars, forms, lists, counters, steppers, games with real loops. Compact code, but take the lines a working game needs (~150 is fine).`;

export function extractCode(raw) {
  const text = String(raw || "").trim();
  let code = text;
  let caption = "";
  const m = text.match(/```(?:tsx?|jsx?)?\s*\n([\s\S]*?)```/);
  if (m) {
    code = m[1].trim();
    // everything outside the fence is the model's markdown caption
    caption = (text.slice(0, m.index) + text.slice((m.index || 0) + m[0].length))
      .replace(/```[\s\S]*?```/g, "")
      .trim();
  }
  if (!code.includes("export default")) {
    const def = code.match(/function\s+R\s*\(/);
    if (def) code += "\nexport default R;\n";
  }
  if (!code.includes("export default") || !code.includes("<"))
    throw new Error("model did not return a module");
  return { caption, code };
}

export async function askModel(job, prompt, prevError, prevCode, name) {
  if (MOCK) return { caption: "", code: mockModule(prompt) };
  const messages = [{ role: "user", content: prompt }];
  if (prevError) {
    if (typeof prevCode === "string")
      messages.push({ role: "assistant", content: prevCode });
    messages.push({
      role: "user",
      content: prevError.includes("truncated at the token limit")
        ? "Your reply was cut off at the token limit. Return the SAME module but shorter: fewer comments, tighter code, no extras."
        : prevError.includes("did not return a module")
        ? "Your reply had no module. Reply with ONE fenced ```tsx block containing the complete module (it must include export default) and nothing else."
        : `The nsh compiler rejected it:\n${prevError}\nReturn a corrected module only.`,
    });
  }
  if (PROVIDER === "anthropic") {
    try {
      const stream = anthropic.messages.stream({
        model: MODEL,
        max_tokens: MAX_TOKENS,
        // haiku-5.5 deprecates temperature — omit it
        system: systemFor(name),
        messages,
      });
      stream.on("text", (_delta, snapshot) =>
        setJob(job, "coding", "Writing some code"),
      );
      const msg = await stream.finalMessage();
      const text = msg.content
        .filter((b) => b.type === "text")
        .map((b) => b.text)
        .join("");
      if (msg.stop_reason === "max_tokens")
        throw new Error("module truncated at the token limit");
      return extractCode(text);
    } catch (e) {
      // A malformed reply (no tsx block) or a truncated one is retriable —
      // only transport/API failures get the llm-api prefix that stops retries.
      if (
        e?.message === "model did not return a module" ||
        e?.message === "module truncated at the token limit"
      )
        throw e;
      throw new Error(`llm-api ${e.status ?? ""}: ${e.message}`);
    }
  }
  const r = await fetch(`${BASE_URL}/chat/completions`, {
    method: "POST",
    headers: {
      Authorization: `Bearer ${process.env.OPENROUTER_API_KEY}`,
      "Content-Type": "application/json",
    },
    body: JSON.stringify({
      model: MODEL,
      messages: [{ role: "system", content: systemFor(name) }, ...messages],
      temperature: 0.4,
      max_tokens: MAX_TOKENS,
    }),
  });
  if (!r.ok) throw new Error(`llm-api ${r.status}: ${await r.text()}`);
  const data = await r.json();
  if (data.choices?.[0]?.finish_reason === "length")
    throw new Error("module truncated at the token limit");
  return extractCode(data.choices?.[0]?.message?.content);
}

// Not every message wants a widget — classify before burning a generation.
// "ui" only when the user wants an interface built; everything else is chat.
export async function classifyIntent(prompt) {
  if (MOCK) return "ui";
  const sys =
    "Classify the user message for a generative-UI chat app. Reply with exactly one word: UI or CHAT.\n" +
    "UI = the user wants an interface, widget, app, card, or game built/rendered — including terse requests that name a thing to build (\"pomodoro timer\", \"make a calculator\").\n" +
    "CHAT = greetings, questions, discussion, feedback, or anything that only wants a text answer.";
  try {
    let answer;
    if (PROVIDER === "anthropic") {
      const m = await anthropic.messages.create({
        model: MODEL,
        max_tokens: 8,
        system: sys,
        messages: [{ role: "user", content: prompt }],
      });
      answer = m.content
        .filter((b) => b.type === "text")
        .map((b) => b.text)
        .join("");
    } else {
      const r = await fetch(`${BASE_URL}/chat/completions`, {
        method: "POST",
        headers: {
          Authorization: `Bearer ${process.env.OPENROUTER_API_KEY}`,
          "Content-Type": "application/json",
        },
        body: JSON.stringify({
          model: MODEL,
          messages: [
            { role: "system", content: sys },
            { role: "user", content: prompt },
          ],
          max_tokens: 8,
        }),
      });
      answer = (await r.json()).choices?.[0]?.message?.content || "";
    }
    return answer.trim().toUpperCase().startsWith("UI") ? "ui" : "chat";
  } catch (e) {
    console.error(`  intent classify failed: ${e.message} — generating anyway`);
    return "ui"; // classifier down → fall back to the old always-generate behavior
  }
}

// Plain conversational reply (no module) — with the recent turns as context.
export async function chatReply(prompt, thread, job) {
  const sys =
    "You are the assistant inside a generative-UI chat app: when the user asks for an interface, a tool builds and renders it natively. This message is conversation — answer in GitHub-flavored markdown, concise. If a UI would obviously help, offer to build it in one line.";
  const messages = [
    ...thread.turns.slice(-10).map((t) =>
      t.role === "user"
        ? { role: "user", content: t.text }
        : {
            role: "assistant",
            content: t.name ? `[built widget ${t.name}]` : t.text || "",
          },
    ),
    { role: "user", content: await userContent(prompt, job?.extra, job?.ctx) },
  ];
  if (MOCK) return "_(mock reply — no model configured)_";
  // userContent speaks Anthropic's block shapes; chat/completions wants
  // image_url parts instead of base64 image sources
  const oaiContent = (c) =>
    typeof c === "string"
      ? c
      : c.map((b) => (b.type === "image" ? { type: "image_url", image_url: { url: `data:${b.source.media_type};base64,${b.source.data}` } } : b));
  try {
    if (PROVIDER === "anthropic") {
      const m = await anthropic.messages.create({
        model: MODEL,
        max_tokens: 1024,
        system: sys,
        messages,
      });
      return m.content
        .filter((b) => b.type === "text")
        .map((b) => b.text)
        .join("")
        .trim();
    }
    const r = await fetch(`${BASE_URL}/chat/completions`, {
      method: "POST",
      headers: {
        Authorization: `Bearer ${process.env.OPENROUTER_API_KEY}`,
        "Content-Type": "application/json",
      },
      body: JSON.stringify({ model: MODEL, messages: messages.map((m) => ({ ...m, content: oaiContent(m.content) })), max_tokens: 1024 }),
    });
    return (
      (await r.json()).choices?.[0]?.message?.content || "(empty reply)"
    ).trim();
  } catch (e) {
    return `_(model error — ${String(e.message || e).slice(0, 200)})_`;
  }
}
