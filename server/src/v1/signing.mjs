// v1/signing.mjs — app fingerprints and manifest signatures (6.2, 4.2), as
// a library. The server imports this, never the sign-manifest CLI: a
// bundled server is one file, so a CLI's "am I the main script" check would
// be true in it and run the CLI instead of the server.
import { createHash } from "node:crypto";
import { sign } from "./ed.mjs";
import { signedBytes } from "./jcs.mjs";

export const PUBLISHER_SERVICE = "dev.goldcoders.publisher";

/** `sha256:<hex>` of an app source's bytes. */
export const fingerprintOf = (bytes) => `sha256:${createHash("sha256").update(bytes).digest("hex")}`;

/** A signed copy of manifest `m`: `sources` maps app id → source bytes
 *  whose fingerprints are filled in first; the key id is the manifest's
 *  publisher.key_id. */
export function signManifest(m, seed, sources = {}) {
  const out = structuredClone(m);

  delete out.signature;

  for (const [id, bytes] of Object.entries(sources)) {
    const app = (out.apps || []).find((a) => a.id === id);

    if (!app) throw new Error(`the manifest has no app ${id}`);
    app.fingerprint = fingerprintOf(bytes);
  }

  const keyId = out.publisher?.key_id;

  if (!keyId) throw new Error("the manifest has no publisher.key_id");

  out.signature = { alg: "ed25519", key_id: keyId, value: sign(seed, signedBytes(out)) };

  return out;
}
