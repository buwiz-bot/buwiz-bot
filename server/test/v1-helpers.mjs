// Shared by the v1 conformance tests (test/v1-*.test.mjs): a publisher key
// and trust file made per run, the fixture plugin's manifest signed with
// it, an in-process broker on an in-memory db with a file keystore, and a
// "fake Rox" that makes session keys and signs call envelopes exactly as
// spec 8.1 says. Nothing here touches the login Keychain or port 8787.
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { generate, random128, sign } from "../server/v1/ed.mjs";
import { signedBytes } from "../server/v1/jcs.mjs";
import { keystore } from "../server/v1/keystore.mjs";
import { permsDigest } from "../server/v1/manifest.mjs";
import { createPlugins, systemClock } from "../server/v1/plugins.mjs";
import { createSessions } from "../server/v1/sessions.mjs";
import { signManifest } from "../server/v1/sign-manifest.mjs";
import { loadTrust } from "../server/v1/trust.mjs";
import { ROOT, tempDir } from "./helpers.mjs";

export const FIXTURE_DIR = join(ROOT, "server", "v1", "fixture");
export const FIXTURE = join(FIXTURE_DIR, "server.mjs");
export const FX = "dev.goldcoders.fixture";

/** A publisher key of this run: {keyId, seed, pub}. */
export function publisher(keyId = `test-${random128().toLowerCase().replace(/[^a-z0-9]/g, "").slice(0, 10)}`) {
  return { keyId, ...generate() };
}

/** Writes a trust file trusting `pubs` and approving `[id, keyId]` pairs. */
export function writeTrust(file, pubs, approved) {
  const publishers = Object.fromEntries(pubs.map((p) => [p.keyId, { publisher: "dev.goldcoders", alg: "ed25519", public_key: p.pub }]));

  writeFileSync(file, JSON.stringify({ v: 1, publishers, approved_plugins: approved.map(([id, key]) => ({ id, publisher_key_id: key })) }));

  return file;
}

/** The fixture's manifest template, edited by `edit(m)` and signed by `pub`
 *  with the real app fingerprints. */
export function fixtureManifest(pub, edit = () => {}) {
  const m = JSON.parse(readFileSync(join(FIXTURE_DIR, "manifest.json"), "utf8"));

  m.publisher.key_id = pub.keyId;
  edit(m);

  return signManifest(m, pub.seed, {
    board: readFileSync(join(FIXTURE_DIR, "board.tsx")),
    card: readFileSync(join(FIXTURE_DIR, "card.tsx")),
  });
}

/** One test bed: a temp dir with a publisher, a trust file and a signed
 *  fixture manifest at `manifestFile`. */
export function bed(label) {
  const dir = tempDir(`v1-${label}`);
  const pub = publisher();
  const trustFile = writeTrust(join(dir, "trust.json"), [pub], [[FX, pub.keyId], ["dev.goldcoders.other", pub.keyId]]);
  const manifestFile = join(dir, "manifest.json");

  const writeManifest = (edit) => {
    const m = fixtureManifest(pub, edit);

    writeFileSync(manifestFile, JSON.stringify(m, null, 2));

    return m;
  };

  writeManifest();

  return { dir, pub, trustFile, manifestFile, writeManifest, keysDir: join(dir, "keys") };
}

/** An in-process broker for bed `b`: {plugins, sessions}. */
export function broker(b, { clock = systemClock, timeouts = {}, db = new DatabaseSync(":memory:") } = {}) {
  const plugins = createPlugins({ db, keys: keystore(`file:${b.keysDir}`), clock, logDir: join(b.dir, "logs"), trust: () => loadTrust(b.trustFile), timeouts });

  return { plugins, sessions: createSessions(plugins), db };
}

/** The fixture's spawn spec, with extra args (e.g. --misbehave=…) and env. */
export const fixtureSpec = (b, args = [], env = {}) => ({ command: process.execPath, args: [FIXTURE, ...args], env: { FX_MANIFEST: b.manifestFile, ...env } });

/** inspect + install, as Rox does after the review. */
export async function installFixture(plugins, b, args = [], env = {}) {
  const spec = fixtureSpec(b, args, env);
  const i = await plugins.inspect(spec);

  return plugins.install({ ...spec, manifest_sha256: i.manifest_sha256 });
}

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

/** The code a promise rejects with (a V1Error's), or "resolved". */
export const codeOf = (p) => p.then(() => "resolved", (e) => e.code || String(e));

// ── fake Rox (spec 4.4, 8.1) ────────────────────────────────────────────

/** Opens an app the way Rox does: a session key, an open_id, the
 *  permissions digest of the app in `manifest`. → the open, with its key. */
export async function openApp(sessions, manifest, app, { notAfter = Date.now() + 3600_000, perms } = {}) {
  const ks = generate();
  const a = manifest.apps.find((x) => x.id === app);
  const req = {
    plugin: manifest.id, app, open_id: random128(), ks_pub: ks.pub,
    perms_sha256: perms ?? permsDigest(a?.permissions), not_after: notAfter,
  };
  const res = await sessions.open(req);

  return { ...req, ks, res };
}

/** A signed call envelope of `open` (8.1); `over` replaces fields,
 *  `signer` signs instead of the open's session key. */
export function envelope(open, tool, args = {}, over = {}, signer = open.ks.seed) {
  const env = { v: 1, typ: "rox-call", plugin: open.plugin, app: open.app, open_id: open.open_id, tool, args, ts: Date.now(), nonce: random128(), ...over };

  return { envelope: env, sig: sign(signer, signedBytes(env)) };
}
