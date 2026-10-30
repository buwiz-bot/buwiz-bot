// A scripted stand-in for opencodex on 127.0.0.1, for the broker tests that
// run with MOCK off (the Rust broker plan, phase 5 §6). It answers:
//   POST …/v1/messages            Anthropic Messages, streaming (SSE) or not
//   POST …/v1/chat/completions    the OpenAI-compatible path (GEN_PROVIDER=openai)
//   GET  …/v1/models              `fake.models` (or a 500 when `fake.modelsFail`)
//   POST …/v1/images/generations|edits   a 1×1 PNG
// Any path prefix works (an ANTHROPIC_BASE_URL of `<url>/direct` lands on
// `/direct/v1/messages`); the recorded path keeps it.
//
// Scripts: the newest user message carrying `[[script:<name>]]` picks the
// script (a tool-result round keeps the prompt's). A script is a list of
// steps, one per model request, in order (`fake.script(name, steps)` or the
// built-in SCRIPTS below); a step with `times: n` answers n requests, and
// the last step repeats once the list runs out. A request without a marker
// gets the text "ok". Steps:
//   {content: [blocks], stop_reason}  a message; blocks are text, tool_use
//       ({id?, name, input}), thinking ({thinking, signature}),
//       server_tool_use, web_search_tool_result. Streamed, text arrives in
//       two deltas and a tool's input in `chunks` input_json_delta pieces;
//       `delay` ms between events lets a test look at the chat mid-stream.
//   {status, error?, headers?}        an HTTP error (429/500 retry quickly:
//       retry-after-ms 1); `error` is the error message
//   {die: true}                       a stream that closes before its first
//       event (a non-streaming request skips the step)
//   (req) => step                     decided per request
//   A step's `classify: "UI"|"CHAT"` answers the legacy intent classifier
//   (its own request, no step used); the default is "UI".
//
// Every request is recorded: {method, path, headers (the ones opencodex
// reads), body (parsed JSON when it is), stream}; `fake.calls()` lists the
// model calls. `fake.reset()` forgets them and rewinds every script,
// `fake.clear()` forgets them only. Nothing listens beyond
// 127.0.0.1 and nothing is fetched.
import { createServer } from "node:http";

export const PNG_B64 = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";

const HEADERS = ["content-type", "x-api-key", "authorization", "anthropic-version", "anthropic-beta"];

// message steps, shorthand
export const say = (text, stop_reason = "end_turn") => ({ content: [{ type: "text", text }], stop_reason });
export const use = (name, input, { id = `toolu_${name}`, text, chunks = 3, delay } = {}) => ({
  content: [...(text ? [{ type: "text", text }] : []), { type: "tool_use", id, name, input, chunks }],
  stop_reason: "tool_use",
  ...(delay ? { delay } : {}),
});

export const SCRIPTS = {
  text: [say("Hello from the fake.")],
  "tool-then-text": [use("react", { emoji: "👍" }), say("Done reacting.")],
  "note-then-tool": [use("react", { emoji: "👀" }, { text: "Checking that for you." }), say("All checked.")],
  thinking: [{ content: [{ type: "thinking", thinking: "Let me think.", signature: "sig-1" }, { type: "text", text: "Thought it through." }], stop_reason: "end_turn" }],
  pause: [
    { content: [{ type: "server_tool_use", id: "srvtoolu_1", name: "web_search", input: { query: "rox" } }], stop_reason: "pause_turn" },
    {
      content: [
        { type: "web_search_tool_result", tool_use_id: "srvtoolu_1", content: [{ type: "web_search_result", url: "https://example.com/", title: "Example", encrypted_content: "x", page_age: null }] },
        { type: "text", text: "Found it after the pause." },
      ],
      stop_reason: "end_turn",
    },
  ],
  "error-429": [{ status: 429, error: "slow down", times: 2 }, say("After the retries.")],
  "error-500": [{ status: 500, error: "upstream exploded" }],
  die: [{ die: true }, say("Answered without streaming.")],
};

function chunk(s, n) {
  if (!s || n <= 1) return [s];
  const size = Math.ceil(s.length / n);
  const out = [];

  for (let i = 0; i < s.length; i += size) out.push(s.slice(i, i + size));

  return out;
}

// the message as the non-streaming API returns it
function finalMessage(step, model) {
  return {
    id: "msg_fake",
    type: "message",
    role: "assistant",
    model,
    content: step.content.map(({ chunks: _c, ...b }, i) =>
      b.type === "tool_use" || b.type === "server_tool_use" ? { ...b, id: b.id || `toolu_${i}` } : b,
    ),
    stop_reason: step.stop_reason || "end_turn",
    stop_sequence: null,
    usage: { input_tokens: 10, output_tokens: 10 },
  };
}

// the events, `step.delay` ms apart (so a test can look in between)
async function sse(res, step, model) {
  const events = [];
  const ev = (type, data) => events.push(`event: ${type}\ndata: ${JSON.stringify({ type, ...data })}\n\n`);
  const msg = finalMessage(step, model);

  ev("message_start", { message: { ...msg, content: [], stop_reason: null, usage: { input_tokens: 10, output_tokens: 1 } } });
  step.content.forEach((b, index) => {
    const block = msg.content[index];

    if (b.type === "text") {
      ev("content_block_start", { index, content_block: { type: "text", text: "" } });
      for (const text of chunk(b.text, 2)) ev("content_block_delta", { index, delta: { type: "text_delta", text } });
    } else if (b.type === "thinking") {
      ev("content_block_start", { index, content_block: { type: "thinking", thinking: "", signature: "" } });
      ev("content_block_delta", { index, delta: { type: "thinking_delta", thinking: b.thinking } });
      ev("content_block_delta", { index, delta: { type: "signature_delta", signature: b.signature } });
    } else if (b.type === "tool_use" || b.type === "server_tool_use") {
      ev("content_block_start", { index, content_block: { type: b.type, id: block.id, name: b.name, input: {} } });
      const json = JSON.stringify(b.input ?? {});

      if (json !== "{}") for (const partial_json of chunk(json, b.chunks || 3)) ev("content_block_delta", { index, delta: { type: "input_json_delta", partial_json } });
    } else {
      ev("content_block_start", { index, content_block: block });
    }

    ev("content_block_stop", { index });
  });
  ev("message_delta", { delta: { stop_reason: msg.stop_reason, stop_sequence: null }, usage: { output_tokens: 10 } });
  ev("message_stop", {});

  for (const e of events) {
    if (step.delay) await new Promise((r) => setTimeout(r, step.delay));
    res.write(e);
  }

  res.end();
}

// the newest user message's text that carries a marker
function markerOf(messages) {
  for (const m of [...(messages || [])].reverse()) {
    if (m.role !== "user") continue;
    const text = typeof m.content === "string" ? m.content : (m.content || []).filter((b) => b.type === "text").map((b) => b.text).join("\n");
    const hit = /\[\[script:([\w.-]+)\]\]/.exec(text || "");

    if (hit) return hit[1];
  }

  return null;
}

export async function startFakeOpencodex({ port = 0 } = {}) {
  const scripts = { ...SCRIPTS };
  const used = new Map(); // script → requests answered
  const requests = [];

  const fake = {
    url: "",
    port: 0,
    requests,
    models: [],
    modelsFail: false,
    /** add or replace a script */
    script(name, steps) {
      scripts[name] = steps;
      used.delete(name);
    },
    /** forget the recorded requests and where each script is */
    reset() {
      requests.length = 0;
      used.clear();
    },
    /** forget the recorded requests only (scripts carry on) */
    clear() {
      requests.length = 0;
    },
    /** recorded model calls (Messages and chat/completions), oldest first */
    calls: () => requests.filter((r) => /\/(messages|chat\/completions)$/.test(r.path)),
    close: () => new Promise((r) => server.close(r)),
  };

  // the step for a request: the script's next, or null (no marker)
  function next(name, req) {
    const steps = scripts[name];

    if (!steps) return { status: 400, error: `the fake has no script ${name}` };
    let k = used.get(name) || 0;
    used.set(name, k + 1);
    // walk past the steps whose `times` the earlier requests used up
    let step;

    for (const s of steps) {
      const n = s.times || 1;

      step = s;

      if (k < n) break;
      k -= n;
    }

    return typeof step === "function" ? step(req) : step;
  }

  const server = createServer((req, res) => {
    let raw = "";

    req.setEncoding("utf8");
    req.on("data", (c) => (raw += c));
    req.on("end", () => {
      let body = raw;

      try {
        body = raw ? JSON.parse(raw) : null;
      } catch {}

      const path = req.url;
      const headers = Object.fromEntries(HEADERS.filter((h) => req.headers[h] !== undefined).map((h) => [h, req.headers[h]]));
      const rec = { method: req.method, path, headers, body, stream: !!body?.stream };

      requests.push(rec);
      const json = (status, obj, extra = {}) => {
        res.writeHead(status, { "content-type": "application/json", ...extra });
        res.end(JSON.stringify(obj));
      };

      if (req.method === "GET" && path.endsWith("/v1/models")) {
        if (fake.modelsFail) return json(500, { error: { message: "models unavailable" } });

        return json(200, { object: "list", data: fake.models });
      }

      if (req.method === "POST" && /\/v1\/images\/(generations|edits)$/.test(path)) return json(200, { created: 1, data: [{ b64_json: PNG_B64 }] });

      const messages = path.endsWith("/v1/messages");
      const openai = path.endsWith("/v1/chat/completions");

      if (req.method !== "POST" || (!messages && !openai)) return json(404, { error: { message: `fake opencodex: no route ${req.method} ${path}` } });
      const msgs = body?.messages || [];
      const system = typeof body?.system === "string" ? body.system : openai ? msgs.find((m) => m.role === "system")?.content || "" : "";
      const name = markerOf(msgs);
      const classify = /^Classify the user message/.test(system);
      let step;

      if (classify) {
        const s = name && scripts[name];
        const first = Array.isArray(s) ? s.find((x) => x && typeof x === "object" && x.classify) : null;

        step = say(first?.classify || "UI");
      } else step = name ? next(name, rec) : say("ok");

      rec.script = name;

      if (step.status) {
        const err = { type: "error", error: { type: step.status === 429 ? "rate_limit_error" : step.status >= 500 ? "api_error" : "invalid_request_error", message: step.error || `status ${step.status}` } };

        return json(step.status, err, { "retry-after-ms": "1", ...(step.headers || {}) });
      }

      const model = body?.model || "";

      if (openai) {
        const text = (step.content || []).filter((b) => b.type === "text").map((b) => b.text).join("");

        return json(200, {
          id: "chatcmpl-fake",
          object: "chat.completion",
          model,
          choices: [{ index: 0, message: { role: "assistant", content: text }, finish_reason: step.stop_reason === "max_tokens" ? "length" : "stop" }],
        });
      }

      if (step.die) {
        if (rec.stream) {
          res.writeHead(200, { "content-type": "text/event-stream" });
          res.flushHeaders();

          return setTimeout(() => res.socket.destroy(), 20);
        }

        step = next(name, rec);
      }

      if (rec.stream) {
        res.writeHead(200, { "content-type": "text/event-stream", "cache-control": "no-cache" });

        return sse(res, step, model);
      }

      return json(200, finalMessage(step, model));
    });
  });

  await new Promise((resolve) => server.listen(port, "127.0.0.1", resolve));
  fake.port = server.address().port;
  fake.url = `http://127.0.0.1:${fake.port}`;

  return fake;
}
