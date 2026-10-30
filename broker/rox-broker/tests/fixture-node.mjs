// A stand-in for the Node broker behind rox-broker in its tests. It
// listens on PORT, appends its pid to FIXTURE_PID_FILE, and answers:
// /health, /echo (what it received), /sse (two events 300 ms apart),
// /exit (answers, then exits 3) and /role (its ROX_NODE_ROLE). With
// FIXTURE_LOG set it appends every request's method and URL there; with
// FIXTURE_ROLE_FILE, its ROX_NODE_ROLE at start (a path the hub never
// forwards can't ask it).
import { spawn } from "node:child_process";
import { appendFileSync } from "node:fs";
import { createServer } from "node:http";

if (process.env.FIXTURE_PID_FILE) appendFileSync(process.env.FIXTURE_PID_FILE, `${process.pid}\n`);
if (process.env.FIXTURE_ROLE_FILE) appendFileSync(process.env.FIXTURE_ROLE_FILE, `${process.env.ROX_NODE_ROLE || ""}\n`);

// a child that ignores its stdin closing, as a plugin can (FIXTURE_CHILD_FILE)
if (process.env.FIXTURE_CHILD_FILE) {
  const child = spawn(process.execPath, ["-e", "setInterval(() => {}, 1000)"], { stdio: "ignore" });

  appendFileSync(process.env.FIXTURE_CHILD_FILE, `${child.pid}\n`);
}

createServer((req, res) => {
  const chunks = [];

  req.on("data", (c) => chunks.push(c));
  req.on("end", () => {
    if (process.env.FIXTURE_LOG) appendFileSync(process.env.FIXTURE_LOG, `${req.method} ${req.url}\n`);

    if (req.url === "/health") return res.end("ok");

    if (req.url === "/role") return res.end(process.env.ROX_NODE_ROLE || "");

    if (req.url.startsWith("/echo")) {
      res.setHeader("content-type", "application/json");

      return res.end(JSON.stringify({
        method: req.method,
        url: req.url,
        host: req.headers.host,
        rox_host: req.headers["x-rox-host"] || null,
        signature: req.headers["x-rox-signature"] || null,
        body: Buffer.concat(chunks).toString("utf8"),
      }));
    }

    if (req.url === "/sse") {
      res.writeHead(200, { "content-type": "text/event-stream", "cache-control": "no-cache" });
      res.write("data: 1\n\n");
      setTimeout(() => res.write("data: 2\n\n"), 300);
      setTimeout(() => res.end(), 600);

      return;
    }

    if (req.url === "/exit") {
      res.end("bye");
      setTimeout(() => process.exit(3), 50);

      return;
    }
    res.statusCode = 404;
    res.end("no");
  });
}).listen(Number(process.env.PORT), "127.0.0.1");
