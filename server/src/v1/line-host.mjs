// v1/line-host.mjs — a v1 plugin host of one's own: `buwiz-broker helper` on
// a free port, with createLinePlugins() and createLineSessions() over its
// line. For tests that want buwiz-broker hosting the plugins under an
// in-process broker (test/v1-helpers.mjs, and a plugin's own broker tests,
// such as BIR's v1-tests, through BUWIZ_BROKER_DIR).
import { spawn } from "node:child_process";
import { both } from "../env.mjs";
import { DatabaseSync } from "node:sqlite";
import { random128 } from "./ed.mjs";
import { systemClock } from "./plugins.mjs";
import { createLinePlugins } from "./plugins-line.mjs";
import { createLineSessions } from "./sessions-line.mjs";
import { loadTrust } from "./trust.mjs";

/** `cmd` is a buwiz-broker binary; `db` the app.db file the two share;
 *  `keystore` as BUWIZ_KEYSTORE takes it (`file:<dir>` for tests). A fake
 *  `clock` gives buwiz-broker a manual one. → {plugins, sessions, db};
 *  `plugins.stopAll()` also stops the helper. */
export function lineHost({ cmd, db: file, logDir, trustFile, keystore, clock = systemClock, timeouts = {}, env = {} }) {
  const db = new DatabaseSync(file, { timeout: 5000 });
  const token = random128();
  const childEnv = {
    // by their old names too: `cmd` may be a buwiz-broker from before the rename
    ...process.env, ...env, ...both("BUWIZ_LINE_TOKEN", token), BUWIZ_TRUST_FILE: trustFile, BUWIZ_KEYSTORE: keystore,
    ...(clock !== systemClock ? both("BUWIZ_V1_MANUAL_CLOCK", "1") : {}),
    ...(timeouts.call ? both("BUWIZ_V1_CALL_TIMEOUT_MS", String(timeouts.call)) : {}),
  };
  const child = spawn(cmd, ["helper", "--db", file, ...(logDir ? ["--log-dir", logDir] : [])], { env: childEnv, stdio: ["ignore", "pipe", "inherit"] });
  const base = new Promise((resolve, reject) => {
    let out = "";

    child.stdout.on("data", (d) => {
      out += d;
      const m = /helper on (127\.0\.0\.1:\d+)/.exec(out);

      if (m) resolve(`http://${m[1]}`);
    });
    child.on("exit", (code) => reject(new Error(`buwiz-broker helper exited (${code})`)));
  });
  const plugins = createLinePlugins({ base, token, db, clock, trust: () => loadTrust(trustFile), timeouts });
  const stopAll = plugins.stopAll;

  plugins.stopAll = async () => {
    await stopAll();
    child.kill("SIGTERM");
  };

  return { plugins, sessions: createLineSessions(plugins), db };
}
