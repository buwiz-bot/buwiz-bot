// v1/broker.mjs — the server's one v1 broker: plugins and sessions on the
// server's sqlite db, keys in the keystore BUWIZ_KEYSTORE names (the login
// Keychain by default), plugin logs under <data dir>/v1/logs, and the v1
// routine runner, whose turns run the way v0's do (backgroundTurn), in
// the routine's own thread (routine-threads.mjs). Importing it spawns nothing; app-server starts the
// enabled plugins and the routine poll once it listens.
// ROX_V1_CALL_TIMEOUT_MS shortens the broker's 120 s call limit for tests.
import { join } from "node:path";
import { DATA_DIR, GEN_DIR, OFFICE_ROLE } from "../config.mjs";
import { allocJobId, db, findThread, modelCtx, nextThreadSeq, store, uuidv7, writeIndex } from "../state.mjs";
import { MOVED_MARKER, keystore, moveFromRox } from "./keystore.mjs";
import { createPlugins } from "./plugins.mjs";
import { LINE } from "./line-env.mjs";
import { createLinePlugins } from "./plugins-line.mjs";
import { createRoutineThreads } from "./routine-threads.mjs";
import { createRoutines } from "./routines.mjs";
import { createSessions } from "./sessions.mjs";
import { createLineSessions } from "./sessions-line.mjs";

const timeouts = {};

if (Number(process.env.ROX_V1_CALL_TIMEOUT_MS) > 0) timeouts.call = Number(process.env.ROX_V1_CALL_TIMEOUT_MS);

// rox-broker hosts the plugins when it was started with ROX_BROKER_V1_HOST=1:
// it hands Node the line's address and token, and "v1" in ROX_LINE_HOSTS
const onLine = !!LINE.base && LINE.hosts.includes("v1");

export const plugins = onLine
  ? createLinePlugins({ base: LINE.base, token: LINE.token, db, timeouts })
  : createPlugins({ db, keys: keystore(), logDir: join(DATA_DIR, "v1", "logs"), timeouts });

// Rox's plugin keys move to the new Keychain service (once) before any
// plugin starts; on the line, rox-broker moves them
if (!onLine) {
  const keys = keystore();
  const ids = db.prepare("SELECT id FROM v1_plugins").all().map((r) => r.id);
  const moved = moveFromRox(keys, ids, join(GEN_DIR, MOVED_MARKER))
    .then((n) => n && console.log(`  keystore: ${n} plugin key(s) moved from the old service`))
    .catch((e) => console.error(`  keystore: plugin keys not moved from the old service yet: ${e.message}`));
  const startAll = plugins.startAll;

  plugins.startAll = () => moved.then(startAll);
}
export const sessions = onLine ? createLineSessions(plugins) : createSessions(plugins);

// A v1 routine's agent turn: v0's backgroundTurn in the routine's thread,
// with the job narrowing the v1 tools to the routine's surface (5.9). The
// shown turn carries the run's `routine` mark (the client captions it
// "Instruction from <name>"). → where it landed, {seq, turn}.
// agent.mjs imports this module, hence the late imports.
async function routineTurn({ plugin, routine, thread, shown, prompt, surface, mark }) {
  const [{ backgroundTurn }, { choiceFor }] = await Promise.all([import("../agent.mjs"), import("../model.mjs")]);
  const t = findThread(thread);

  if (!t) throw new Error("its thread was deleted");
  const job = { id: allocJobId(), thread: t.id, seq: 0, phase: "queued", detail: "", done: false, result: null, routine: `${plugin}/${routine}`, v1Surface: surface };
  const at = { seq: t.turns.length, turn: uuidv7() };

  await modelCtx.run(choiceFor(t.id), () => backgroundTurn(job, t, shown, prompt, { id: at.turn, routine: mark }));

  return at;
}

const threads = createRoutineThreads({ store, nextId: () => `t${nextThreadSeq()}`, uid: uuidv7, save: writeIndex });

// Not in Node's office role (ROX_NODE_ROLE=office): rox-broker owns
// v1_routines and v1_instances there, and createRoutines would write them
// on import (the one-time move to instances, racing rox-broker's on an
// older database) and on a plugin's uninstall. Nothing in the role reaches
// the routines: their routes answer 501 and app-server never starts them.
export const routines = OFFICE_ROLE ? null : createRoutines({ db, plugins, runTurn: routineTurn, threads, threadExists: (id) => !!findThread(String(id || "")) });
