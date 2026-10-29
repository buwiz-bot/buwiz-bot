// windows.mjs — pop-out windows: screen geometry and raising an
// already-open pop-out.
import { execFileSync } from "node:child_process";
import { mkdirSync } from "node:fs";
import { WIDGETS_DIR } from "./config.mjs";

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

mkdirSync(WIDGETS_DIR, { recursive: true });
