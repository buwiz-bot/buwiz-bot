// A node:test reporter for `npm run conformance`: after the spec reporter's
// output, the spec 17 checklist this suite covers, one line per ID with
// pass or FAIL and the tests that cover it. A test covers an ID when its
// name starts with it (e.g. "BRG-04 the same envelope twice is replayed").
// IDs outside spec 17's sections are this suite's extensions and are listed
// after it: RTN (v1 routine runner, 5.9), GWS (the Google Workspace plugin)
// and SDK (the mini app developer kit, sdk/mini-app).
const ID = /^([A-Z]{3}-\d{2})\b/;
const SPEC = new Set(["MAN", "APP", "BRG", "RSV", "INV", "AGT", "LCY", "CON"]);
const EXTENSIONS = { RTN: "v1 routine runner (5.9)", GWS: "Google Workspace plugin (dev.goldcoders.google-workspace)", SDK: "mini app developer kit (sdk/mini-app)" };

export default async function* checklist(source) {
  const ids = new Map(); // id → {ok, names}

  for await (const ev of source) {
    if (ev.type !== "test:pass" && ev.type !== "test:fail") continue;
    const id = ID.exec(ev.data.name)?.[1];

    if (!id) continue;
    const row = ids.get(id) || { ok: true, names: [] };

    row.ok &&= ev.type === "test:pass";
    row.names.push(ev.data.name);
    ids.set(id, row);
  }

  const line = (id) => {
    const { ok, names } = ids.get(id);

    return `  [${ok ? "x" : " "}] ${id} ${ok ? "pass" : "FAIL"}\n${names.map((n) => `        ${n}`).join("\n")}`;
  };

  const all = [...ids.keys()].sort();
  const spec = all.filter((id) => SPEC.has(id.slice(0, 3)));
  const ext = all.filter((id) => !SPEC.has(id.slice(0, 3)));
  const kinds = [...new Set(ext.map((id) => id.slice(0, 3)))].map((k) => `${k} ${EXTENSIONS[k] || ""}`.trim()).join("; ");

  yield `\nRox Mini App Protocol v1, spec 17 (broker side): ${spec.length} IDs\n${spec.map(line).join("\n")}\n`;

  if (ext.length) yield `\nExtensions (not in spec 17; ${kinds}): ${ext.length} IDs\n${ext.map(line).join("\n")}\n`;
}
