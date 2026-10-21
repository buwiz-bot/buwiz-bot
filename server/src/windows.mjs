// windows.mjs — pop-out and desktop windows: screen geometry, desktop
// column slots, raising widget windows, and the off-screen snapshot render.
import { execFile, execFileSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, unlinkSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { GEN_DIR, NSH, NSH_HOST, WIDGETS_DIR, WINDOWS_DIR } from "./config.mjs";
import { deskSlots, openWindows, shotCache, shotInflight } from "./state.mjs";

let deskBounds = null;
export const desktopBounds = () => {
  if (deskBounds) return deskBounds;
  try {
    const s = execFileSync(
      "osascript",
      ["-e", 'tell application "Finder" to get bounds of window of desktop'],
      { encoding: "utf8" },
    ).trim();
    const [, , w, h] = s.split(",").map(Number);
    deskBounds = { w: w || 1440, h: h || 900 };
  } catch {
    deskBounds = { w: 1440, h: 900 };
  }
  return deskBounds;
};
export const allocDeskSlot = (name) => {
  if (deskSlots.has(name)) return deskSlots.get(name);
  for (let i = 0; ; i++)
    if (![...deskSlots.values()].includes(i)) {
      deskSlots.set(name, i);
      return i;
    }
};
// Raise an already-open pop-out: find its window by title and pull it onto
// the focused workspace. AeroSpace-only (no-op without it).
export const raiseWidget = (title) => {
  try {
    const out = execFileSync(
      "aerospace",
      ["list-windows", "--all", "--format", "%{window-id} %{window-title}"],
      { encoding: "utf8" },
    );
    const line = out.split("\n").find((l) => l.includes(title));
    if (!line) return;
    const wid = line.trim().split(/\s+/)[0];
    const ws = execFileSync(
      "aerospace", ["list-workspaces", "--focused"], { encoding: "utf8" },
    ).trim();
    execFileSync("aerospace", [
      "move-node-to-workspace", "--window-id", wid, "--focus-follows-window", ws,
    ]);
    execFileSync("aerospace", ["focus", "--window-id", wid]);
  } catch {}
};

// Render a mini-app to a PNG for the WidgetKit extension. A snapshot
// wrapper parks the host window offscreen at the desktop level — even if
// the tiling WM manages it, it stays behind every window, so nothing
// flashes on screen — then --script waits for the mount, shots the
// window's content in-process (cacheDisplayInRect: needs no Screen
// Recording permission) and quits. Serialized per name, cached ~45 s.
export const renderShot = (name, w, h) => {
  const cached = shotCache.get(name);
  if (cached && Date.now() - cached.t < 45_000) return Promise.resolve(cached.png);
  const running = shotInflight.get(name);
  if (running) return running;
  const p = new Promise((resolve) => {
    const widgetFile = join(WIDGETS_DIR, `${name}.tsx`);
    const genFile = join(GEN_DIR, `${name}.tsx`);
    const file = existsSync(widgetFile)
      ? widgetFile
      : existsSync(genFile)
        ? genFile
        : null;
    if (!file) return resolve(null);
    const rel =
      file === widgetFile ? `../widgets/${name}` : `../generated/${name}`;
    const snap = join(WINDOWS_DIR, `snap-${name}.tsx`);
    const png = join(WINDOWS_DIR, `.shot-${name}.png`);
    writeFileSync(
      snap,
      `import { mount, View } from "nsh";
import { isAvailable, Window } from "nsh/desktop";
import { colorScheme } from "nsh/device";
import R from "${rel}";
// Snapshot surface: parked offscreen at the desktop-icon level so the
// tiling WM can never flash it on screen; sized exactly to the widget
// slot so the module lays out the way it will be shown.
if (isAvailable) {
  Window.setTitle("snap · " + ${JSON.stringify(name)});
  Window.setLevel("desktopIcon");
  Window.setFrame({ x: -6000, y: -6000, width: ${w}, height: ${h} });
}
mount(() => (
  <View
    style={{
      flex: 1,
      backgroundColor: colorScheme() === "dark" ? "#161616" : "#ffffff",
      overflow: "hidden",
    }}
  >
    <R />
  </View>
));
`,
    );
    try {
      unlinkSync(png);
    } catch {}
    // Bundle once, then launch the host straight on it — ~2 s end to end,
    // fast enough for WidgetKit's timeline budget (nsh run's dev-server
    // session took ~19 s and the extension's load got killed).
    const outDir = join(WINDOWS_DIR, `.snap-${name}`);
    const host = NSH_HOST;
    execFile(
      NSH,
      ["build", snap, "-o", outDir],
      { timeout: 30_000 },
      (buildErr) => {
        if (buildErr) return resolve(null);
        execFile(
          host,
          [
            "--bundle",
            join(outDir, "bundle.nshc"),
            "--name",
            `snap · ${name}`,
            "--window-size",
            `${w}x${h}`,
            // Mount → settle → capture → exit. The host runs the steps
            // once the window shows; quit ends the process.
            "--script",
            `wait 1500; shot ${png}; quit`,
          ],
          { timeout: 30_000 },
          () => {
            try {
              const buf = readFileSync(png);
              shotCache.set(name, { t: Date.now(), png: buf });
              console.log(`  /widget-shot ${name} → ${buf.length} B`);
              resolve(buf);
            } catch {
              resolve(null);
            }
          },
        );
      },
    );
  }).finally(() => shotInflight.delete(name));
  shotInflight.set(name, p);
  return p;
};
mkdirSync(WIDGETS_DIR, { recursive: true });
