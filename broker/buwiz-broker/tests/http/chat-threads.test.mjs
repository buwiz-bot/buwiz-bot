// Several chats answer at once: /chat names its thread, the job and its
// turns stay there, and two jobs in different threads never cross.
import assert from "node:assert/strict";
import { after, before, test } from "node:test";
import { postJson, request, startServer, tempDir, waitFor } from "./helpers.mjs";

let server;

before(async () => {
  server = await startServer(tempDir("chat-threads"));
});

after(async () => {
  await server?.stop();
});

const json = async (r) => r.json;
const turns = async (id) => (await json(await request("GET", `/turns?thread=${id}`))).turns;

test("two chats with jobs in flight at once each get their own reply", async () => {
  const a = (await json(await postJson("/thread/new", {}))).id;
  const b = (await json(await postJson("/thread/new", {}))).id;

  assert.notEqual(a, b);
  // b is the server's active thread now; a's job must still land in a
  const [ja, jb] = await Promise.all([
    postJson("/chat", { prompt: "question for A", thread: a }).then(json),
    postJson("/chat", { prompt: "question for B", thread: b }).then(json),
  ]);

  assert.ok(ja.ok && jb.ok);
  const replied = async (id) => {
    const t = await turns(id);

    return t.length >= 2 && t[t.length - 1].role === "ai" && !t[t.length - 1].building;
  };

  await waitFor(async () => (await replied(a)) && (await replied(b)), 60000);
  const ta = await turns(a);
  const tb = await turns(b);

  assert.deepEqual(ta.filter((t) => t.role === "user").map((t) => t.text), ["question for A"]);
  assert.deepEqual(tb.filter((t) => t.role === "user").map((t) => t.text), ["question for B"]);
  assert.ok(!JSON.stringify(ta).includes("question for B") && !JSON.stringify(tb).includes("question for A"), "never crossed");
});

test("a /chat naming an unknown thread is refused; one naming none uses the active thread", async () => {
  const r = await postJson("/chat", { prompt: "x", thread: "t9999" });

  assert.equal(r.status, 400);
  const active = (await json(await request("GET", "/turns"))).active;
  const j = await json(await postJson("/chat", { prompt: "no thread named" }));

  assert.ok(j.ok);
  await waitFor(async () => (await turns(active)).some((t) => t.text === "no thread named"), 60000);
});
