// Origin and Content-Type checks at dispatch (server/guard.mjs).
import assert from "node:assert/strict";
import { after, before, test } from "node:test";
import { PORT, postJson, request, startServer, tempDir } from "./helpers.mjs";

let server;

before(async () => {
  server = await startServer(tempDir("guard"));
});

after(async () => {
  await server?.stop();
});

const SERVER_BODY = JSON.stringify({ name: "web", command: "/usr/bin/true" });

test("a Rox-style request works", async () => {
  const health = await request("GET", "/health");
  assert.equal(health.status, 200);
  assert.equal(health.headers["access-control-allow-origin"], undefined);
  const r = await postJson("/thread/new", {});
  assert.equal(r.status, 200);
  assert.equal(r.json.ok, true);

  const charset = await request("POST", "/thread/new", {
    headers: { "Content-Type": "application/json; charset=utf-8" },
    body: "{}",
  });

  assert.equal(charset.status, 200);
});

test("a bodyless POST needs no Content-Type", async () => {
  const r = await request("POST", "/mcp/server/state");
  assert.equal(r.status, 400); // reached the route: it names no server
  assert.match(r.json.error, /no such server/);
});

test("a request with a foreign Origin is refused", async () => {
  const r = await request("POST", "/mcp/servers", {
    headers: { Origin: "https://evil.example", "Content-Type": "application/json" },
    body: SERVER_BODY,
  });

  assert.equal(r.status, 403);
  assert.equal(r.json.ok, false);
  assert.match(r.json.error, /evil\.example/);
  assert.equal(r.headers["access-control-allow-origin"], undefined);

  for (const origin of ["null", `http://127.0.0.1:${PORT + 1}`]) {
    const g = await request("GET", "/mcp/servers", { headers: { Origin: origin } });
    assert.equal(g.status, 403, origin);
  }

  const pre = await request("OPTIONS", "/mcp/servers", {
    headers: { Origin: "https://evil.example", "Access-Control-Request-Method": "POST" },
  });

  assert.equal(pre.status, 403);
  assert.equal(pre.headers["access-control-allow-origin"], undefined);
  const list = await request("GET", "/mcp/servers");
  assert.deepEqual(list.json.servers, []);
});

test("the server's own origin is allowed and reflected", async () => {
  const origin = `http://127.0.0.1:${PORT}`;

  const r = await request("POST", "/thread/new", {
    headers: { Origin: origin, "Content-Type": "application/json" },
    body: "{}",
  });

  assert.equal(r.status, 200);
  assert.equal(r.headers["access-control-allow-origin"], origin);
  const pre = await request("OPTIONS", "/thread/new", { headers: { Origin: origin } });
  assert.equal(pre.status, 204);
});

test("a body that is not application/json is refused", async () => {
  for (const type of ["text/plain", "text/plain;charset=UTF-8", "application/x-www-form-urlencoded", "multipart/form-data; boundary=x"]) {
    const r = await request("POST", "/mcp/servers", { headers: { "Content-Type": type }, body: SERVER_BODY });
    assert.equal(r.status, 415, type);
    assert.equal(r.json.ok, false);
    assert.match(r.json.error, /application\/json/);
  }

  const untyped = await request("POST", "/mcp/servers", { body: Buffer.from(SERVER_BODY) });
  assert.equal(untyped.status, 415);
  const list = await request("GET", "/mcp/servers");
  assert.deepEqual(list.json.servers, []);
});
