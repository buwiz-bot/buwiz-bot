// v1/trust.mjs — the trust store (spec 4.1): which publisher keys sign
// manifests and which plugins may be installed.
//
// Rox ships server/v1/trust.json; ROX_TRUST_FILE points at another file
// (every test run writes its own, with a key it generated). The file is
// read on every use, so an edit takes effect at the next verification and
// at the next look at the agent surface (approved-plugin text, 10.2).
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { isObject, isString, parseIJson } from "./jcs.mjs";
import { fromB64u } from "./ed.mjs";

export const TRUST_FILE = () => process.env.ROX_TRUST_FILE || join(dirname(fileURLToPath(import.meta.url)), "trust.json");

/** {publishers: Map key_id -> {publisher, public_key}, approved: [{id,
 *  publisher_key_id}]}. A missing or broken file trusts nothing. */
export function loadTrust(file = TRUST_FILE()) {
  let raw = {};

  try {
    raw = parseIJson(readFileSync(file, "utf8"));
  } catch (e) {
    console.error(`  v1 trust store ${file}: ${String(e?.message || e).slice(0, 200)}`);
  }

  const publishers = new Map();

  for (const [keyId, p] of Object.entries(isObject(raw.publishers) ? raw.publishers : {}))
    if (isObject(p) && p.alg === "ed25519" && fromB64u(p.public_key, 32)) publishers.set(keyId, { publisher: String(p.publisher || ""), public_key: p.public_key });

  const approved = (Array.isArray(raw.approved_plugins) ? raw.approved_plugins : []).flatMap((a) =>
    isObject(a) && isString(a.id) && isString(a.publisher_key_id) ? [{ id: a.id, publisher_key_id: a.publisher_key_id }] : []);

  return { publishers, approved };
}

/** Whether plugin `id` signed by `keyId` is on the approved list. */
export const isApproved = (trust, id, keyId) => trust.approved.some((a) => a.id === id && a.publisher_key_id === keyId);
