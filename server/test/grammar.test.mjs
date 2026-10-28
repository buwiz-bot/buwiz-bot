import assert from "node:assert/strict";
import { test } from "node:test";
import { isName, isServerId, parseToken, tokensIn } from "../server/grammar.mjs";

test("a token parses into server, scope and value", () => {
  assert.deepEqual(parseToken("@bir:tin:123-456-789-000"), { server: "bir", scope: "tin", value: "123-456-789-000" });
  assert.deepEqual(parseToken("  @my-srv-2:Tax_Profile:a:b/c  "), { server: "my-srv-2", scope: "Tax_Profile", value: "a:b/c" });
  assert.deepEqual(parseToken("@b:s:x"), { server: "b", scope: "s", value: "x" });
});

test("tokens outside the grammar are refused", () => {
  for (const bad of [
    "@bir:tin:", // empty value
    "@Bir:tin:1", // upper-case server
    "@bir_x:tin:1", // underscore in server
    "@bir:bad.scope:1", // dot in scope
    "@bir:tin:1 2", // whitespace in value
    "bir:tin:1", // no @
    "@bir:tin", // no value part
    "@:tin:1", // empty server
    "@bir::1", // empty scope
    "",
    null,
  ])
    assert.equal(parseToken(bad), null, String(bad));
});

test("tokens are found in free text", () => {
  assert.deepEqual(tokensIn("see @bir:tin:123/0000 and @cal:day:2026-10-28, ok"), [
    { server: "bir", scope: "tin", value: "123/0000" },
    { server: "cal", scope: "day", value: "2026-10-28," },
  ]);
  assert.deepEqual(tokensIn("@bir:tin: nothing"), []);
});

test("server ids and scope or command names", () => {
  assert.ok(isServerId("bir-2"));
  assert.ok(!isServerId("Bir"));
  assert.ok(!isServerId("a b"));
  assert.ok(!isServerId(""));
  assert.ok(isName("Tax_Profile-1"));
  assert.ok(!isName("bad.scope"));
  assert.ok(!isName("x:y"));
  assert.ok(!isName(""));
  assert.ok(!isName(undefined));
  assert.ok(!isServerId(null));
});
