// v1/line-env.mjs — rox-broker's line as rox-broker hands it to Node: its
// address (ROX_V1_LINE) and token (ROX_V1_LINE_TOKEN), read once and taken
// out of this process's environment. Node passes its environment to what
// it starts (stdio MCP servers, the office child, shell tools); none of
// them may inherit a token that drives the plugins. app-server imports this
// before anything else.
export const LINE = Object.freeze({ base: process.env.ROX_V1_LINE || "", token: process.env.ROX_V1_LINE_TOKEN || "" });

delete process.env.ROX_V1_LINE;
delete process.env.ROX_V1_LINE_TOKEN;
