// v1/line-env.mjs — buwiz-broker's line as buwiz-broker hands it to Node: its
// address (BUWIZ_V1_LINE) and token (BUWIZ_V1_LINE_TOKEN), read once and taken
// out of this process's environment (by their old names too, ROX_V1_LINE and
// ROX_V1_LINE_TOKEN). Node passes its environment to what
// it starts (stdio MCP servers, the office child, shell tools); none of
// them may inherit a token that drives the plugins. app-server imports this
// before anything else.
// `hosts`: what buwiz-broker hosts behind the line (BUWIZ_LINE_HOSTS, "v1,mcp");
// a broker that does not say hosts the v1 plugins only (phase 3's).
import { env, withOld } from "../env.mjs";

export const LINE = Object.freeze({
  base: env("BUWIZ_V1_LINE") || "",
  token: env("BUWIZ_V1_LINE_TOKEN") || "",
  hosts: Object.freeze((env("BUWIZ_LINE_HOSTS") || "v1").split(",").filter(Boolean)),
});

for (const name of ["BUWIZ_V1_LINE", "BUWIZ_V1_LINE_TOKEN", "BUWIZ_LINE_HOSTS"].flatMap(withOld)) delete process.env[name];
