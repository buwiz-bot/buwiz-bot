// v1/line-host.mjs — a v1 plugin host of one's own: `rox-broker helper` on
// a free port, with createLinePlugins() and createLineSessions() over its
// line. For tests that want rox-broker hosting the plugins under an
// in-process broker (test/v1-helpers.mjs, and a plugin's own broker tests,
// such as BIR's v1-tests, through ROX_BROKER_DIR).
import { spawn } from "node:child_process";
import { DatabaseSync } from "node:sqlite";
import { random128 } from "./ed.mjs";
import { systemClock } from "./plugins.mjs";
import { createLinePlugins } from "./plugins-line.mjs";
import { createLineSessions } from "./sessions-line.mjs";
import { loadTrust } from "./trust.mjs";

/** `cmd` is a rox-broker binary; `db` the app.db file the two share;
 *  `keystore` as ROX_KEYSTORE takes it (`file:<dir>` for tests). A fake
 *  `clock` gives rox-broker a manual one. → {plugins, sessions, db};
 *  `plugins.stopAll()` also stops the helper. */
export function lineHost({ cmd, db: file, logDir, trustFile, keystore, clock = systemClock, timeouts = {}, env = {} }) {
  const db = new DatabaseSync(file, { timeout: 5000 });
  const token = random128();
  const childEnv = {
    ...process.env, ...env, ROX_LINE_TOKEN: token, ROX_TRUST_FILE: trustFile, ROX_KEYSTORE: keystore,
    ...(clock !== systemClock ? { ROX_V1_MANUAL_CLOCK: "1" } : {}),
    ...(timeouts.call ? { ROX_V1_CALL_TIMEOUT_MS: String(timeouts.call) } : {}),
  };
  const child = spawn(cmd, ["helper", "--db", file, ...(logDir ? ["--log-dir", logDir] : [])], { env: childEnv, stdio: ["ignore", "pipe", "inherit"] });
  const base = new Promise((resolve, reject) => {
    let out = "";

    child.stdout.on("data", (d) => {
      out += d;
      const m = /helper on (127\.0\.0\.1:\d+)/.exec(out);

      if (m) resolve(`http://${m[1]}`);
    });
    child.on("exit", (code) => reject(new Error(`rox-broker helper exited (${code})`)));
  });
  const plugins = createLinePlugins({ base, token, db, clock, trust: () => loadTrust(trustFile), timeouts });
  const stopAll = plugins.stopAll;

  plugins.stopAll = async () => {
    await stopAll();
    child.kill("SIGTERM");
  };

  return { plugins, sessions: createLineSessions(plugins), db };
}
