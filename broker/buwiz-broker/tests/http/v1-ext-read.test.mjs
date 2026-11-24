// Phase 2: read-only external desktop read door (/v1/ext/read) and host grant
// management (/v1/host/ext/grant, /v1/host/ext/grants, /v1/host/ext/revoke).
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { test } from "node:test";
import { PORT } from "./helpers.mjs";
import { FX, dataOf, installFixture, withHost } from "./v1-helpers.mjs";

async function rawPost(path, headers, body) {
  const r = await fetch(`http://127.0.0.1:${PORT}${path}`, {
    method: "POST",
    headers: { "content-type": "application/json", ...headers },
    body: JSON.stringify(body),
  });
  return { status: r.status, json: await r.json().catch(() => ({})) };
}

async function rawGet(path, headers) {
  const r = await fetch(`http://127.0.0.1:${PORT}${path}`, { headers });
  return { status: r.status, json: await r.json().catch(() => ({})) };
}

test("EXT-01 grant, read-only /v1/ext/read, write refusal, and revoke over HTTP", async () => {
  await withHost("ext01", async ({ b, api, dataDir }) => {
    await installFixture(api, b);
    const hostToken = readFileSync(join(dataDir, "host-token"), "utf8").trim();
    const hostHdr = { "X-Buwiz-Host": hostToken };

    // 1. /v1/host/ext/grants requires X-Buwiz-Host and starts empty
    const noHost = await rawGet("/v1/host/ext/grants", {});
    assert.equal(noHost.status, 401);
    const empty = await rawGet("/v1/host/ext/grants", hostHdr);
    assert.equal(empty.status, 200);
    assert.deepEqual(empty.json.grants, []);

    // 2. /v1/ext/read without bearer is 401
    const unauth = await rawPost("/v1/ext/read", {}, { plugin: FX, tool: "fx_search", args: { query: "books-coa" } });
    assert.equal(unauth.status, 401);

    // 2b. Non-read-only tool (fx_echo) is refused at grant time with `forbidden`
    const writeRefused = await rawPost("/v1/host/ext/grant", hostHdr, {
      client: "buwiz-forms",
      plugin: FX,
      tools: ["fx_echo"],
    });
    assert.equal(writeRefused.json.code, "forbidden");

    // 3. Granting a read-only tool (fx_search, readOnlyHint: true) returns a one-time ext_ token
    const granted = await rawPost("/v1/host/ext/grant", hostHdr, {
      client: "buwiz-forms",
      plugin: FX,
      tools: ["fx_search"],
    });
    assert.equal(granted.status, 200, JSON.stringify(granted.json));
    assert.equal(granted.json.ok, true);
    assert.ok(typeof granted.json.token === "string" && granted.json.token.startsWith("ext_"));
    const token = granted.json.token;
    const grantId = granted.json.id;

    // 4. Listing grants omits raw token
    const listed = await rawGet("/v1/host/ext/grants", hostHdr);
    assert.equal(listed.status, 200);
    assert.equal(listed.json.grants.length, 1);
    assert.equal(listed.json.grants[0].id, grantId);
    assert.equal(listed.json.grants[0].token, undefined);

    // 5. /v1/ext/read calls the real v1 plugin read-only tool and returns its result
    const res = await rawPost(
      "/v1/ext/read",
      { Authorization: `Bearer ${token}` },
      { plugin: FX, tool: "fx_search", args: { query: "books-coa" } },
    );
    assert.equal(res.status, 200, JSON.stringify(res.json));
    assert.equal(res.json.ok, true);
    const payload = res.json.result?.items ? res.json.result : dataOf(res.json.result);
    assert.equal(payload?.items?.[0]?.id, "books-coa");

    // 6. Ungranted tool is refused (403)
    const denied = await rawPost(
      "/v1/ext/read",
      { Authorization: `Bearer ${token}` },
      { plugin: FX, tool: "fx_secret", args: {} },
    );
    assert.equal(denied.status, 403);

    // 7. Revoking invalidates the bearer token immediately
    const rev = await rawPost("/v1/host/ext/revoke", hostHdr, { id: grantId });
    assert.equal(rev.status, 200);
    assert.equal(rev.json.revoked, true);

    const afterRev = await rawPost(
      "/v1/ext/read",
      { Authorization: `Bearer ${token}` },
      { plugin: FX, tool: "fx_search", args: { query: "books-coa" } },
    );
    assert.equal(afterRev.status, 401);
  });
});
