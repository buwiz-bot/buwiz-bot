// v1/broker.mjs — the server's one v1 broker: plugins and sessions on the
// server's sqlite db, keys in the keystore ROX_KEYSTORE names (the login
// Keychain by default), plugin logs under <data dir>/v1/logs, and the v1
// routine runner, whose turns run the way v0's do (backgroundTurn in the
// routine's own chat). Importing it spawns nothing; app-server starts the
// enabled plugins and the routine poll once it listens.
// ROX_V1_CALL_TIMEOUT_MS shortens the broker's 120 s call limit for tests.
import { join } from "node:path";
import { DATA_DIR } from "../config.mjs";
import { allocJobId, db, findThread, modelCtx } from "../state.mjs";
import { keystore } from "./keystore.mjs";
import { createPlugins } from "./plugins.mjs";
import { createRoutines } from "./routines.mjs";
import { createSessions } from "./sessions.mjs";

const timeouts = {};

if (Number(process.env.ROX_V1_CALL_TIMEOUT_MS) > 0) timeouts.call = Number(process.env.ROX_V1_CALL_TIMEOUT_MS);

export const plugins = createPlugins({ db, keys: keystore(), logDir: join(DATA_DIR, "v1", "logs"), timeouts });
export const sessions = createSessions(plugins);

// A v1 routine's agent turn: v0's backgroundTurn in the routine's chat,
// with the job narrowing the v1 tools to the routine's surface (5.9).
// agent.mjs imports this module, hence the late imports.
async function routineTurn({ plugin, routine, thread, shown, prompt, surface }) {
  const [{ backgroundTurn }, { choiceFor }] = await Promise.all([import("../agent.mjs"), import("../model.mjs")]);
  const t = findThread(thread);

  if (!t) throw new Error("its chat was deleted");
  const job = { id: allocJobId(), thread: t.id, seq: 0, phase: "queued", detail: "", done: false, result: null, routine: `${plugin}/${routine}`, v1Surface: surface };

  await modelCtx.run(choiceFor(t.id), () => backgroundTurn(job, t, shown, prompt));
}

export const routines = createRoutines({ db, plugins, runTurn: routineTurn, threadExists: (id) => !!findThread(String(id || "")) });
