// routes/routines.mjs — Settings › Routines (routines.mjs): the list, the
// on/off switch and interval, Delete, Run now, and the templates connected
// servers suggest.
import { deleteRoutine, listRoutines, listTemplates, routineFromTemplate, runRoutine, updateRoutine } from "../routines.mjs";

export default async function handle(req, res) {
  // GET /routines — {routines:[…]}, oldest first.
  if (req.method === "GET" && req.url === "/routines") {
    res.setHeader("content-type", "application/json");
    res.end(JSON.stringify({ routines: listRoutines() }));
    return true;
  }
  // GET /routines/templates — [{server, id, name, description, every_s,
  // needs, available, reason?, needs_email?}].
  if (req.method === "GET" && req.url === "/routines/templates") {
    res.setHeader("content-type", "application/json");
    res.end(JSON.stringify(await listTemplates()));
    return true;
  }
  // POST /routine/update {id, enabled?, every_s?, prompt?, name?} →
  // {ok, routine}; POST /routine/delete {id} → {ok}; POST /routine/run {id}
  // → {ok, routine} once the tick is done; POST /routine/from-template
  // {server, id, thread, user_google_email?} → {ok, routine}, or {ok:false,
  // error, need:"user_google_email"} when Rox has to ask for it.
  if (req.method === "POST" && /^\/routine\/(update|delete|run|from-template)$/.test(req.url)) {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", async () => {
      res.setHeader("content-type", "application/json");
      try {
        const j = JSON.parse(body || "{}");
        if (req.url === "/routine/delete") {
          deleteRoutine(j.id);
          return res.end(JSON.stringify({ ok: true }));
        }
        const routine =
          req.url === "/routine/run" ? await runRoutine(j.id) : req.url === "/routine/from-template" ? await routineFromTemplate(j) : updateRoutine(j);
        res.end(JSON.stringify({ ok: true, routine }));
      } catch (e) {
        res.statusCode = 400;
        res.end(JSON.stringify({ ok: false, error: String(e?.message || e), ...(e?.need ? { need: e.need } : {}) }));
      }
    });
    return true;
  }
  return false;
}
