// v1/cli.mjs — whether a module is the script node was started with, for
// the v1 command-line tools (sign-manifest, publisher-keygen,
// buwiz-plugin-test).
//
// `process.argv[1]` is the path as typed, made absolute but not resolved
// through symlinks, while `import.meta.url` is the module's real path. Run
// through a symlinked folder (on macOS, anything under $TMPDIR: /var is a
// link to /private/var) the two differ, and a plain comparison makes the
// tool exit 0 having done nothing. Real paths on both sides.
import { realpathSync } from "node:fs";
import { fileURLToPath } from "node:url";

/** Whether the module at `url` (its `import.meta.url`) is node's main script. */
export function isMain(url) {
  if (!process.argv[1]) return false;

  try {
    return realpathSync(process.argv[1]) === realpathSync(fileURLToPath(url));
  } catch {
    return false;
  }
}
