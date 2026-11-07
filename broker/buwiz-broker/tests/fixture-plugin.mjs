// A mini app plugin for the broker's tests, with nothing to install: MCP over
// stdio, buwiz.provision and buwiz.session.open, the manifest FX_MANIFEST names.
// While the file FX_HOLD names is there, provisioning waits, after writing
// <FX_HOLD>.seen: a test's way to act during a restart.
import { createHash, createPrivateKey, sign } from "node:crypto";
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { createInterface } from "node:readline";

const text = readFileSync(process.env.FX_MANIFEST, "utf8");
const manifest = JSON.parse(text);
let seed = null;

const jcs = (v) => {
  if (Array.isArray(v)) return `[${v.map(jcs).join(",")}]`;
  if (v && typeof v === "object") return `{${Object.keys(v).sort().map((k) => `${JSON.stringify(k)}:${jcs(v[k])}`).join(",")}}`;

  return JSON.stringify(v);
};
const signed = (cert) => {
  const der = Buffer.concat([Buffer.from("302e020100300506032b657004220420", "hex"), Buffer.from(seed, "base64url")]);

  return sign(null, Buffer.from(jcs(cert)), createPrivateKey({ key: der, format: "der", type: "pkcs8" })).toString("base64url");
};
const ok = (structuredContent) => ({ content: [{ type: "text", text: JSON.stringify(structuredContent) }], structuredContent });
const tools = {
  "buwiz.provision": (a) => {
    const hold = process.env.FX_HOLD;

    if (hold && existsSync(hold)) {
      writeFileSync(`${hold}.seen`, "");
      while (existsSync(hold)) Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 10);
    }
    seed = a.key.seed;

    return ok({ ok: true, manifest_sha256: createHash("sha256").update(jcs(manifest)).digest("hex") });
  },
  "buwiz.session.open": (a) => {
    const cert = { v: 1, typ: "buwiz-session", plugin: manifest.id, app: a.app, open_id: a.open_id, ks_pub: a.ks_pub, perms_sha256: a.perms_sha256, not_after: a.not_after };

    return ok({ cert, cert_sig: signed(cert) });
  },
  "buwiz.session.close": () => ok({ ok: true }),
};
const reply = {
  initialize: (p) => ({ protocolVersion: p.protocolVersion, capabilities: { tools: {}, resources: {} }, serverInfo: { name: "fixture-plugin", version: "1.0.0" } }),
  "resources/list": () => ({ resources: [{ uri: "buwiz://manifest", name: "manifest", mimeType: "application/json" }] }),
  "resources/read": (p) => ({ contents: [{ uri: p.uri, mimeType: "application/json", text }] }),
  "tools/list": () => ({ tools: Object.keys(tools).map((name) => ({ name, description: "Reserved for Buwiz Bot.", inputSchema: { type: "object" } })) }),
  "tools/call": (p) => tools[p.name](p.arguments),
};

createInterface({ input: process.stdin }).on("line", (line) => {
  const m = JSON.parse(line);

  if (m.id !== undefined) process.stdout.write(`${JSON.stringify({ jsonrpc: "2.0", id: m.id, result: reply[m.method](m.params) })}\n`);
});
