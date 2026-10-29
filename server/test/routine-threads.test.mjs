// A v1 routine's thread and its notices survive a restart: state.mjs
// persists the thread's `routine` mark, the notice turn and the item turn's
// `routine` mark, and turnsJson / threadBot hand them back (each step runs
// in its own node process on a temp data dir, as the server would).
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { test } from "node:test";
import { tempDir } from "./helpers.mjs";

const dir = tempDir("routine-threads");

mkdirSync(join(dir, "widgets"));
const run = (code) =>
  JSON.parse(
    execFileSync(process.execPath, ["--input-type=module", "-e", code], {
      cwd: new URL("..", import.meta.url).pathname,
      env: { ...process.env, GROK_DATA_DIR: dir, MOCK: "1" },
      encoding: "utf8",
    }).trim().split("\n").at(-1),
  );

test("a routine's thread, its item turns and the chat's notice persist", () => {
  const made = run(`
    import { store, nextThreadSeq, uuidv7, writeIndex } from "./server/state.mjs";
    import { createRoutineThreads } from "./server/v1/routine-threads.mjs";
    const chat = store.list[0];
    store.bots.push({ id: "b1", name: "Piccolo", shape: "blob", color: "#FFFFFF", threadId: chat.id });
    const threads = createRoutineThreads({ store, nextId: () => "t" + nextThreadSeq(), uid: uuidv7, save: writeIndex });
    const h = threads.ensure({ plugin: "dev.example.bir", routine: "payments", name: "BIR payments", chat: chat.id });
    h.turns.push({ id: uuidv7(), role: "user", text: "New item", routine: { plugin: "dev.example.bir", id: "payments", name: "BIR payments", run: "run_1" } });
    h.turns.push({ id: uuidv7(), role: "ai", text: "Recorded." });
    threads.notice(chat.id, { kind: "routine", plugin: "dev.example.bir", routine: "payments", name: "BIR payments", thread: h.id, thread_uid: h.uid, run: "run_1", seq: 0, turn: h.turns[0].id, items: 1 });
    writeIndex();
    console.log(JSON.stringify({ chat: chat.id, home: h.id, again: threads.ensure({ plugin: "dev.example.bir", routine: "payments", name: "BIR payments", chat: chat.id }).id }));
  `);

  assert.equal(made.again, made.home, "ensure reuses the thread");
  const back = run(`
    import { findThread, threadBot, turnsJson } from "./server/state.mjs";
    const h = findThread(${JSON.stringify(made.home)});
    console.log(JSON.stringify({ routine: h.routine, title: h.title, bot: threadBot(h.id)?.id, home: turnsJson(h), chat: turnsJson(findThread(${JSON.stringify(made.chat)})) }));
  `);

  assert.deepEqual(back.routine, { plugin: "dev.example.bir", id: "payments", name: "BIR payments", thread: made.chat });
  assert.equal(back.title, "BIR payments");
  assert.equal(back.bot, "b1", "it answers as the reporting chat's bot");
  assert.equal(back.home[0].routine.run, "run_1");
  const n = back.chat.at(-1);

  assert.equal(n.role, "notice");
  assert.equal(n.text, "BIR payments ran · 1 new item");
  assert.equal(n.notice.thread, made.home);
  assert.equal(n.notice.turn, back.home[0].id);
});
