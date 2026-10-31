// The plugin keystore's Keychain backend after the rename: keys under
// com.buwiz.bot.plugin-key, read through to Rox's service. Each run makes a
// keychain file of its own (never on the search list) and test service
// names, so the login Keychain is never touched.
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { KEYCHAIN_SERVICE, OLD_KEYCHAIN_SERVICE, keychainStore } from "../src/v1/keystore.mjs";

const MAC = { skip: process.platform !== "darwin" && "the Keychain is macOS's" };
const ID = "dev.example.fixture";
const seed = (b) => Buffer.alloc(32, b).toString("base64url");

test("the services are Buwiz Bot's, then Rox's", () => {
  assert.equal(KEYCHAIN_SERVICE, "com.buwiz.bot.plugin-key");
  assert.equal(OLD_KEYCHAIN_SERVICE, "dev.goldcoders.rox.plugin-key");
});

test("a key only under Rox's service is read, copied over and kept; a remove deletes both", MAC, async () => {
  const dir = mkdtempSync(join(tmpdir(), "rox-keystore-kc-"));
  const keychain = join(dir, "test.keychain-db");
  assert.ok(!keychain.includes(" "));
  const security = (...args) => execFileSync("/usr/bin/security", args, { stdio: ["ignore", "pipe", "ignore"] });
  security("create-keychain", "-p", "test", keychain);
  try {
    security("unlock-keychain", "-p", "test", keychain);
    const service = `com.buwiz.bot.test-plugin-key.${process.pid}`;
    const old = `dev.goldcoders.rox.test-plugin-key.${process.pid}`;
    const ks = keychainStore({ service, old, keychain });
    const find = (at) => keychainStore({ service: at, old: at, keychain }).get(ID);
    assert.equal(await ks.get(ID), null);
    await keychainStore({ service: old, old, keychain }).put(ID, seed(9));
    assert.equal(await find(service), null);
    assert.equal(await ks.get(ID), seed(9));
    assert.equal(await find(service), seed(9), "copied to the new service");
    assert.equal(await find(old), seed(9), "the old item stays for a downgrade");
    // the new service wins
    await ks.put(ID, seed(7));
    assert.equal(await ks.get(ID), seed(7));
    await ks.remove(ID);
    assert.equal(await find(service), null);
    assert.equal(await find(old), null);
  } finally {
    security("delete-keychain", keychain);
    rmSync(dir, { recursive: true, force: true });
  }
});
