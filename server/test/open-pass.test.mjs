import { test } from "node:test";
import assert from "node:assert/strict";
import { redeem, withPass } from "../src/open-pass.mjs";

const passOf = (url) => new URL(url).searchParams.get("buwiz_pass");

test("an app link gets a pass that works once", () => {
  const url = withPass("buwiz://app/bir/forms?props=%7B%7D");
  assert.match(url, /^buwiz:\/\/app\/bir\/forms\?props=%7B%7D&buwiz_pass=[A-Za-z0-9_-]{22}$/);
  const pass = passOf(url);
  assert.equal(redeem(pass), true);
  assert.equal(redeem(pass), false, "a pass is used up");
});

test("a pass expires, and unknown passes are refused", () => {
  const t0 = 1_000_000;
  const pass = passOf(withPass("buwiz://app/bir/forms", t0));
  assert.equal(redeem(pass, t0 + 16_000), false, "older than 15 s");
  assert.equal(redeem("not-a-pass"), false);
  assert.equal(redeem(""), false);
});

test("a link without a query gets ?buwiz_pass=", () => {
  assert.match(withPass("buwiz://app/bir/calendar"), /^buwiz:\/\/app\/bir\/calendar\?buwiz_pass=/);
});
