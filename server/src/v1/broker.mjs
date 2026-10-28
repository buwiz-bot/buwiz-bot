// v1/broker.mjs — the server's one v1 broker: plugins and sessions on the
// server's sqlite db, keys in the keystore ROX_KEYSTORE names (the login
// Keychain by default), plugin logs under <data dir>/v1/logs. Importing it
// spawns nothing; app-server starts the enabled plugins once it listens.
// ROX_V1_CALL_TIMEOUT_MS shortens the broker's 120 s call limit for tests.
import { join } from "node:path";
import { DATA_DIR } from "../config.mjs";
import { db } from "../state.mjs";
import { keystore } from "./keystore.mjs";
import { createPlugins } from "./plugins.mjs";
import { createSessions } from "./sessions.mjs";

const timeouts = {};

if (Number(process.env.ROX_V1_CALL_TIMEOUT_MS) > 0) timeouts.call = Number(process.env.ROX_V1_CALL_TIMEOUT_MS);

export const plugins = createPlugins({ db, keys: keystore(), logDir: join(DATA_DIR, "v1", "logs"), timeouts });
export const sessions = createSessions(plugins);
