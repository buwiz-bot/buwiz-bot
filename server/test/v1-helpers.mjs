// Shared by v1-manifest.test.mjs: a publisher key and trust file made per
// run, and the fixture plugin's manifest signed with the publisher's key.
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { generate, random128 } from "../src/v1/ed.mjs";
import { signManifest } from "../src/v1/sign-manifest.mjs";
import { ROOT } from "./helpers.mjs";

export const FIXTURE_DIR = join(ROOT, "..", "broker", "buwiz-broker", "tests", "fixtures", "v1");
export const FX = "app.buwiz.fixture";

/** A publisher key of this run: {keyId, seed, pub}. */
export function publisher(keyId = `test-${random128().toLowerCase().replace(/[^a-z0-9]/g, "").slice(0, 10)}`) {
  return { keyId, ...generate() };
}

/** Writes a trust file trusting `pubs` and approving `[id, keyId]` pairs. */
export function writeTrust(file, pubs, approved) {
  const publishers = Object.fromEntries(pubs.map((p) => [p.keyId, { publisher: "app.buwiz", alg: "ed25519", public_key: p.pub, ...(p.firstParty ? { first_party: true } : {}) }]));

  writeFileSync(file, JSON.stringify({ v: 1, publishers, approved_plugins: approved.map(([id, key]) => ({ id, publisher_key_id: key })) }));

  return file;
}

/** The fixture's manifest template, edited by `edit(m)` and signed by `pub`
 *  with the real app fingerprints. */
export function fixtureManifest(pub, edit = () => {}) {
  const m = JSON.parse(readFileSync(join(FIXTURE_DIR, "manifest.json"), "utf8"));

  m.publisher.key_id = pub.keyId;
  edit(m);

  const sources = {};

  for (const id of ["board", "card"]) if ((m.apps || []).some((a) => a.id === id)) sources[id] = readFileSync(join(FIXTURE_DIR, `${id}.tsx`));

  return signManifest(m, pub.seed, sources);
}
