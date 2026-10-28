import { test } from "node:test";
import assert from "node:assert/strict";
import { redeem, withPass } from "../server/open-pass.mjs";

const passOf = (url) => new URL(url).searchParams.get("rox_pass");

test("an app link gets a pass that works once", () => {
  const url = withPass("rox://app/bir/forms?props=%7B%7D");
  assert.match(url, /^rox:\/\/app\/bir\/forms\?props=%7B%7D&rox_pass=[A-Za-z0-9_-]{22}$/);
  const pass = passOf(url);
  assert.equal(redeem(pass), true);
  assert.equal(redeem(pass), false, "a pass is used up");
});

test("a pass expires, and unknown passes are refused", () => {
  const t0 = 1_000_000;
  const pass = passOf(withPass("rox://app/bir/forms", t0));
  assert.equal(redeem(pass, t0 + 16_000), false, "older than 15 s");
  assert.equal(redeem("not-a-pass"), false);
  assert.equal(redeem(""), false);
});

test("a link without a query gets ?rox_pass=", () => {
  assert.match(withPass("rox://app/bir/calendar"), /^rox:\/\/app\/bir\/calendar\?rox_pass=/);
});
