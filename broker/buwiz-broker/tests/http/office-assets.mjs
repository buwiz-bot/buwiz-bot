// The office tests' static files, laid out as the release bundle lays them
// (scripts/release/build.sh): an assets folder `server/` holding
// `office/viewer.html`, `office/editor/editor.html` (and `dist/` once the
// editor is built), `office/seeds`, `plugins` and `trust.json`, and
// `node_modules/@betteroffice` beside it, which is where the broker looks
// for the engine packages the pages import. Assembled from office/ (after
// `npm ci` there) and broker/buwiz-broker/assets/, in a temp folder.
import { cpSync, existsSync, mkdirSync, symlinkSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { ASSETS, tempDir } from "./helpers.mjs";

export const REPO = join(dirname(fileURLToPath(import.meta.url)), "..", "..", "..", "..");
export const OFFICE = join(REPO, "office");
export const PACKAGES = join(OFFICE, "node_modules", "@betteroffice");

/** The assets folder of a bundle. `packages: false` leaves node_modules out
 *  (what a broker built without them sees); `bundle: true` adds a stand-in
 *  editor bundle (`office/editor/dist/editor.js`): the routes serve it as a
 *  static file, so no test needs the real build. */
export function officeAssets(label, { packages = true, bundle = false } = {}) {
  if (packages && !existsSync(join(PACKAGES, "docx", "package.json"))) throw new Error(`no @betteroffice packages in ${PACKAGES}: run npm ci in office/`);
  const root = tempDir(`office-assets-${label}`);
  const server = join(root, "server");
  const office = join(server, "office");

  mkdirSync(join(office, "editor"), { recursive: true });
  cpSync(join(OFFICE, "viewer.html"), join(office, "viewer.html"));
  cpSync(join(OFFICE, "editor", "editor.html"), join(office, "editor", "editor.html"));
  cpSync(join(OFFICE, "seeds"), join(office, "seeds"), { recursive: true });
  cpSync(join(ASSETS, "plugins"), join(server, "plugins"), { recursive: true });
  cpSync(join(ASSETS, "trust.json"), join(server, "trust.json"));

  if (packages) {
    mkdirSync(join(root, "node_modules"), { recursive: true });
    symlinkSync(PACKAGES, join(root, "node_modules", "@betteroffice"), "dir");
  }

  if (bundle) {
    mkdirSync(join(office, "editor", "dist"), { recursive: true });
    writeFileSync(join(office, "editor", "dist", "editor.js"), "export {};\n");
  }

  return { assets: server, bundle: join(office, "editor", "dist", "editor.js") };
}
