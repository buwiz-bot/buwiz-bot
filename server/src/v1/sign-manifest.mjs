#!/usr/bin/env node
// v1/sign-manifest.mjs — sign a plugin manifest with a publisher key (4.2).
//
//   node server/v1/sign-manifest.mjs manifest.json --keychain gc-2026-01 \
//        --app forms=dist/forms.tsx --app calendar=dist/calendar.tsx [--out signed.json]
//   node server/v1/sign-manifest.mjs manifest.json --key-file test.key ...
//
// Fills each named app's `fingerprint` from its source bytes (6.2), then
// `signature` = ed25519 over the JCS bytes of the manifest without it. The
// key comes from the macOS login Keychain (service dev.goldcoders.publisher,
// account = key id) or from a 0600 file holding the base64url seed (test
// keys). The seed is never printed; the signed manifest goes to --out or
// stdout.
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { readFileSync, statSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { sign } from "./ed.mjs";
import { isObject, parseIJson, signedBytes } from "./jcs.mjs";

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

// A key file must be the owner's alone, as ssh insists for private keys.
function readKeyFile(path) {
  if (statSync(path).mode & 0o077) throw new Error(`${path} is readable by others; chmod 600 it`);

  return readFileSync(path, "utf8").trim();
}

const readKeychain = (keyId) =>
  execFileSync("/usr/bin/security", ["find-generic-password", "-s", PUBLISHER_SERVICE, "-a", keyId, "-w"], { encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] }).trim();

function cli(argv) {
  const opts = { apps: {}, file: "", out: "", keyFile: "", keychain: "" };

  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];

    if (a === "--key-file") opts.keyFile = argv[++i];
    else if (a === "--keychain") opts.keychain = argv[++i];
    else if (a === "--out") opts.out = argv[++i];
    else if (a === "--app") {
      const [id, path] = String(argv[++i]).split(/=(.*)/s);

      opts.apps[id] = readFileSync(path);
    } else opts.file = a;
  }

  if (!opts.file || (!opts.keyFile === !opts.keychain)) throw new Error("usage: sign-manifest.mjs <manifest.json> (--key-file <path> | --keychain <key_id>) [--app <id>=<file>]... [--out <file>]");

  const m = parseIJson(readFileSync(opts.file, "utf8"));

  if (!isObject(m)) throw new Error("the manifest must be a JSON object");

  if (opts.keychain && m.publisher?.key_id !== opts.keychain) throw new Error(`the manifest's publisher.key_id is ${m.publisher?.key_id}, not ${opts.keychain}`);

  const seed = opts.keyFile ? readKeyFile(opts.keyFile) : readKeychain(opts.keychain);
  const text = `${JSON.stringify(signManifest(m, seed, opts.apps), null, 2)}\n`;

  if (opts.out) writeFileSync(opts.out, text);
  else process.stdout.write(text);
}

if (process.argv[1] === fileURLToPath(import.meta.url))
  try {
    cli(process.argv.slice(2));
  } catch (e) {
    console.error(`sign-manifest: ${String(e?.message || e).slice(0, 300)}`);
    process.exit(1);
  }
