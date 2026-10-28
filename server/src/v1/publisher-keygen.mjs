#!/usr/bin/env node
// v1/publisher-keygen.mjs — make a publisher signing key (spec 4.1, 4.2).
//
//   node server/v1/publisher-keygen.mjs gc-2026-01 [--publisher dev.goldcoders]
//
// Generates an ed25519 key, stores the seed in the macOS login Keychain
// (service dev.goldcoders.publisher, account = key id) through
// `security -i` with the command on stdin, so the seed is never in argv,
// and prints the public key with the trust.json entry to paste. Refuses
// when the Keychain already holds that key id: rotating a key is a new id.
import { execFileSync, spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { isV1 } from "../grammar.mjs";
import { generate } from "./ed.mjs";
import { PUBLISHER_SERVICE } from "./sign-manifest.mjs";

function exists(keyId) {
  try {
    execFileSync("/usr/bin/security", ["find-generic-password", "-s", PUBLISHER_SERVICE, "-a", keyId], { stdio: "ignore" });

    return true;
  } catch {
    return false;
  }
}

function cli(argv) {
  const keyId = argv[0] || "";
  const at = argv.indexOf("--publisher");
  const publisher = at >= 0 ? argv[at + 1] : "dev.goldcoders";

  if (!isV1("keyId", keyId)) throw new Error("usage: publisher-keygen.mjs <key_id> [--publisher <publisher id>]");

  if (exists(keyId)) throw new Error(`the Keychain already holds ${PUBLISHER_SERVICE} / ${keyId}`);

  const { seed, pub } = generate();

  const r = spawnSync("/usr/bin/security", ["-i"], {
    input: `add-generic-password -s ${PUBLISHER_SERVICE} -a ${keyId} -w ${seed}\nquit\n`,
    stdio: ["pipe", "ignore", "pipe"],
  });

  if (r.status !== 0 || String(r.stderr).trim()) throw new Error(`security: ${String(r.stderr).trim().slice(0, 200) || `exit ${r.status}`}`);

  if (!exists(keyId)) throw new Error("the Keychain did not keep the key");

  console.log(`public key ${keyId}: ${pub}`);
  console.log("trust.json publishers entry:");
  console.log(JSON.stringify({ [keyId]: { publisher, alg: "ed25519", public_key: pub } }, null, 2));
}

if (process.argv[1] === fileURLToPath(import.meta.url))
  try {
    cli(process.argv.slice(2));
  } catch (e) {
    console.error(`publisher-keygen: ${String(e?.message || e).slice(0, 300)}`);
    process.exit(1);
  }
