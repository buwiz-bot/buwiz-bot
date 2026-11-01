// v1/manifest.mjs's checks run in buwiz-broker too (BUWIZ_BROKER_CMD set):
// every call asks `buwiz-broker manifest <op>` and Node's module the same
// question and asserts the same answer (the value, or the code and
// message), then hands back buwiz-broker's. Importing this in place of
// v1/manifest.mjs makes a test file run each case against both.
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { V1Error } from "../src/v1/errors.mjs";
import { isString } from "../src/v1/jcs.mjs";
import * as node from "../src/v1/manifest.mjs";

const CMD = process.env.BUWIZ_BROKER_CMD;

const rust = (op, input) => JSON.parse(execFileSync(CMD, ["manifest", op], { input: JSON.stringify(input), encoding: "utf8", maxBuffer: 16 << 20 }));

// an I-JSON parse error is worded by each parser (offsets vs line and column)
const PARSE = "manifest: is not I-JSON: ";
const comparable = (a) => (!a.ok && a.message.startsWith(PARSE) ? { ...a, message: PARSE } : a);

function both(op, input, run) {
  let want;

  try {
    want = { ok: true, value: JSON.parse(JSON.stringify(run() ?? null)) };
  } catch (e) {
    if (!(e instanceof V1Error)) throw e;
    want = { ok: false, code: e.code, message: e.message.slice(0, 500), retryable: e.retryable };
  }

  const got = rust(op, input);

  assert.deepEqual(comparable(got), comparable(want), `buwiz-broker manifest ${op} answers as Node`);

  if (!got.ok) throw new V1Error(got.code, got.message);

  return got.value;
}

// loadTrust's Map back in trust.json's shape
const trustJson = (t) => ({
  publishers: Object.fromEntries([...t.publishers].map(([keyId, p]) => [keyId, { alg: "ed25519", ...p }])),
  approved_plugins: t.approved,
});

assert.deepEqual(rust("limits", {}).value, JSON.parse(JSON.stringify(node.LIMITS)), "buwiz-broker holds Appendix A's limits");

export const { LIMITS } = node;

// the host protocol is Node's (buwiz-broker's may be newer: 2.1 packages)
export const verifyManifest = (text, trust, host) =>
  both("verify", { text: String(text ?? ""), trust: trustJson(trust), host: host || node.HOST_PROTOCOL }, () => node.verifyManifest(text, trust, host));

export const checkTools = (m, tools) => both("tools", { manifest: m, tools }, () => node.checkTools(m, tools));

export const validateProps = (schema, props) => both("props", { schema, props }, () => node.validateProps(schema, props));

export function reviewOf(m, titleOf = (id) => id) {
  const plugins = (m.routines || []).flatMap((r) => (r.reads || []).map((x) => x.plugin)).filter(isString);

  return both("review", { manifest: m, titles: Object.fromEntries(plugins.map((p) => [p, titleOf(p)])) }, () => node.reviewOf(m, titleOf));
}

export const addedPermissions = (prev, next) => both("added", { prev, next }, () => node.addedPermissions(prev, next));

export const compareSemver = (a, b) => both("semver", { a, b }, () => node.compareSemver(a, b));
