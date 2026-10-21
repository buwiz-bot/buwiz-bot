// routes/data.mjs — extracted from app-server.mjs
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { GEN_DIR, WIDGETS_DIR } from "../config.mjs";
import { db, getLinearKey, jxa } from "../state.mjs";

  const minBetween = (a, b) =>
    Math.max(0, Math.round((new Date(b || Date.now()) - new Date(a)) / 60000));
  const tcJson = () => {
    const rows = db
      .prepare(
        "SELECT clock_in AS cin, clock_out AS cout FROM timeclock ORDER BY id DESC LIMIT 25",
      )
      .all();
    const open = rows.find((r) => !r.cout);
    const dayStart = new Date();
    dayStart.setHours(0, 0, 0, 0);
    const sessions = rows
      .map((r) => ({ in: r.cin, out: r.cout, min: minBetween(r.cin, r.cout) }))
      .slice(0, 10);
    return {
      status: open ? "in" : "out",
      since: open?.cin ?? null,
      todayMin: rows
        .filter((r) => new Date(r.cin) >= dayStart)
        .reduce((s, r) => s + minBetween(r.cin, r.cout), 0),
      sessions,
    };
  };
  const L_FIELDS = `nodes { id identifier title priority url updatedAt state { id name } assignee { displayName } team { key } }`;

export default async function handle(req, res) {
  if (req.method === "GET" && req.url === "/data/timeclock") {
    res.setHeader("content-type", "application/json");
    return res.end(JSON.stringify(tcJson()));
  }
  if (req.method === "POST" && req.url === "/data/timeclock/in") {
    const open = db
      .prepare("SELECT id FROM timeclock WHERE clock_out IS NULL")
      .get();
    if (!open)
      db.prepare("INSERT INTO timeclock(clock_in) VALUES(?)").run(
        new Date().toISOString(),
      );
    res.setHeader("content-type", "application/json");
    return res.end(JSON.stringify({ ok: true, already: !!open, ...tcJson() }));
  }
  if (req.method === "POST" && req.url === "/data/timeclock/out") {
    db.prepare("UPDATE timeclock SET clock_out=? WHERE clock_out IS NULL").run(
      new Date().toISOString(),
    );
    res.setHeader("content-type", "application/json");
    return res.end(JSON.stringify({ ok: true, ...tcJson() }));
  }

  // reminders — JXA (osascript -l JavaScript); args travel via env vars.
  if (req.method === "GET" && req.url === "/data/reminders") {
    const out = jxa(`
      const app = Application("Reminders");
      const out = [];
      outer: for (const l of app.lists()) {
        const ln = l.name();
        for (const r of l.reminders.whose({completed: false})()) {
          let due = null, flagged = false;
          try { const d = r.dueDate(); if (d) due = d.toISOString(); } catch (e) {}
          try { flagged = !!r.flagged(); } catch (e) {}
          out.push({ name: r.name(), list: ln, due, flagged });
          if (out.length >= 50) break outer;
        }
      }
      JSON.stringify(out);`);
    res.setHeader("content-type", "application/json");
    return res.end(
      JSON.stringify(out?.error ? out : { reminders: out || [] }),
    );
  }
  if (
    req.method === "POST" &&
    (req.url === "/data/reminders/add" || req.url === "/data/reminders/complete")
  ) {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      let b = {};
      try {
        b = JSON.parse(body || "{}");
      } catch {}
      res.setHeader("content-type", "application/json");
      const env = {
        REM_NAME: String(b.name || ""),
        REM_LIST: String(b.list || ""),
        REM_DUE: String(b.due || ""),
      };
      if (!env.REM_NAME) return res.end(JSON.stringify({ error: "name?" }));
      const script =
        req.url === "/data/reminders/complete"
          ? `
            const app = Application("Reminders");
            ObjC.import("Cocoa");
            const env = $.NSProcessInfo.processInfo.environment;
            const name = ObjC.unwrap(env.objectForKey("REM_NAME"));
            let done = false;
            outer: for (const l of app.lists()) {
              const hits = l.reminders.whose({completed: false, name})();
              if (hits.length) { hits[0].completed = true; done = true; break outer; }
            }
            JSON.stringify({ ok: done });`
          : `
            const app = Application("Reminders");
            ObjC.import("Cocoa");
            const env = $.NSProcessInfo.processInfo.environment;
            const get = (k) => { const v = env.objectForKey(k); return v ? ObjC.unwrap(v) : null; };
            const name = get("REM_NAME"), listName = get("REM_LIST"), due = get("REM_DUE");
            let list = null;
            if (listName) { const hits = app.lists.whose({name: listName})(); if (hits.length) list = hits[0]; }
            if (!list) list = app.defaultList();
            const props = { name };
            if (due) props.dueDate = new Date(due);
            list.reminders.push(app.Reminder(props));
            JSON.stringify({ ok: true, name, list: list.name() });`;
      return res.end(JSON.stringify(jxa(script, env)));
    });
    return true;
  }

  // linear — GraphQL, key resolved lazily (env or `secret get`).
  const lgql = (query, variables) => {
    const key = getLinearKey();
    if (!key) return Promise.resolve({ _nokey: true });
    return fetch("https://api.linear.app/graphql", {
      method: "POST",
      headers: { Authorization: key, "Content-Type": "application/json" },
      body: JSON.stringify(variables ? { query, variables } : { query }),
    }).then((r) => r.json());
  };
  if (req.method === "GET" && req.url === "/data/linear/issues") {
    res.setHeader("content-type", "application/json");
    const shape = (nodes, scope) => ({
      scope,
      issues: nodes.map((n) => ({
        uid: n.id,
        id: n.identifier,
        title: n.title,
        state: n.state?.name || "",
        stateId: n.state?.id || "",
        priority: n.priority,
        assignee: n.assignee?.displayName || null,
        team: n.team?.key || null,
        url: n.url,
      })),
    });
    // Assigned-to-me first; when that's empty fall back to the workspace's
    // most recently updated issues so the widget still shows real data.
    lgql(
      `{ viewer { assignedIssues(first: 25, filter: { state: { type: { nin: ["completed","canceled"] } } }) { ${L_FIELDS} } } }`,
    )
      .then((d) => {
        if (d?._nokey)
          return res.end(
            JSON.stringify({
              error: "linear not configured — run `secret set LINEAR_API_KEY`",
            }),
          );
        if (d?.errors) throw new Error(String(d.errors[0]?.message));
        const assigned = d?.data?.viewer?.assignedIssues?.nodes || [];
        if (assigned.length) return res.end(JSON.stringify(shape(assigned, "assigned")));
        return lgql(
          `{ issues(first: 25, orderBy: updatedAt, filter: { state: { type: { nin: ["completed","canceled"] } } }) { ${L_FIELDS} } }`,
        ).then((d2) => {
          if (d2?.errors) throw new Error(String(d2.errors[0]?.message));
          res.end(
            JSON.stringify(shape(d2?.data?.issues?.nodes || [], "workspace")),
          );
        });
      })
      .catch((e) => {
        res.statusCode = 502;
        res.end(JSON.stringify({ error: String(e?.message || e) }));
      });
    return true;
  }

  // GET /data/linear/states — every team's workflow states (for transitions).
  if (req.method === "GET" && req.url === "/data/linear/states") {
    res.setHeader("content-type", "application/json");
    lgql(
      `{ teams(first: 10) { nodes { key states { nodes { id name type position } } } } }`,
    )
      .then((d) => {
        if (d?._nokey)
          return res.end(JSON.stringify({ error: "linear not configured" }));
        if (d?.errors) throw new Error(String(d.errors[0]?.message));
        const teams = (d?.data?.teams?.nodes || []).map((t) => ({
          team: t.key,
          states: t.states.nodes
            .slice()
            .sort((a, b) => a.position - b.position)
            .map((s) => ({ id: s.id, name: s.name, type: s.type })),
        }));
        res.end(JSON.stringify({ teams }));
      })
      .catch((e) => {
        res.statusCode = 502;
        res.end(JSON.stringify({ error: String(e?.message || e) }));
      });
    return true;
  }

  // POST /data/linear/update {uid, stateId?, priority?} — mutate an issue;
  // POST /data/linear/assign {uid} — assign it to the token's owner;
  // POST /data/linear/comment {uid, body} — leave a comment.
  const linearMutate = (build) => {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      res.setHeader("content-type", "application/json");
      let b = {};
      try {
        b = JSON.parse(body || "{}");
      } catch {}
      const call = build(b);
      if (call?.error) return res.end(JSON.stringify(call));
      lgql(call.query, call.variables)
        .then((d) => {
          if (d?._nokey)
            return res.end(JSON.stringify({ error: "linear not configured" }));
          if (d?.errors) throw new Error(String(d.errors[0]?.message));
          res.end(JSON.stringify({ ok: true }));
        })
        .catch((e) => {
          res.statusCode = 502;
          res.end(JSON.stringify({ error: String(e?.message || e) }));
        });
    });
  };


  // assign resolves the viewer id first, then runs the update mutation.
  if (req.method === "POST" && req.url === "/data/linear/assign") {
    let body = "";
    req.on("data", (c) => (body += c));
    return req.on("end", () => {
      res.setHeader("content-type", "application/json");
      let b = {};
      try {
        b = JSON.parse(body || "{}");
      } catch {}
      if (!b.uid) return res.end(JSON.stringify({ error: "uid?" }));
      lgql(`{ viewer { id } }`)
        .then((d) => {
          const vid = d?.data?.viewer?.id;
          if (!vid) throw new Error("no viewer");
          return lgql(
            `mutation($id: String!, $input: IssueUpdateInput!) { issueUpdate(id: $id, input: $input) { success } }`,
            { id: b.uid, input: { assigneeId: vid } },
          );
        })
        .then((d2) => {
          if (d2?.errors) throw new Error(String(d2.errors[0]?.message));
          res.end(JSON.stringify({ ok: true }));
        })
        .catch((e) =>
          res.end(JSON.stringify({ error: String(e?.message || e) })),
        );
    });
  }

  // GET /widget-feed — the generic channel the native WidgetKit extension
  // renders: every persisted widget's state flattened to summary lines, plus
  // first-party sources. items: [{id,title,module?,lines:[{text,url?}],view?}]
  //
  // `view` is the template payload the extension renders natively: a kind
  // + typed fields. Resolution: the module's persisted `widget` object
  // wins (it can compute live values), then its `widgetKind` export +
  // state-shape inference, then a plain list of the summary lines.
  const viewFor = (name, st, lines) => {
    if (st && typeof st.widget === "object" && st.widget?.kind)
      return st.widget;
    let kind = null;
    try {
      const file = [join(WIDGETS_DIR, `${name}.tsx`), join(GEN_DIR, `${name}.tsx`)]
        .find((f) => existsSync(f));
      const src = file && readFileSync(file, "utf8");
      kind = src?.match(/export\s+const\s+widgetKind\s*=\s*["'](\w+)["']/)?.[1] || null;
    } catch {}
    if (!kind) {
      // infer the template from the state's own shape
      if (st.status === "in" || st.status === "out" || typeof st.running === "boolean" ||
          st.since || st.startedAt) kind = "clock";
      else if (st.temp != null || st.temperature != null || st.condition) kind = "weather";
      else if (st.fraction != null || st.progress != null || st.percent != null) kind = "progress";
      else if (Object.values(st).some((v) => typeof v === "number")) kind = "stat";
      else kind = "list";
    }
    const num = (k) => (typeof st[k] === "number" ? st[k] : null);
    if (kind === "clock")
      return {
        kind,
        running: st.running ?? st.status === "in",
        startedAt: st.startedAt ?? st.since ?? null,
        todayMin: num("todayMin") ?? num("minutes") ?? num("elapsedMin") ?? 0,
        label: st.label ?? null,
      };
    if (kind === "progress") {
      const f = num("fraction") ?? num("progress") ??
        (num("percent") != null ? num("percent") / 100 : null) ??
        (num("value") != null && num("total") ? num("value") / num("total") : null);
      return {
        kind,
        fraction: Math.max(0, Math.min(1, f ?? 0)),
        value: st.valueText ?? st.label ?? null,
        sub: st.sub ?? null,
      };
    }
    if (kind === "weather")
      return {
        kind,
        temp: num("temp") ?? num("temperature"),
        unit: st.unit ?? "°",
        condition: st.condition ?? null,
        high: num("high") ?? num("hi"),
        low: num("low") ?? num("lo"),
        days: Array.isArray(st.days) ? st.days.slice(0, 4) : null,
      };
    if (kind === "note")
      return {
        kind,
        headline: st.headline ?? st.title ?? null,
        body: st.body ?? st.text ?? lines[0]?.text ?? null,
      };
    if (kind === "stat") {
      // headline number = the most meaningful numeric key the module stores
      const pick = ["count", "best", "total", "score", "value", "minutesFocused",
        "minutes", "n", "plays", "rolls", "sessions"]
        .find((k) => typeof st[k] === "number") ??
        Object.keys(st).find((k) => typeof st[k] === "number" && k !== "widget");
      return {
        kind,
        value: pick != null ? String(st[pick]) : "—",
        unit: st.unit ?? null,
        label: st.label ?? pick ?? null,
        sub: st.sub ?? lines[1]?.text ?? null,
      };
    }
    // A stateless app has nothing to template — leave `view` unset so the
    // extension falls back to the rendered PNG of the real UI instead of
    // showing a bare "tap to open" row.
    if (lines.length === 1 && lines[0].text === "tap to open") return null;
    return { kind: "list", rows: lines.map((l) => ({ title: l.text, detail: null, url: l.url ?? null })) };
  };

  if (req.method === "GET" && req.url === "/widget-feed") {
    res.setHeader("content-type", "application/json");
    const items = [];
    const pinned = new Set(
      db.prepare("SELECT k FROM meta WHERE k LIKE 'pin:%'").all()
        .map((r) => r.k.slice(4)),
    );
    const finish = () => {
      // nominated (pinned) widgets lead the feed, then linear, then the
      // rest — an unconfigured widget slot shows your pick, not linear.
      items.sort(
        (a, b) =>
          (b.pinned ? 1 : 0) - (a.pinned ? 1 : 0) ||
          (a.id === "linear" ? -1 : b.id === "linear" ? 1 : 0),
      );
      res.end(
        JSON.stringify({ updated: new Date().toISOString(), items }),
      );
    };
    // every published widget AND every generated module that persisted
    // state is a feed row — state lines when present, a launch hint
    // otherwise. Nominated (pinned) ones lead.
    const titles = Object.fromEntries(
      db.prepare("SELECT name,title FROM widgets").all().map((w) => [w.name, w.title]),
    );
    const names = new Set([
      ...Object.keys(titles),
      ...db.prepare("SELECT name FROM kv").all().map((r) => r.name),
    ]);
    for (const name of [...names].sort()) {
      const w = { name, title: titles[name] };
      let st = {};
      try {
        st = JSON.parse(
          db.prepare("SELECT data FROM kv WHERE name=?").get(w.name)?.data || "{}",
        );
      } catch {}
      const lines = Object.entries(st)
        .filter(([, v]) => v == null || ["string", "number", "boolean"].includes(typeof v))
        .slice(0, 3)
        .map(([k, v]) => ({ text: `${k}: ${v}` }));
      if (!lines.length) lines.push({ text: "tap to open" });
      const clean = (w.title || w.name)
        .replace(/\*\*/g, "")
        .split(/[:.\n—]/)[0]
        .trim()
        .slice(0, 28);
      items.push({
        id: w.name,
        title: clean || w.name,
        module: w.name,
        pinned: pinned.has(w.name),
        lines,
        view: viewFor(w.name, st, lines),
      });
    }
    // first-party: clock-in/out status (no module — informational row)
    const tc = tcJson();
    items.push({
      id: "timeclock",
      title: "Time Clock",
      lines: [
        { text: tc.status === "in" ? `clocked in · ${minBetween(tc.since, null)} min` : "clocked out" },
        { text: `${tc.todayMin} min today` },
      ],
      view: {
        kind: "clock",
        running: tc.status === "in",
        startedAt: tc.since,
        todayMin: tc.todayMin,
      },
    });
    // linear — same assigned-first/fallback query as /data/linear/issues
    const pushLinear = (nodes, scope) =>
      items.unshift({
        id: "linear",
        title: `Linear · ${nodes.length} open`,
        lines: nodes.slice(0, 4).map((n) => ({
          text: `${n.identifier} · ${n.title}`,
          url: n.url || null,
        })),
        view: {
          kind: "list",
          rows: nodes.slice(0, 4).map((n) => ({
            title: n.title,
            detail: `${n.identifier}${n.state?.name ? " · " + n.state.name : ""}`,
            url: n.url || null,
          })),
        },
      });
    if (!getLinearKey()) {
      items.unshift({ id: "linear", title: "Linear", lines: [{ text: "not configured" }] });
      return finish();
    }
    lgql(
      `{ viewer { assignedIssues(first: 25, filter: { state: { type: { nin: ["completed","canceled"] } } }) { ${L_FIELDS} } } }`,
    )
      .then((d) => {
        const assigned = d?.data?.viewer?.assignedIssues?.nodes || [];
        if (assigned.length) return pushLinear(assigned, "assigned"), finish();
        return lgql(
          `{ issues(first: 25, orderBy: updatedAt, filter: { state: { type: { nin: ["completed","canceled"] } } }) { ${L_FIELDS} } }`,
        ).then((d2) => {
          pushLinear(d2?.data?.issues?.nodes || [], "workspace");
          finish();
        });
      })
      .catch(() => {
        items.unshift({ id: "linear", title: "Linear", lines: [{ text: "unreachable" }] });
        finish();
      });
    return true;
  }
  return false;
}
