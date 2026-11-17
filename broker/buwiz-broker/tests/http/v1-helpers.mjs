// Shared by the plugin-host HTTP tests: a publisher key and trust file made
// per run, the fixture plugin's manifest signed with it (by
// `buwiz-broker sign-manifest`), and the signed call envelopes the app
// sends. Nothing here touches the login Keychain.
import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { generate, random128, sign } from "../fixtures/v1/ed.mjs";
import { canonicalSigned, signedBytes } from "../fixtures/v1/jcs.mjs";
import { BROKER, tempDir } from "./helpers.mjs";

export const FIXTURE_DIR = join(dirname(fileURLToPath(import.meta.url)), "..", "fixtures", "v1");
export const FIXTURE = join(FIXTURE_DIR, "server.mjs");
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

/** `manifest` (an object) signed by `pub`, the apps' fingerprints filled
 *  from `sources` ({app id: file}). */
export function signManifest(manifest, pub, sources = {}, dir = tempDir("v1-sign")) {
  const input = join(dir, "unsigned.json");
  const key = join(dir, "publisher.key");
  const out = join(dir, "signed.json");

  writeFileSync(input, JSON.stringify(manifest));
  writeFileSync(key, `${pub.seed}\n`, { mode: 0o600 });
  execFileSync(BROKER, ["sign-manifest", input, "--key-file", key, ...Object.entries(sources).flatMap(([id, file]) => ["--app", `${id}=${file}`]), "--out", out], { stdio: "pipe" });

  return JSON.parse(readFileSync(out, "utf8"));
}

/** The fixture's manifest template, edited by `edit(m)` and signed by `pub`
 *  with the real app fingerprints. */
export function fixtureManifest(pub, edit = () => {}) {
  const m = JSON.parse(readFileSync(join(FIXTURE_DIR, "manifest.json"), "utf8"));

  m.publisher.key_id = pub.keyId;
  edit(m);

  const sources = {};

  for (const id of ["board", "card"]) if ((m.apps || []).some((a) => a.id === id)) sources[id] = join(FIXTURE_DIR, `${id}.tsx`);

  return signManifest(m, pub, sources);
}

/** One test bed: a temp dir with a publisher, a trust file and a signed
 *  fixture manifest at `manifestFile`. */
export function bed(label) {
  const dir = tempDir(`v1-${label}`);
  const pub = publisher();
  const trustFile = writeTrust(join(dir, "trust.json"), [pub], [[FX, pub.keyId], ["app.buwiz.other", pub.keyId]]);
  const manifestFile = join(dir, "manifest.json");

  const writeManifest = (edit) => {
    const m = fixtureManifest(pub, edit);

    writeFileSync(manifestFile, JSON.stringify(m, null, 2));

    return m;
  };

  writeManifest();

  return { dir, pub, trustFile, manifestFile, writeManifest, keysDir: join(dir, "keys") };
}

/** The fixture's spawn spec, with extra args (e.g. --misbehave=…) and env. */
export const fixtureSpec = (b, args = [], env = {}) => ({ command: process.execPath, args: [FIXTURE, ...args], env: { FX_MANIFEST: b.manifestFile, ...env } });

/** Waits until `check()` is truthy; its value, or throws after `ms`. */
export async function until(check, ms = 10000, what = "condition") {
  const end = Date.now() + ms;

  while (Date.now() < end) {
    const v = await check();

    if (v) return v;
    await new Promise((r) => setTimeout(r, 20));
  }

  throw new Error(`timed out waiting for ${what}`);
}

/** The permissions digest of an app (spec 8.1): sha256 of the JCS bytes. */
export const permsDigest = (perms) => createHash("sha256").update(canonicalSigned(perms ?? {})).digest("hex");

/** A signed call envelope of `open` (8.1); `over` replaces fields,
 *  `signer` signs instead of the open's session key. */
export function envelope(open, tool, args = {}, over = {}, signer = open.ks.seed) {
  const env = { v: 1, typ: "buwiz-call", plugin: open.plugin, app: open.app, open_id: open.open_id, tool, args, actor: "person", ts: Date.now(), nonce: random128(), ...over };

  return { envelope: env, sig: sign(signer, signedBytes(env)) };
}
