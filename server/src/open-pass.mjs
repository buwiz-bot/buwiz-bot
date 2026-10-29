// open-pass.mjs — one-time passes for the app links the server opens.
//
// A mini app opens another app through POST /mcp/open, which hands a
// `rox://app/…` link to the system; Rox receives it like a link from any
// other app or web page and would ask first. The server adds a pass to the
// links it opens itself, and Rox redeems it here: a valid pass means the
// link came from this server (web pages are refused at dispatch, guard.mjs),
// so Rox opens the app without asking. A pass works once, within PASS_MS.

import { randomBytes } from "node:crypto";

const PASS_MS = 15_000;
const passes = new Map(); // pass -> expiry (ms since epoch)

/** `url` with a fresh one-time pass added as `rox_pass`. */
export function withPass(url, now = Date.now()) {
  for (const [p, until] of passes) if (until <= now) passes.delete(p);
  const pass = randomBytes(16).toString("base64url");
  passes.set(pass, now + PASS_MS);

  return url + (url.includes("?") ? "&" : "?") + "rox_pass=" + pass;
}

/** Whether `pass` was issued and is still unused and fresh; uses it up. */
export function redeem(pass, now = Date.now()) {
  const until = passes.get(String(pass || ""));
  passes.delete(String(pass || ""));

  return until !== undefined && until > now;
}
