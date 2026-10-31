// The plugin keystore after the rename: keys under com.buwiz.bot.plugin-key,
// Rox's moved there once. Each run makes a keychain file of its own (never
// on the search list) and test service names, so the login Keychain is
// never touched.
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { existsSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { KEYCHAIN_SERVICE, MOVED_MARKER, OLD_KEYCHAIN_SERVICE, keychainStore, keystore, moveFromRox, moveKeys } from "../src/v1/keystore.mjs";

const MAC = { skip: process.platform !== "darwin" && "the Keychain is macOS's" };
const ID = "dev.example.fixture";
const OTHER = "dev.example.other";
const NEWER = "dev.example.newer";
const seed = (b) => Buffer.alloc(32, b).toString("base64url");

test("the services are Buwiz Bot's, then Rox's; the marker is rox-broker's", () => {
  assert.equal(KEYCHAIN_SERVICE, "com.buwiz.bot.plugin-key");
  assert.equal(OLD_KEYCHAIN_SERVICE, "dev.goldcoders.rox.plugin-key");
  assert.equal(MOVED_MARKER, ".plugin-keys-moved-from-rox");
});

test("Rox's keys move to the new service once: read, written unless newer is there, deleted", MAC, async () => {
  const dir = mkdtempSync(join(tmpdir(), "rox-keystore-kc-"));
  const keychain = join(dir, "test.keychain-db");
  assert.ok(!keychain.includes(" "));
  const security = (...args) => execFileSync("/usr/bin/security", args, { stdio: ["ignore", "pipe", "ignore"] });
  security("create-keychain", "-p", "test", keychain);
  try {
    security("unlock-keychain", "-p", "test", keychain);
    const store = keychainStore({ service: `com.buwiz.bot.test-plugin-key.${process.pid}`, keychain });
    const old = keychainStore({ service: `dev.goldcoders.rox.test-plugin-key.${process.pid}`, keychain });
    for (const id of [ID, OTHER, NEWER]) await old.put(id, seed(9));
    await store.put(NEWER, seed(7));
    const ids = [ID, NEWER, "dev.example.none"];
    assert.equal(await moveKeys(old, store, ids), 1);
    assert.equal(await store.get(ID), seed(9));
    assert.equal(await store.get(NEWER), seed(7), "a key under the new service already is the newer one");
    assert.equal(await old.get(ID), null, "deleted from the old service");
    assert.equal(await old.get(NEWER), null);
    assert.equal(await old.get(OTHER), seed(9), "only the given plugins are touched");
    assert.equal(await moveKeys(old, store, ids), 0, "nothing left to move");
  } finally {
    security("delete-keychain", keychain);
    rmSync(dir, { recursive: true, force: true });
  }
});

test("a file store moves nothing, and the marker stops a second move", async () => {
  const dir = mkdtempSync(join(tmpdir(), "rox-keystore-marker-"));
  const marker = join(dir, MOVED_MARKER);
  try {
    assert.equal(await moveFromRox(keystore(`file:${dir}`), [ID], marker), 0);
    assert.ok(!existsSync(marker), "a file store writes no marker");
    writeFileSync(marker, "{}");
    // with the marker the Keychain is not even asked
    assert.equal(await moveFromRox(keystore("keychain"), [ID], marker), 0);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
