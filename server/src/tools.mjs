// tools.mjs — the agent's tool surface: schemas, executors (generate_app,
// images, fetch/shell/search, react, office, artifacts, launch/close,
// version ops), MCP and skill admin.
import { execFile, execFileSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { ASSETS_DIR, GEN_DIR, MODEL, OUT_DIR, STAGE_DIR, outFile } from "./config.mjs";
import { uuidv7, db, modelCtx, openWindows, nextSeq, setJob, store, slugFile, writeIndex, cardTurn, markRevising, clearRevising } from "./state.mjs";
import { contractFor, labelOf } from "./model.mjs";
import { checkCandidate, fixImports, headOf, setHead, verFile, writeVersion } from "./pipeline.mjs";
import { fileRecord } from "./files.mjs";
import { appInfo, genAppIcon, iconPath, setAppCols, updateAppMeta } from "./appmeta.mjs";
import { callOfficeTool, officeCreateBytes, officeMcpTools } from "./office.mjs";
import { pluginSkillsInject } from "./plugins.mjs";
import { editImage, generateImage } from "./avatars.mjs";
import { BOT_TOOLS, botOfThread, botSection, isBotTool, recordToolUse, toolBot } from "./botctx.mjs";
import {
  anthropicTools as mcpAnthropicTools, appKey, appOfUri, callTool as mcpCallTool, callToolJson as mcpCallJson,
  ensureClient as mcpEnsure, listApps as mcpListApps, parseAppKey, readApp as mcpReadApp, readManifest as mcpReadManifest, invalidate as mcpInvalidate, parseMcpName,
  setAuthHooks as mcpSetAuthHooks, statusOf as mcpStatus,
} from "./mcp.mjs";
import { DRIVE_SERVER, noteDriveCall } from "./drive.mjs";
import { isServerId } from "./grammar.mjs";
import { ROUTINE_TOOLS, isRoutineTool, noteGoogleEmail, toolRoutine } from "./routines.mjs";
import { v1AgentCall, v1ToolPlugin, v1Tools } from "./v1/agent.mjs";
import { isString, kindOf } from "./v1/jcs.mjs";
import { plugins as v1Plugins } from "./v1/broker.mjs";

export const CLIENT_SEARCH = {
  name: "web_search",
  description:
    "Search the web for current facts, news, docs, prices. Returns ranked results (title, url, snippet).",
  input_schema: {
    type: "object",
    properties: { query: { type: "string", description: "the search query" } },
    required: ["query"],
  },
};

// generate_image's aspect presets, in pixels (multiples of 16).
// The sizes gpt-image makes — the placeholder is drawn at the one picked.
const IMAGE_ASPECTS = {
  "1:1": [1024, 1024],
  "3:2": [1536, 1024],
  "2:3": [1024, 1536],
};

export const TOOLS = [
  {
    name: "generate_app",
    description:
      "Build an interactive native-UI mini-app from a complete Native Shell TSX module and mount it as a card in the conversation. Use it whenever the user wants an interface — apps, games, trackers, dashboards, forms, data views. The server typechecks it and returns the compiler error if it fails; fix the code and call again. Always pass description and icon_prompt on create — the app gets a generated icon from icon_prompt in the background.",
    input_schema: {
      type: "object",
      properties: {
        caption: {
          type: "string",
          description: "optional one-line markdown caption rendered above the card",
        },
        title: {
          type: "string",
          description:
            'short product name shown as the card\'s title (e.g. "Tic Tac Toe") — always set it; the Library also lists the app under this',
        },
        desc: {
          type: "string",
          description:
            'one-line description of what the app does, shown under the title (e.g. "Two-player noughts and crosses")',
        },
        layout: {
          type: "object",
          description:
            "the app's intended render size in points — the card's placeholder is drawn at exactly this size while it loads, so declare the size you designed for",
          properties: {
            width: { type: "number" },
            height: { type: "number" },
          },
        },
        slug: {
          type: "string",
          description:
            'optional kebab-case display filename, no extension (e.g. "tiny-trajectory" → tiny-trajectory.tsx)',
        },
        description: {
          type: "string",
          description:
            "one sentence saying what the app does, kept in the app's details (e.g. \"Track daily water intake against a goal\")",
        },
        icon_prompt: {
          type: "string",
          description:
            "the app icon's subject/metaphor — a single symbol, not the UI (e.g. \"a glass of water with a rising level gauge\")",
        },
        // Last: the card's title and size stream first, so its placeholder
        // is drawn while the code is still being written.
        code: {
          type: "string",
          description:
            "The complete TSX module: one default export function returning JSX, no mount(), all names imported from nsh modules. Where the module needs its own name (the /state endpoint) write the literal string \"__NAME__\" — the server substitutes it on write.",
        },
      },
      required: ["code"],
    },
  },
  {
    name: "generate_image",
    description:
      "Generate a real image from a text prompt (gpt-image) and mount it as a card — or edit an existing image card (edit_of). Use for draw/image/picture/wallpaper/icon requests — describe subject, style, lighting richly. Never use it for UI mockups; interfaces are generate_app. Always choose the shape up front with aspect (\"1:1\" square, \"3:2\" landscape — scenes, wallpapers, banners; \"2:3\" portrait — people, posters, phone wallpapers): the placeholder the user watches is drawn at that exact size.",
    input_schema: {
      type: "object",
      properties: {
        prompt: { type: "string", description: "rich description of the image to generate" },
        caption: { type: "string", description: "optional one-line markdown caption" },
        title: { type: "string", description: 'short product name shown as the card\'s title (e.g. "Sunset Over The Marina")' },
        aspect: {
          type: "string",
          enum: Object.keys(IMAGE_ASPECTS),
          description: "the image's shape (default 1:1): 1:1 → 1024×1024, 3:2 → 1536×1024, 2:3 → 1024×1536",
        },
        edit_of: {
          type: "string",
          description: 'an existing image card to change (its name, "R<n>") — the edit keeps the rest of the picture and becomes that card\'s next version. Use it whenever the user asks to change, fix or adjust an image they replied to or named; the prompt then says only what to change. Its shape is kept unless aspect is given.',
        },
      },
      required: ["prompt"],
    },
  },
  {
    name: "fetch_url",
    description:
      "Fetch a web page and return its readable text (HTML stripped, ~6000 chars). Use it to read a specific URL the user gives or a search result worth opening.",
    input_schema: {
      type: "object",
      properties: { url: { type: "string" } },
      required: ["url"],
    },
  },
  {
    name: "query_source",
    description:
      "Read one of the app's live local data sources and return its JSON. Sources: 'timeclock' (the user's clock-in/out tracker), 'reminders' (macOS Reminders), 'linear' (Linear issues). Use it when the user asks about their own data — answer in text, or feed it into a widget you build.",
    input_schema: {
      type: "object",
      properties: {
        source: { type: "string", enum: ["timeclock", "reminders", "linear"] },
      },
      required: ["source"],
    },
  },
  {
    name: "office_create",
    description:
      "Create a real Office file — .docx, .xlsx, or .pptx — and mount it as a file card. Use it whenever the user asks for a document, spreadsheet, or slide deck (report, budget, memo, pitch). Payload by extension: docx.paragraphs [{text, style: title|heading1|heading2|normal, bold, italic}]; xlsx.rows string[][] of raw inputs (\"42\" is a number, \"=SUM(A1:A3)\" a formula, anything else text); pptx.slides [{title, bullets: string[], notes}]. To revise the file later use the office_* tools (office_files → office_open → office_grep/office_cells → office_propose/office_propose_cells → office_export).",
    input_schema: {
      type: "object",
      properties: {
        path: { type: "string", description: "file name ending .docx, .xlsx, or .pptx" },
        docx: {
          type: "object",
          properties: {
            paragraphs: {
              type: "array",
              items: {
                type: "object",
                properties: { text: { type: "string" }, style: { type: "string", enum: ["title", "heading1", "heading2", "normal"] }, bold: { type: "boolean" }, italic: { type: "boolean" } },
                required: ["text"],
              },
            },
          },
        },
        xlsx: { type: "object", properties: { rows: { type: "array", items: { type: "array", items: { type: "string" } } } } },
        pptx: {
          type: "object",
          properties: {
            slides: {
              type: "array",
              items: {
                type: "object",
                properties: { title: { type: "string" }, bullets: { type: "array", items: { type: "string" } }, notes: { type: "string" } },
              },
            },
          },
        },
      },
      required: ["path"],
    },
  },
  {
    name: "revise_app",
    description:
      "Revise an existing mini-app artifact in place: writes the code as a NEW version on top of base_v (default: the current version) and makes it what Library launches. ALWAYS use this — never generate_app — when the user asks to change, fix, or iterate an app that already exists (its current source is in the conversation wherever the artifact was replied to or @-mentioned). base_v names the version to branch from. Pass description when what the app does changed, and icon_prompt only when its icon should be redrawn.",
    input_schema: {
      type: "object",
      properties: {
        name: { type: "string", description: "the artifact id, e.g. R6" },
        base_v: { type: "number", description: "version to revise from (default: current); revising an older version starts a branch" },
        branch_name: { type: "string", description: "optional short name for the branch when base_v isn't the current version (e.g. \"slow-ball\")" },
        code: { type: "string", description: "the complete revised TSX module (same contract as generate_app)" },
        caption: { type: "string", description: "optional one-line markdown caption" },
        title: { type: "string", description: "optional new product name" },
        desc: { type: "string", description: "optional one-line description of what changed, shown under the title" },
        layout: { type: "object", description: "optional revised intended render size in points", properties: { width: { type: "number" }, height: { type: "number" } } },
        slug: { type: "string", description: "optional kebab-case display filename" },
        description: { type: "string", description: "optional updated one-sentence description of what the app does" },
        icon_prompt: { type: "string", description: "optional — redraw the app icon with this subject/metaphor (a single symbol, not the UI)" },
      },
      required: ["name", "code"],
    },
  },
  {
    name: "react",
    description:
      "Attach an emoji reaction to a message in the conversation — like an iMessage tapback. Omit index to react to the latest user message; an empty emoji clears the reaction.",
    input_schema: {
      type: "object",
      properties: {
        emoji: { type: "string", description: "the emoji, e.g. 👍 — empty string removes the reaction" },
        index: { type: "number", description: "0-based turn index in this conversation (default: latest user message)" },
      },
      required: ["emoji"],
    },
  },
  {
    name: "launch_app",
    description: "Open a mini-app artifact in its own pop-out window — the current version unless v is given.",
    input_schema: {
      type: "object",
      properties: { name: { type: "string" }, v: { type: "number" } },
      required: ["name"],
    },
  },
  {
    name: "close_app",
    description: "Close a mini-app's open pop-out windows.",
    input_schema: {
      type: "object",
      properties: { name: { type: "string" } },
      required: ["name"],
    },
  },
  {
    name: "promote_app",
    description: "Promote a mini-app artifact into the user's Mini Apps library (publish). demote_app removes it from Mini Apps without deleting it.",
    input_schema: {
      type: "object",
      properties: { name: { type: "string" } },
      required: ["name"],
    },
  },
  {
    name: "demote_app",
    description: "Remove a mini-app artifact from Mini Apps (unpublish) — the artifact and its versions stay in the conversation's Library.",
    input_schema: {
      type: "object",
      properties: { name: { type: "string" } },
      required: ["name"],
    },
  },
  {
    name: "set_version",
    description: "Move an artifact's current pointer to version v — a revert. Later versions stay on the tree; the next revise off a reverted version starts a branch.",
    input_schema: {
      type: "object",
      properties: { name: { type: "string" }, v: { type: "number" } },
      required: ["name", "v"],
    },
  },
  {
    name: "view_code",
    description: "Read a mini-app artifact's TSX source — version v, or the current version when v is omitted.",
    input_schema: {
      type: "object",
      properties: { name: { type: "string" }, v: { type: "number" } },
      required: ["name"],
    },
  },
  {
    name: "app_icon",
    description: "Regenerate the icon of an existing mini-app artifact from a description of its subject/metaphor (a single bold symbol, not the UI). Waits for the image.",
    input_schema: {
      type: "object",
      properties: {
        name: { type: "string", description: "the artifact id, e.g. R6" },
        prompt: { type: "string", description: "the icon's subject/metaphor, e.g. \"a paper plane over a calendar grid\"" },
      },
      required: ["name", "prompt"],
    },
  },
  {
    name: "app_details_update",
    description: "Edit a mini-app's details without changing its code: title, one-sentence description, display filename, and its preferred window size in points (0 clears).",
    input_schema: {
      type: "object",
      properties: {
        name: { type: "string", description: "the artifact id, e.g. R6" },
        title: { type: "string" },
        description: { type: "string", description: "one sentence, ≤280 chars; empty clears" },
        file: { type: "string", description: "kebab-case display filename" },
        width: { type: "number", description: "preferred width in points (120–2400, 0 clears)" },
        height: { type: "number", description: "preferred height in points (120–2400, 0 clears)" },
      },
      required: ["name"],
    },
  },
  {
    name: "mcp_install",
    description: "Add a stdio MCP server, switched off: it is not started until the user reviews and enables it in Plugins, and only then do its tools become callable. Use when the user wants an external capability you lack (filesystem, GitHub, Slack, a database, …), then tell the user to enable it. command is the executable (npx, uvx, node, python, …), args its arguments, env the KEY=VALUE pairs it needs (ask the user for any secrets; never invent them). scope \"bot\" binds it to this bot only; default shares it with every bot.",
    input_schema: {
      type: "object",
      properties: {
        name: { type: "string", description: "short label, e.g. \"github\"" },
        command: { type: "string" },
        args: { type: "array", items: { type: "string" }, description: "e.g. [\"-y\",\"@modelcontextprotocol/server-github\"]" },
        env: { type: "object", additionalProperties: { type: "string" } },
        scope: { type: "string", enum: ["all", "bot"] },
      },
      required: ["name", "command"],
    },
  },
  {
    name: "mcp_uninstall",
    description: "Remove an MCP server connection entirely — its tools disappear. id is the slug mcp_install returned (mcp_list shows them); or pass the server's name.",
    input_schema: {
      type: "object",
      properties: { id: { type: "string" }, name: { type: "string" } },
    },
  },
  {
    name: "mcp_enable",
    description: "Re-enable a disabled MCP server (id or name) — its tools return next step. A server you added with mcp_install can only be enabled by the user, in Plugins.",
    input_schema: {
      type: "object",
      properties: { id: { type: "string" }, name: { type: "string" } },
    },
  },
  {
    name: "mcp_disable",
    description: "Disable an MCP server (id or name) without removing its configuration — its tools are withheld until mcp_enable.",
    input_schema: {
      type: "object",
      properties: { id: { type: "string" }, name: { type: "string" } },
    },
  },
  {
    name: "mcp_list",
    description: "List configured MCP servers: id, name, status, tool count, scope, env key names.",
    input_schema: { type: "object", properties: {} },
  },
  {
    name: "skill_create",
    description: "Create a private skill — saved instructions you follow whenever they apply. Use when the user teaches you a procedure, a preference, or a slash-command-like workflow they want kept (\"remember to always…\", \"when I say X, do Y\"). instructions is the full skill body in markdown.",
    input_schema: {
      type: "object",
      properties: {
        name: { type: "string", description: "kebab-case id, e.g. \"draft-x-post\"" },
        description: { type: "string", description: "one line — when to use it" },
        instructions: { type: "string", description: "the skill body in markdown" },
      },
      required: ["name", "description", "instructions"],
    },
  },
  {
    name: "skill_update",
    description: "Edit a private skill (id or name) — description and/or instructions.",
    input_schema: {
      type: "object",
      properties: {
        id: { type: "string" },
        name: { type: "string" },
        description: { type: "string" },
        instructions: { type: "string" },
      },
    },
  },
  {
    name: "skill_delete",
    description: "Delete a private skill (id or name) permanently.",
    input_schema: {
      type: "object",
      properties: { id: { type: "string" }, name: { type: "string" } },
    },
  },
  {
    name: "skill_enable",
    description: "Re-enable a private skill (id or name) — its instructions apply again.",
    input_schema: {
      type: "object",
      properties: { id: { type: "string" }, name: { type: "string" } },
    },
  },
  {
    name: "skill_disable",
    description: "Disable a private skill (id or name) — kept, but its instructions stop applying.",
    input_schema: {
      type: "object",
      properties: { id: { type: "string" }, name: { type: "string" } },
    },
  },
  {
    name: "skill_list",
    description: "List private skills: id, name, enabled, description.",
    input_schema: { type: "object", properties: {} },
  },
  {
    name: "shell_run",
    description: "Run a shell command on the user's Mac (zsh login shell, so the user's PATH — homebrew, cargo, nodenv, local CLIs — is available). Returns stdout+stderr and the exit code; long output is truncated. Use it for local tasks the user asks for: listing/reading files, running CLIs like aui, checking processes, git, building. Keep commands non-interactive — no sudo, no editors, no prompts; timeout_ms caps runtime (default 30s, max 180s).",
    input_schema: {
      type: "object",
      properties: {
        command: { type: "string", description: "the exact zsh command line" },
        cwd: { type: "string", description: "working directory; defaults to the user's home" },
        timeout_ms: { type: "number" },
      },
      required: ["command"],
    },
  },
];

// A bot's thread appends its own context (who it is, instructions,
// memories); a plain chat gets the base prompt unchanged.
export const agentSystem = (thread) => baseSystem() + botSection(thread);

const baseSystem = () => `You are the assistant inside a generative-UI chat app: a chat where you answer in text OR native UI, whichever fits the request.

Reply style: GitHub-flavored markdown, concise. Conversation, questions, and explanations get a text reply — do NOT build a card for those. When the user asks about their own data use query_source first, then answer (or build a widget from what it returns).

When a real interface is what they want — apps, games, trackers, dashboards, forms, anything interactive or persistent — call generate_app once with the complete module. The server typechecks it; if it returns a compiler error, fix the code and call again (a few tries is normal). You may build more than one card in a turn when the request implies it. Always set generate_app's title (a short product name, e.g. "Tiny Trajectory"), desc (one line about what it does), description (one sentence for the app's details), icon_prompt (the icon's subject/metaphor — one symbol, not the UI), layout (the width/height in points you designed it for) and slug (kebab-case, e.g. tiny-trajectory) — title and desc head the card in chat, layout sizes its placeholder exactly, and title/slug name the app in the user's Library.

Artifacts are versioned: every app you mount keeps its versions under one name (R<n>), and each card in the conversation pins the version it rendered. When the user asks to change, fix, restyle, or extend an app that already exists — especially when they replied to its card or @-mentioned it, so its source is attached — call revise_app with that name; never rebuild it as a new artifact. To iterate an older version on purpose, pass base_v — that starts a branch the user can see in the Library's version history. launch_app pops an app into its own window, close_app closes it, promote_app/demote_app publish/unpublish to Mini Apps, set_version reverts the current pointer, view_code reads any version's source.

Compose tools: search the web or query a source first, then build the widget around real results — that composition is the point of this app.

Connected apps: tools named mcp_<server>_<tool> come from MCP servers the user connected — use them like built-in tools; their [server] tag says which app they belong to. Apps with many tools aren't listed until loaded: app_tools_load names them — load the one a request needs, then call its tools. When the user wants a capability you don't have, add it with mcp_install (a stdio MCP server command — npx/uvx/node/…); it is added switched off, and its mcp_* tools appear only after the user enables it in Plugins, so tell them to. mcp_list, mcp_uninstall, mcp_enable and mcp_disable manage what is connected. Never invent env secrets — ask the user for keys a server needs.
Office files: when the user wants a document, spreadsheet, or deck, call office_create with the right payload — never paste file-like text into the reply. The office_* tools revise existing files: office_files lists what's under the workspace (including files from earlier replies), office_open starts a session, grep/cells find exact spans, propose/propose_cells stage changes, office_export writes a new file (it lands as a card). Accept edits only when the user wants the same file updated in place.
Word, Excel or PowerPoint files in Google Drive (.docx/.xlsx/.pptx — not Google Docs, whose own docs tools edit them): get_drive_file_download_url saves the file under drive/ in the office workspace and returns its absolute path; office_open that path, find and stage the change, office_accept it, then office_export it to a new name in the same folder; finally update_drive_file with the file's id and file_path = the exported file's absolute path (the download's folder + the exported name). That replaces the Drive file's content in place — same id, link and sharing — so tell the user it is saved, not only that a copy exists.

Private skills: the user teaches you procedures — when they explain a workflow to keep, skill_create saves it (kebab-case name, one-line description, full instructions). skill_update, skill_delete, skill_enable, skill_disable and skill_list manage them. Enabled skill instructions are appended below — follow them whenever the request matches the skill's description.

Routines: when the user wants something done on a schedule ("every minute", "daily", "keep checking"), routine_create sets it up in this chat — with a check whenever a tool can tell what's new, so you only wake for new items. routine_list, routine_update and routine_delete manage them.

Local shell: shell_run executes commands on the user's machine — their login PATH applies, so installed CLIs work. Use it when a request needs local state or a local tool instead of guessing; keep commands short and non-interactive.
${privateSkillsInject()}${pluginSkillsInject()}

When you DO write a widget module, this contract applies:
${contractFor("__NAME__")}`;

// Enabled private-skill bodies, appended to the agent system prompt so the
// model actually follows them.
export function privateSkillsInject() {
  try {
    const rows = db.prepare("SELECT name, description, body FROM skills WHERE enabled=1 ORDER BY created_at").all();

    if (!rows.length) return "";
    const parts = rows.map((r) => `### ${r.name}\n${r.description}\n${String(r.body || "").slice(0, 8000)}`);

    return `\nPrivate skills (active — follow when applicable):\n${parts.join("\n\n")}`.slice(0, 30000);
  } catch { return ""; }
}

// ————— tool executors —————

// The model's declared artifact metadata rides the turn (`art`): title and
// one-line description for the card header, declared width/height so the
// placeholder is drawn at the exact size the content will occupy.

export const clampDim = (n, lo, hi) => {
  const v = Math.round(Number(n));

  return v >= lo && v <= hi ? v : 0;
};

export function artOf(input) {
  const lay = input.layout || {};

  const art = {
    title: String(input.title || "").trim().slice(0, 60) || null,
    desc: String(input.desc || "").trim().slice(0, 140) || "",
    w: clampDim(lay.width ?? input.width, 60, 2048),
    h: clampDim(lay.height ?? input.height, 60, 2048),
  };

  return art.title || art.desc || art.w || art.h ? art : null;
}

// The generated image an image card's current version shows (its
// `asset:/generated/assets/<file>`), or null for any other card.
export function imageFileOf(name, v = null) {
  try {
    const path = v ? verFile(name, v) : join(GEN_DIR, `${name}.tsx`);

    return readFileSync(path, "utf8").match(/asset:\/generated\/assets\/(img-[\w.-]+)/)?.[1] || null;
  } catch {
    return null;
  }
}

// The gpt-image shape closest to the image at `path`.
function aspectOfFile(path) {
  try {
    const out = execFileSync("sips", ["-g", "pixelWidth", "-g", "pixelHeight", path], { encoding: "utf8" });
    const w = Number(out.match(/pixelWidth: (\d+)/)?.[1]);
    const h = Number(out.match(/pixelHeight: (\d+)/)?.[1]);

    if (w && h) return w > h * 1.15 ? "3:2" : h > w * 1.15 ? "2:3" : "1:1";
  } catch {}

  return "1:1";
}

export function imageModule(file, prompt, w, h) {
  const ar = w && h ? `, aspectRatio: ${(w / h).toFixed(4)}` : "";

  return `import { signal, Text, View } from "nsh";
import { Image } from "nsh/image";

export default function R() {
  const failed = signal(false);
  return (
    <View class="gap-2">
      <If when={!failed()}>
        <Image src="asset:/generated/assets/${file}" resizeMode="cover" onError={() => failed.set(true)} style={{ width: "100%"${ar}, borderRadius: 12 }} />
      </If>
      <If when={failed()}>
        <Text class="text-sm text-zinc-500">image failed to load</Text>
      </If>
    </View>
  );
}
`;
}

// `GET /module?name=mcp:<server>/<app>[|<props>]`: the shipped module with
// its props ahead of it as `globalThis.roxProps` (plus `server`, the id
// the app passes back to `/mcp/call`).
export async function mcpAppModule(raw) {
  const k = parseAppKey(raw);
  const row = k && db.prepare("SELECT * FROM mcp_servers WHERE id=? AND enabled=1").get(k.server);

  if (!row) return { source: "", error: "no such server" };
  const res = (await mcpListApps(row)).find((r) => appOfUri(r.uri) === k.app);

  if (!res) return { source: "", error: `${k.server} ships no app ${k.app}` };
  const text = await mcpReadApp(row, res.uri);
  const props = { ...k.props, server: row.id };

  return { source: text && `globalThis.roxProps = ${JSON.stringify(props)};\n${text}`, title: res.title || res.name || k.app };
}

// `POST /mcp/call {server, tool, args}` — a mini app calling its server.
export async function mcpAppCall({ server, tool, args }) {
  const row = db.prepare("SELECT * FROM mcp_servers WHERE id=? AND enabled=1").get(String(server || ""));

  if (!row) return { ok: false, error: "no such server" };
  const name = String(tool || "");

  // the per-tool switch binds apps as it binds the agent (mcpSurface)
  if (new Set(JSON.parse(row.disabled_tools || "[]")).has(name))
    return { ok: false, error: `${name} is switched off for ${row.name || row.id}; turn it on in Plugins to let apps call it` };

  return mcpCallJson(row, name, args || {});
}

// `GET /mcp/apps` — every enabled server's mini apps.
export async function mcpAppList() {
  const rows = db.prepare("SELECT * FROM mcp_servers WHERE enabled=1").all();

  const lists = await Promise.all(rows.map(async (row) =>
    (await mcpListApps(row).catch(() => [])).map((r) => ({
      server: row.id, app: appOfUri(r.uri), uri: r.uri, title: r.title || r.name || "", description: r.description || "",
    }))));

  return lists.flat();
}

// `GET /mcp/manifest` — what every enabled server offers the composer: its
// mini apps plus the slash commands and `@` scopes its `rox://manifest`
// announces. A server with neither is left out.
export async function mcpManifest() {
  const rows = db.prepare("SELECT * FROM mcp_servers WHERE enabled=1").all();
  const named = (xs) => (Array.isArray(xs) ? xs.filter((x) => x && isString(x.name) && x.name) : []);

  const servers = await Promise.all(rows.map(async (row) => {
    const [apps, m] = await Promise.all([mcpListApps(row).catch(() => []), mcpReadManifest(row).catch(() => ({}))]);

    return {
      id: row.id,
      title: String(m.title || row.name || row.id),
      apps: apps.map((r) => ({ id: appOfUri(r.uri), title: r.title || r.name || "", description: r.description || "" })),
      commands: named(m.commands),
      scopes: named(m.scopes),
    };
  }));

  return servers.filter((s) => s.apps.length || s.commands.length || s.scopes.length);
}

// `POST /mcp/resolve` lives with the attachments that scope it.
export { mcpResolve } from "./attachments.mjs";

// A server's `rox/open` ({uri, props?}): the reply carries a card for that
// mini app and the client opens it full screen when the job reports it.
async function noteAppOpen(job, row, open) {
  const app = appOfUri(open.uri);

  if (!job || !app) return;
  const res = (await mcpListApps(row)).find((r) => r.uri === open.uri);
  const props = open.props && ["object", "array", "other"].includes(kindOf(open.props)) ? open.props : {};
  const key = appKey(row.id, app, props);
  const title = open.title || res?.title || res?.name || app;
  job.openApp = { key, title };
  job.linkTurn ||= { links: [] };
  const links = job.linkTurn.links.filter((l) => l.id !== key);
  // `token` names what the app shows (an attachable value), carried on the link
  const token = isString(open.token) ? open.token : "";
  const link = `rox://app/${row.id}/${app}?props=${encodeURIComponent(JSON.stringify(props))}${token ? `&token=${encodeURIComponent(token)}` : ""}`;
  const card = { provider: "mcp-app", id: key, name: title, link, action: "open", kind: "app", file_type: res?.description || "Mini app" };

  if (token) card.token = token;
  links.push(card);
  job.linkTurn.links = links;
}

export async function toolGenerateApp(job, thread, input) {
  let code = String(input.code || "").trim();

  if (!code.includes("export default") && /function\s+R\s*\(/.test(code))
    code += "\nexport default R;\n";

  if (!code.includes("export default") || !code.includes("<"))
    return {
      content:
        "not a module: it needs `export default` on a component returning JSX.",
      is_error: true,
    };

  // input.name (set by revise_app) revises an existing artifact: the code is
  // a new version parented to base_v (or current), never an overwrite.
  const revName = /^R\d+$/.test(String(input.name || ""))
    ? String(input.name)
    : null;

  if (revName && !existsSync(join(GEN_DIR, `${revName}.tsx`)))
    return { content: `no such artifact ${revName}`, is_error: true };
  const name = revName || `R${nextSeq()}`;
  setJob(job, "checking", "Checking the code");
  code = code.split("__NAME__").join(name);

  // Models that self-name their state key (const S = "tipcalc") save under a
  // key the /state/<module> lookups (pop-outs, other surfaces) can never find —
  // pin the state const to the real name.
  if (code.includes("/state/"))
    code = code.replace(
      /const\s+(S|NAME|WIDGET|WIDGET_NAME|KEY|MOD|MODULE|SELF)\s*=\s*(["'`])([a-z][\w-]{1,40})\2/,
      (m) => m.replace(/["'`][a-z][\w-]{1,40}["'`]/, JSON.stringify(name)),
    );
  code = fixImports(code);
  writeFileSync(join(STAGE_DIR, `${name}.tsx`), code);
  // The card exists before the check: it renders as an exact-size
  // placeholder whose status line mirrors the job's phase. A failed call
  // removes it again — the model's retry makes a fresh one. A revise
  // instead covers the card it revises (`revising`); its new card lands
  // once the code passes (a rejected one stays covered while the model
  // fixes it; the job's end uncovers it).
  // The placeholder drawn while the code streamed becomes this card.
  const pre = job?.pendingApp && thread.turns.includes(job.pendingApp) ? job.pendingApp : null;

  if (job) job.pendingApp = null;

  const turn = Object.assign(pre || { id: uuidv7() }, {
    role: "ai",
    name,
    building: true,
    status: "checking types…",
    label: labelOf(modelCtx.getStore()),
    caption: String(input.caption || ""),
    art: artOf(input) || pre?.art || null,
  });

  if (revName) {
    if (pre) thread.turns.splice(thread.turns.indexOf(pre), 1);
    markRevising(job, cardTurn(thread, revName, Number(input.base_v) || null));
  } else {
    if (!pre) thread.turns.push(turn);

    if (job) job.turn = turn;
  }

  const dropTurn = () => {
    const i = thread.turns.indexOf(turn);

    if (i >= 0) thread.turns.splice(i, 1);

    if (job?.turn === turn) job.turn = null;
  };

  try {
    const err = await checkCandidate(name);

    if (err) {
      dropTurn();

      return {
        content: `nsh check rejected ${name}:\n${err.slice(0, 3000)}\nFix the module and call generate_app again with the corrected code.`,
        is_error: true,
      };
    }
  } catch (e) {
    dropTurn();
    throw e;
  }

  // Revising off a specific version (base_v) branches; default parent is the
  // current pointer. Nothing earlier is touched — cards keep their version.
  const headBefore = revName ? headOf(name) : null;

  const parent = revName
    ? db.prepare("SELECT v FROM versions WHERE name=? AND v=?").get(name, Number(input.base_v) || 0)
        ? Number(input.base_v)
        : headBefore
    : null;

  const branched = revName && parent != null && parent !== headBefore;

  // The apps row lands FIRST so writeVersion names the file <slug>.v<n>.tsx.
  const prior = db
    .prepare("SELECT title,file FROM apps WHERE name=?")
    .get(name) || {};

  const title = String(input.title || "").trim().slice(0, 60) || prior.title || null;
  const file = slugFile(input.slug) || prior.file || null;
  // generate_app's desc is also "what the app does"; revise_app's says what
  // changed, so only an explicit description replaces it there.
  const description = String(input.description || (revName ? "" : input.desc) || "").trim().slice(0, 280);
  const cols = { title, file };

  if (description) cols.description = description;
  setAppCols(name, cols);

  const v = writeVersion(
    name, code, parent, job?.prompt || input.caption || "",
    input.branch_name,
  );

  // the placeholder turn becomes the real card — pinned to this version
  turn.v = v;
  delete turn.building;
  delete turn.status;

  if (job?.turn === turn) job.turn = null;

  if (revName) {
    thread.turns.push(turn);
    clearRevising(job);
  }

  job.cards = (job.cards || 0) + 1;
  writeIndex();
  console.log(`✓ ${name}@v${v}.tsx via ${labelOf(modelCtx.getStore())} (tool)`);
  // The icon draws in the background — never blocks or fails the result.
  const iconPrompt = String(input.icon_prompt || "").trim();

  if (!existsSync(iconPath(name)) || (revName && iconPrompt))
    genAppIcon(name, iconPrompt, modelCtx.getStore()?.model || MODEL).catch((e) =>
      console.error(`  icon ${name}: ${String(e?.message || e).slice(0, 200)}`),
    );

  const branchNote = branched
    ? ` — this is a new branch off v${parent}${input.branch_name ? ` named "${String(input.branch_name).trim().slice(0, 40)}"` : ""}; mention the fork to the user (they can rename it in Library history)`
    : "";

  return {
    content: `mounted as ${name} v${v}${branchNote} — the card is live in the conversation. Library now launches v${v}; earlier versions stay on the tree.`,
  };
}

export async function toolGenerateImage(job, thread, input) {
  const prompt = String(input.prompt || "").trim();

  if (!prompt) return { content: "prompt is required", is_error: true };
  setJob(job, "generating", "Sketching something up");
  // An edit starts from that card's current image and keeps its shape.
  const editOf = /^R\d+$/.test(String(input.edit_of || "")) ? String(input.edit_of) : null;
  const src = editOf ? imageFileOf(editOf) : null;

  if (editOf && !src) return { content: `${editOf} is not an image card`, is_error: true };
  // The size is settled before the placeholder lands, so it is drawn at the
  // final image's exact shape.
  const aspect = IMAGE_ASPECTS[input.aspect] ? input.aspect : src ? aspectOfFile(join(ASSETS_DIR, src)) : "1:1";
  const [w, h] = IMAGE_ASPECTS[aspect];
  const art = artOf(input) || { title: null, desc: "" };

  // An edit keeps the card's title unless a new one is given.
  if (editOf && !art.title)
    art.title = [...thread.turns].reverse().find((t) => t.name === editOf && t.art?.title)?.art.title || null;
  art.w = w;
  art.h = h;
  // An image card: Rox opens its file in the lightbox.
  art.kind = "image";

  const turn = {
    id: uuidv7(),
    role: "ai",
    building: true,
    status: "generating image…",
    label: "image",
    caption: String(input.caption || ""),
    art,
  };

  thread.turns.push(turn);

  if (job) job.turn = turn;

  const dropTurn = () => {
    const i = thread.turns.indexOf(turn);

    if (i >= 0) thread.turns.splice(i, 1);

    if (job?.turn === turn) job.turn = null;
  };

  // gpt-image on the ChatGPT login (opencodex's relay), else Gemini.
  const file = `img-${Date.now()}.png`;
  art.file = file;
  let backend;

  try {
    backend = src
      ? await editImage(join(ASSETS_DIR, src), prompt, join(ASSETS_DIR, file), { size: `${w}x${h}` })
      : await generateImage(prompt, join(ASSETS_DIR, file), { size: `${w}x${h}`, aspect });
  } catch (e) {
    dropTurn();

    return { content: `image generation failed: ${String(e?.message || e).slice(0, 300)}`, is_error: true };
  }

  const name = editOf || `R${nextSeq()}`;
  const v = writeVersion(name, imageModule(file, prompt, w, h), editOf ? headOf(editOf) : null, prompt);
  turn.name = name;
  turn.v = v;
  // what drew it — Edit Details' "Made with"
  art.via = backend;
  delete turn.building;
  delete turn.status;

  if (job?.turn === turn) job.turn = null;
  job.cards = (job.cards || 0) + 1;
  console.log(`✓ ${name}.tsx image via ${backend} (${prompt.slice(0, 50)})`);

  return {
    content: `image generated and mounted as ${name} (asset:/generated/assets/${file}) — to embed it inside a widget, use <Image src="asset:/generated/assets/${file}" …>.`,
  };
}

export async function toolFetchUrl(input) {
  const url = String(input.url || "");

  const r = await fetch(url, {
    signal: AbortSignal.timeout(20000),
    redirect: "follow",
    headers: { "user-agent": "Mozilla/5.0 (grok-shell)" },
  });

  if (!r.ok) return { content: `fetch failed: ${r.status}`, is_error: true };
  const html = await r.text();

  const text = html
    .replace(/<script[\s\S]*?<\/script>/gi, "")
    .replace(/<style[\s\S]*?<\/style>/gi, "")
    .replace(/<[^>]+>/g, " ")
    .replace(/&nbsp;/g, " ")
    .replace(/&amp;/g, "&")
    .replace(/\s+/g, " ")
    .trim()
    .slice(0, 6000);

  return { content: text || "(empty page)" };
}

// shell_run — a real zsh login shell on the user's machine (`-l` loads their
// PATH: homebrew, cargo, nodenv, …). Non-interactive by contract; capped.
export function toolShell(input) {
  const command = String(input.command || "").trim();

  if (!command) return { content: "shell_run needs a command", is_error: true };
  const timeout = Math.min(Math.max(Number(input.timeout_ms) || 30000, 1000), 180000);
  const cwd = input.cwd ? String(input.cwd) : process.env.HOME || "/";

  return new Promise((resolve) => {
    execFile("/bin/zsh", ["-lc", command], { cwd, timeout, maxBuffer: 8 << 20, killSignal: "SIGKILL" }, (err, stdout, stderr) => {
      let out = "";

      if (stdout) out += stdout.trimEnd();

      if (stderr) out += (out ? "\n" : "") + stderr.trimEnd();

      if (err) {
        const code = err.killed ? "timeout" : (err.code ?? "?");
        out += `${out ? "\n" : ""}(exit ${code})`;
      }

      resolve({ content: (out || "(no output)").slice(0, 50000), is_error: !!err });
    });
  });
}

export async function toolQuerySource(input) {
  const src = String(input.source || "");

  const url =
    src === "linear"
      ? "http://127.0.0.1:8787/data/linear/issues"
      : `http://127.0.0.1:8787/data/${src}`;

  try {
    const r = await fetch(url, { signal: AbortSignal.timeout(25000) });
    const j = await r.json();

    return { content: JSON.stringify(j).slice(0, 12000) };
  } catch (e) {
    return { content: `source error: ${String(e?.message || e)}`, is_error: true };
  }
}

export async function toolWebSearch(input) {
  // Client-side fallback (DuckDuckGo html) — only used if the model rejects
  // the Anthropic server-side web_search tool.
  const q = encodeURIComponent(String(input.query || ""));

  const r = await fetch(`https://html.duckduckgo.com/html/?q=${q}`, {
    signal: AbortSignal.timeout(20000),
    headers: { "user-agent": "Mozilla/5.0 (grok-shell)" },
  });

  if (!r.ok) return { content: `search failed: ${r.status}`, is_error: true };
  const html = await r.text();

  const results = [
    ...html.matchAll(
      /<a[^>]*class="result__a"[^>]*href="([^"]+)"[^>]*>([\s\S]*?)<\/a>/g,
    ),
  ]
    .slice(0, 8)
    .map((m) => {
      const href = m[1].replace(/.*uddg=/, "").split("&")[0];
      const title = m[2].replace(/<[^>]+>/g, "").trim();

      return `${title}\n${decodeURIComponent(href)}`;
    });

  return { content: results.join("\n\n") || "(no results)" };
}

// The agent's tapback — same store the /react endpoint writes, so a
// tool-made reaction lands on the user bubble's top-left badge identically.
export function toolReact(thread, input) {
  const emoji = Array.from(String(input.emoji || "")).slice(0, 16).join("");
  let i = Number.isInteger(input.index) ? Number(input.index) : -1;

  if (i < 0 || i >= thread.turns.length) {
    for (i = thread.turns.length - 1; i >= 0; i--)
      if (thread.turns[i].role === "user") break;
  }

  const turn = thread.turns[i];

  if (!turn) return { content: "no message to react to", is_error: true };
  turn.reaction = emoji || undefined;
  writeIndex();

  return { content: emoji ? `reacted ${emoji} to turn ${i}` : `cleared reaction on turn ${i}` };
}

// A file an office tool produced becomes a card on its own turn — like a
// widget card, it lands the moment it exists instead of waiting for the reply.
export function cardOfficeFile(thread, rel) {
  const p = outFile(rel);

  if (!p || !existsSync(p)) return;
  thread.turns.push({ id: uuidv7(), role: "ai", label: "office", files: [fileRecord(p, rel)] });
}

// The building placeholder of an office file, shaped like its output: a
// page (8.5:11) for a document, 16:9 for a deck, 4:3 for a sheet.
export function officeArt(name) {
  const [kind, w, h] = /\.pptx?$/i.test(name)
    ? ["presentation", 560, 315]
    : /\.(xlsx|xlsm|xls|csv)$/i.test(name)
      ? ["spreadsheet", 480, 360]
      : [/\.pdf$/i.test(name) ? "pdf" : "document", 425, 550];

  return { title: name || null, desc: "", w, h, kind };
}

export async function toolOffice(job, thread, block) {
  const input = block.input || {};

  if (block.name === "office_create") {
    const name = String(input.path || "").split("/").pop() || "";
    const format = /\.xlsx$/i.test(name) ? "xlsx" : /\.pptx$/i.test(name) ? "pptx" : /\.docx$/i.test(name) ? "docx" : null;
    // The placeholder drawn while the payload streamed (agent.mjs) becomes
    // the file's turn, as generate_app adopts its own; without one a fresh
    // placeholder shows while the file is written.
    const pre = job?.pendingOffice && thread.turns.includes(job.pendingOffice) ? job.pendingOffice : null;

    if (job) job.pendingOffice = null;

    const dropPre = () => {
      const i = pre ? thread.turns.indexOf(pre) : -1;

      if (i >= 0) thread.turns.splice(i, 1);
    };

    if (!format) {
      dropPre();

      return { content: "path must end in .docx, .xlsx, or .pptx", is_error: true };
    }

    const payload = input[format];

    if (!payload) {
      dropPre();

      return { content: `a .${format} path requires a ${format} payload`, is_error: true };
    }

    const turn = Object.assign(pre || { id: uuidv7() }, {
      role: "ai",
      building: true,
      status: "Putting the file together",
      label: "office",
      art: officeArt(name),
    });

    if (!pre) thread.turns.push(turn);

    if (job) job.turn = turn;

    const dropTurn = () => {
      const i = thread.turns.indexOf(turn);

      if (i >= 0) thread.turns.splice(i, 1);

      if (job?.turn === turn) job.turn = null;
    };

    const rel = `o${Date.now().toString(36)}`;
    mkdirSync(join(OUT_DIR, rel), { recursive: true });

    try {
      writeFileSync(join(OUT_DIR, rel, name), await officeCreateBytes(format, { [format]: payload }));
    } catch (e) {
      dropTurn();

      return { content: `office_create failed: ${String(e?.message || e).slice(0, 400)}`, is_error: true };
    }

    const key = `${rel}/${name}`;
    // the placeholder turn becomes the file card
    turn.files = [fileRecord(join(OUT_DIR, key), key)];
    delete turn.art;
    delete turn.building;
    delete turn.status;

    if (job?.turn === turn) job.turn = null;
    writeIndex();
    console.log(`✓ office ${format} → ${key}`);

    return { content: `created ${key} — the file card is in the reply. To revise it, open this path with office_open and export the result.` };
  }

  const r = await callOfficeTool(OUT_DIR, block.name, input);

  if (r.is_error) return r;

  // office_export's JSON result carries the workspace-relative path of the
  // file it wrote — card it so the new file shows up in the reply.
  if (block.name === "office_export" || block.name === "office_create") {
    try {
      const rel = JSON.parse(r.content).path;

      if (isString(rel) && rel) cardOfficeFile(thread, rel);
    } catch {}
  }

  return r;
}

// Artifact action tools ride the same endpoints the UI uses (self-POST over
// the loopback server — always listening by the time a tool runs), except
// view_code/set_version which are pure reads/writes of the version store.
export async function toolArtifact(block, input) {
  const name = String(input.name || "");

  if (!/^R\d+$/.test(name))
    return { content: `bad artifact name ${name}`, is_error: true };

  const self = async (path, b) => {
    const r = await fetch(`http://127.0.0.1:8787${path}`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(b),
    });

    return r.json().catch(() => ({}));
  };

  if (block.name === "launch_app") {
    const pop = { name };

    if (input.v) pop.v = Number(input.v);
    const r = await self("/popout", pop);

    return r?.ok
      ? { content: `${name}${input.v ? ` v${input.v}` : ""} opened in its own window.` }
      : { content: `launch failed: ${r?.error || "no such module"}`, is_error: true };
  }

  if (block.name === "close_app") {
    let n = 0;

    for (const [k, child] of openWindows)
      if (k === name || k.startsWith(`${name}@`))
        try { child.kill(); n++; } catch {}

    return { content: n ? `closed ${n} window(s).` : `no open window for ${name}.` };
  }

  if (block.name === "promote_app") {
    const r = await self("/publish", { name });

    return r?.ok
      ? { content: `${name} is now in Mini Apps.` }
      : { content: `promote failed: ${r?.error || "?"}`, is_error: true };
  }

  if (block.name === "demote_app") {
    const r = await self("/unpublish", { name });

    return r?.ok
      ? { content: `${name} removed from Mini Apps.` }
      : { content: `demote failed: ${r?.error || "?"}`, is_error: true };
  }

  if (block.name === "set_version") {
    const v = Number(input.v);

    return setHead(name, v)
      ? { content: `${name} now points at v${v} — Library launches it; later versions stay on the tree.` }
      : { content: `no version ${v} on ${name}`, is_error: true };
  }

  if (block.name === "view_code") {
    const v = input.v ? Number(input.v) : headOf(name);
    const file = v ? verFile(name, v) : join(GEN_DIR, `${name}.tsx`);

    if (!existsSync(file))
      return { content: `no source for ${name}${v ? `@v${v}` : ""}`, is_error: true };

    return {
      content: `<code name="${name}@v${v ?? ""}">\n${readFileSync(file, "utf8").slice(0, 60000)}\n</code>`,
    };
  }

  return { content: `unknown artifact tool ${block.name}`, is_error: true };
}

export async function execTool(job, thread, block) {
  try {
    if (block.name === "generate_app")
      return await toolGenerateApp(job, thread, block.input || {});

    if (block.name === "revise_app") {
      const input = { ...block.input };

      if (!/^R\d+$/.test(String(input.name || "")))
        return { content: "revise_app needs the artifact name (R<n>)", is_error: true };

      return await toolGenerateApp(job, thread, input);
    }

    if (block.name === "generate_image")
      return await toolGenerateImage(job, thread, block.input || {});

    if (block.name === "react") return toolReact(thread, block.input || {});

    if (block.name === "app_tools_load") return await toolAppToolsLoad(thread, block.input || {});

    if (block.name === "fetch_url") return await toolFetchUrl(block.input || {});

    if (block.name === "shell_run") return await toolShell(block.input || {});

    if (block.name === "query_source")
      return await toolQuerySource(block.input || {});

    if (block.name === "web_search")
      return await toolWebSearch(block.input || {});

    if (block.name === "app_icon") {
      const { name, prompt } = block.input || {};
      setJob(job, "generating", "Sketching something up");
      const backend = await genAppIcon(String(name || ""), prompt, modelCtx.getStore()?.model || MODEL);

      return { content: `${name} has a new icon (drawn by ${backend}).` };
    }

    if (block.name === "app_details_update") {
      const { name, width, height, ...rest } = block.input || {};
      const meta = { ...rest };

      if (width !== undefined) meta.w = width;

      if (height !== undefined) meta.h = height;
      updateAppMeta(String(name || ""), meta);
      const i = appInfo(String(name));

      return { content: `${name} details: title "${i.title}", file ${i.file}, description "${i.description}", size ${i.w}×${i.h}` };
    }

    if (/^(launch_app|close_app|promote_app|demote_app|set_version|view_code)$/.test(block.name))
      return await toolArtifact(block, block.input || {});

    if (block.name.startsWith("office_"))
      return await toolOffice(job, thread, block);

    if (/^mcp_(install|uninstall|enable|disable|list)$/.test(block.name))
      return await toolMcpAdmin(thread, block.name, block.input || {});

    if (/^skill_(create|update|delete|enable|disable|list)$/.test(block.name))
      return toolSkillAdmin(block.name, block.input || {});

    if (isBotTool(block.name)) return toolBot(thread, block.name, block.input || {});

    if (isRoutineTool(block.name)) return await toolRoutine(thread, block.name, block.input || {});

    // a v1 plugin's tool, under its own name (v1/agent.mjs)
    if (v1ToolPlugin(v1Plugins, block.name, job?.v1Surface)) {
      setJob(job, "connecting", "Working in a connected app");

      return await v1AgentCall(v1Plugins, job, block.name, block.input || {});
    }

    const mm = parseMcpName(block.name);

    if (mm) {
      const row = db
        .prepare("SELECT * FROM mcp_servers WHERE id=?")
        .get(mm[0]);

      if (!row) return { content: `mcp server ${mm[0]} is not configured`, is_error: true };

      if (!row.enabled) return { content: `mcp server ${mm[0]} is switched off; the user enables it in Plugins`, is_error: true };
      setJob(job, ...toolActivity(block.name));

      // the thread's bot actually used this server → it joins its integrations
      try { recordToolUse(thread, row.id); } catch {}

      markAppUsed(thread, row.id);
      const { meta, ...r } = await mcpCallTool(row, mm[1], block.input || {});

      // a Drive file the AI made or changed joins the reply (drive.mjs)
      if (row.id === DRIVE_SERVER) try { noteDriveCall(job, thread, mm[1], block.input || {}, r); } catch {}

      // the Google address a call named: routine templates fall back on it
      if (!r.is_error) noteGoogleEmail(block.input);
      // a server asking to show one of its mini apps: it opens full screen
      // and its card joins the reply
      const open = meta?.["rox/open"];

      if (open?.uri && !r.is_error) try { await noteAppOpen(job, row, open); } catch {}

      return r;
    }

    return { content: `unknown tool ${block.name}`, is_error: true };
  } catch (e) {
    return {
      content: `tool error: ${String(e?.message || e).slice(0, 500)}`,
      is_error: true,
    };
  }
}

// The Anthropic server-side web_search tool gets one shot — if this build of
// the API/model rejects it we drop it and use the client-side version.
export let serverSearch = true;
export let agentStream = true;

export const setServerSearch = (v) => {
  serverSearch = v;
};

export const setAgentStream = (v) => {
  agentStream = v;
};

// Run status, phrased the way Grok Bot does: a phase (→ the avatar's
// animation in Rox) plus a short phrase; Rox adds the "…". One phrase is
// drawn per phase and kept while that phase lasts (phraseFor).
export const PHRASES = {
  thinking: ["Thinking it through", "Mulling it over", "Putting it together"],
  working: ["Getting it done", "Making progress", "Working on it", "Handling it", "Chipping away"],
};

export function phraseFor(job, key, pool = PHRASES[key] || PHRASES.working) {
  if (job._phraseKey !== key) {
    job._phraseKey = key;
    job._phrase = pool[Math.floor(Math.random() * pool.length)];
  }

  return job._phrase;
}

export const TOOL_ACTIVITY = {
  web_search: ["searching", "Digging around"],
  fetch_url: ["browsing", "Doing some reading"],
  query_source: ["connecting", "Working in a connected app"],
  generate_app: ["coding", "Writing some code"],
  revise_app: ["coding", "Writing some code"],
  view_code: ["reading", "Reading the file"],
  generate_image: ["generating", "Sketching something up"],
  app_icon: ["generating", "Sketching something up"],
  app_details_update: ["writing", "Jotting it down"],
  mcp_install: ["connecting", "Setting up a connected app"],
  mcp_uninstall: ["connecting", "Working in a connected app"],
  mcp_enable: ["connecting", "Working in a connected app"],
  mcp_disable: ["connecting", "Working in a connected app"],
  mcp_list: ["connecting", "Working in a connected app"],
  shell_run: ["running-commands", "Running a few commands"],
  office_create: ["writing", "Putting the file together"],
  office_export: ["writing", "Putting the file together"],
  skill_create: ["writing", "Jotting it down"],
  skill_update: ["writing", "Jotting it down"],
  memory_add: ["writing", "Jotting it down"],
  memory_remove: ["writing", "Jotting it down"],
  bot_profile_update: ["writing", "Jotting it down"],
};

// A connected app's display name for "Working in <app>".
function appName(serverId) {
  const row = db.prepare("SELECT name FROM mcp_servers WHERE id=?").get(String(serverId));

  return row?.name || serverId;
}

const titleCase = (s) => s.trim().replace(/[-_]+/g, " ").replace(/\b\w/g, (c) => c.toUpperCase());

export function toolActivity(name, input = {}) {
  // What a search/read is about, as Grok Bot shows it ("Searching for
  // bbtree"). The block's input is empty while it streams, then complete
  // when the tool runs.
  const clip = (t) => (t.length > 40 ? `${t.slice(0, 39)}…` : t);

  if (name === "web_search" && isString(input?.query) && input.query.trim())
    return ["searching", `Searching for ${clip(input.query.trim())}`];

  if (name === "fetch_url" && isString(input?.url)) {
    try { return ["browsing", `Reading ${new URL(input.url).hostname.replace(/^www\./, "")}`]; } catch {}
  }

  if (name === "query_source" && isString(input?.source) && input.source.trim())
    return ["connecting", `Working in ${titleCase(input.source)}`];

  if (name === "app_tools_load" && input?.app) return ["connecting", `Working in ${titleCase(appName(input.app))}`];
  const mcp = parseMcpName(name);

  if (mcp) return ["connecting", `Working in ${titleCase(appName(mcp[0]))}`];

  if (name.startsWith("office_")) return ["reading", "Reading the file"];

  return TOOL_ACTIVITY[name] || ["working", PHRASES.working[0]];
}

// Apps with more tools than this load on demand (Vercel's ~240 tools, ~90
// KB): app_tools_load adds one for the thread. A few dozen tools cost about
// half a second per call (measured), less than the extra round a load takes.
const DEFER_OVER = 80;
const LOADED_TTL = 15 * 60 * 1000;
const loadedApps = new Map(); // thread id -> Map(server id -> last used)

function loadedFor(thread) {
  const m = loadedApps.get(thread?.id) || new Map();

  for (const [id, at] of m) if (Date.now() - at > LOADED_TTL) m.delete(id);

  return m;
}

export function markAppUsed(thread, serverId) {
  const m = loadedFor(thread);

  if (!m.has(serverId)) return;
  m.set(serverId, Date.now());
}

// A thread's bot scopes which connected MCP servers the agent sees:
// global servers (bot IS NULL) plus servers bound to this bot. → the
// callable defs plus the large apps left to load on demand. A v1 routine's
// job narrows the v1 tools to its plugin and its `reads` (job.v1Surface).
async function mcpSurface(thread, job) {
  const bot = store.bots.find((b) => b.threadId === thread?.id);

  const rows = db
    .prepare("SELECT * FROM mcp_servers WHERE enabled=1 AND (bot IS NULL OR bot=?)")
    .all(bot?.id || "");

  const loaded = loadedFor(thread);
  const defs = [];
  const deferred = [];

  for (const row of rows) {
    const rec = await mcpEnsure(row);

    if (rec.needsAuth || rec.unavailable) continue; // signed out or blocked: its calls would only fail
    const off = new Set(JSON.parse(row.disabled_tools || "[]"));
    const tools = rec.tools.filter((t) => !off.has(t.name));

    if (tools.length > DEFER_OVER && !loaded.has(row.id)) {
      deferred.push({ id: row.id, name: row.name || row.id, count: tools.length, sample: tools.slice(0, 6).map((t) => t.name) });
      continue;
    }

    defs.push(...mcpAnthropicTools(row.id, tools));
  }

  // v1 plugins: their own tool names, approved plugins only; a plugin over
  // DEFER_OVER tools waits for app_tools_load like a v0 server
  const taken = new Set([...TOOLS, ...ROUTINE_TOOLS, ...BOT_TOOLS, ...defs].map((t) => t.name));
  const byPlugin = Map.groupBy(v1Tools(v1Plugins, taken, job?.v1Surface), (t) => t.plugin);

  for (const [plugin, tools] of byPlugin) {
    const short = v1Plugins.rowOf(plugin)?.short;

    if (tools.length > DEFER_OVER && !loaded.has(`v1:${short}`)) {
      deferred.push({ id: short, name: v1Plugins.manifestOf(v1Plugins.rowOf(plugin)).title, count: tools.length, sample: tools.slice(0, 6).map((t) => t.name) });
      continue;
    }

    defs.push(...tools.map(({ plugin: _p, ...t }) => t));
  }

  return { defs, deferred };
}

export const mcpToolDefs = async (thread) => (await mcpSurface(thread)).defs;

const appToolsLoad = (deferred) => ({
  name: "app_tools_load",
  description:
    "Load a connected app's tools so you can call them (they become callable on your next step and stay loaded for this chat). " +
    "Load one only when the request needs that app. Apps: " +
    deferred.map((d) => `${d.id} (${d.name}, ${d.count} tools, e.g. ${d.sample.join(", ")})`).join("; "),
  input_schema: {
    type: "object",
    properties: { app: { type: "string", enum: deferred.map((d) => d.id), description: "The app id" } },
    required: ["app"],
  },
});

// app_tools_load → marks the app loaded for the thread; its mcp_* tools
// join the tool list from the next model call on.
async function toolAppToolsLoad(thread, input) {
  const id = String(input?.app || "");
  const row = db.prepare("SELECT * FROM mcp_servers WHERE id=? AND enabled=1").get(id);
  const v1 = !row && v1Plugins.rows().find((r) => r.short === id && r.enabled);

  if (v1) {
    const m = loadedFor(thread);
    m.set(`v1:${id}`, Date.now());
    loadedApps.set(thread?.id, m);
    const names = v1Tools(v1Plugins).filter((t) => t.plugin === v1.id).map((t) => t.name);

    return { content: `Loaded ${names.length} ${id} tools; call them from your next step:\n${names.join("\n")}` };
  }

  if (!row) return { content: `no connected app ${id}`, is_error: true };
  const rec = await mcpEnsure(row);

  if (!rec.client) return { content: `${id} is not connected (${rec.err || "failed to start"})`, is_error: true };
  const m = loadedFor(thread);
  m.set(id, Date.now());
  loadedApps.set(thread?.id, m);
  const off = new Set(JSON.parse(row.disabled_tools || "[]"));
  const names = rec.tools.filter((t) => !off.has(t.name)).map((t) => mcpAnthropicTools(id, [t])[0].name);

  return { content: `Loaded ${names.length} ${row.name || id} tools; call them from your next step:\n${names.join("\n")}` };
}

export async function toolList(thread, job) {
  // The bundled BetterOffice MCP server contributes its office_* surface once
  // connected (office_create stays ours — its version mounts the file card).
  const office = officeMcpTools();
  const { defs: mcp, deferred } = await mcpSurface(thread, job);
  const own = botOfThread(thread) ? BOT_TOOLS : [];
  const load = deferred.length ? [appToolsLoad(deferred)] : [];

  return serverSearch
    ? [{ type: "web_search_20250305", name: "web_search", max_uses: 3 }, ...TOOLS, ...ROUTINE_TOOLS, ...own, ...office, ...load, ...mcp]
    : [...TOOLS, ...ROUTINE_TOOLS, ...own, ...office, ...load, ...mcp, CLIENT_SEARCH];
}

// Shared by POST /mcp/servers and the mcp_install agent tool. Omitted fields
// keep their stored values so a partial call edits an existing row. byAgent
// (mcp_install) creates the row switched off and waiting for the user
// (agent_pending); a switched-off row is never spawned here.
export async function mcpUpsertServer({ id, name, command, args, env, bot, url, headers, oauth_client, transport, probe = true, byAgent = false } = {}) {
  const cur = id
    ? db.prepare("SELECT * FROM mcp_servers WHERE id=?").get(String(id))
    : null;

  const finalName = String(name ?? cur?.name ?? "").trim();
  const finalUrl = String(url ?? cur?.url ?? "").trim();
  const finalCommand = String(command ?? cur?.command ?? "").trim();

  if (!finalName) throw new Error("server needs a name");

  if (!finalCommand && !finalUrl) throw new Error("server needs a command or url");

  // a new row's id is what `@<server>:…` tokens and rox://app links name
  if (id && !cur && !isServerId(String(id))) throw new Error(`server id ${String(id).slice(0, 40)} must be [a-z0-9-]+`);

  const sid =
    (id && String(id)) ||
    (() => {
      const base = finalName.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "").slice(0, 24) || "srv";
      let s = base, i = 2;

      while (db.prepare("SELECT id FROM mcp_servers WHERE id=?").get(s)) s = `${base}-${i++}`;

      return s;
    })();

  const argList =
    args === undefined
      ? JSON.parse(cur?.args || "[]")
      : Array.isArray(args)
        ? args.map(String)
        : String(args || "").trim().split(/\s+/).filter(Boolean);

  const envObj =
    env === undefined
      ? JSON.parse(cur?.env || "{}")
      : env && ["object", "array", "other"].includes(kindOf(env))
        ? env
        : Object.fromEntries(
            String(env || "")
              .split("\n")
              .map((l) => l.match(/^([A-Za-z_][A-Za-z0-9_]*)=(.*)$/))
              .filter(Boolean)
              .map((m) => [m[1], m[2]]),
          );

  const botId =
    bot === undefined ? cur?.bot ?? null
    : bot && store.bots.some((b) => b.id === bot) ? bot
    : null;

  // headers / oauth_client / transport: undefined keeps the stored value.
  const keep = (v, col) => (v === undefined ? cur?.[col] ?? null : v == null ? null : isString(v) ? v : JSON.stringify(v));
  // REPLACE rewrites the whole row — carry the OAuth state, account label
  // and per-tool switches over so an edit doesn't sign the account out.
  db.prepare(
    "INSERT OR REPLACE INTO mcp_servers(id,name,command,args,env,enabled,bot,created_at,url,headers,oauth_client,transport,auth,account,disabled_tools,agent_pending) VALUES(?,?,?,?,?,COALESCE((SELECT enabled FROM mcp_servers WHERE id=?),?),?,COALESCE((SELECT created_at FROM mcp_servers WHERE id=?),?),?,?,?,?,?,?,?,?)",
  ).run(
    sid, finalName.slice(0, 60), finalCommand, JSON.stringify(argList), JSON.stringify(envObj), sid, byAgent ? 0 : 1, botId, sid, Date.now(), finalUrl || null,
    keep(headers, "headers"), keep(oauth_client, "oauth_client"), keep(transport, "transport"),
    cur?.auth ?? null, cur?.account ?? null, cur?.disabled_tools ?? null, cur ? cur.agent_pending ?? null : byAgent ? 1 : null,
  );
  mcpInvalidate(sid); // config changed → drop any cached client
  const row = db.prepare("SELECT * FROM mcp_servers WHERE id=?").get(sid);

  if (!row.enabled) return { id: sid, status: "off", tools: 0, toolNames: [], err: null };

  if (!probe) return { id: sid, status: "idle", tools: 0, toolNames: [], err: null };
  const rec = await mcpEnsure(row); // probe now so the reply carries real status
  const st = mcpStatus(sid);

  return {
    id: sid,
    status: rec.client ? "connected" : "error",
    tools: st.tools,
    toolNames: rec.tools.map((t) => mcpAnthropicTools(sid, [t])[0]?.name ?? ""),
    err: st.err || null,
  };
}

// OAuth persistence for remote MCP servers — mcp.mjs reads/writes the
// row's `auth` JSON through these; done() drops the cached client so the
// next ensureClient() connects with fresh tokens.
mcpSetAuthHooks({
  loadAuth: (id) => {
    const row = db.prepare("SELECT auth FROM mcp_servers WHERE id=?").get(String(id));

    try { return JSON.parse(row?.auth || "{}"); } catch { return {}; }
  },
  saveAuth: (id, obj) =>
    db.prepare("UPDATE mcp_servers SET auth=? WHERE id=?").run(JSON.stringify(obj), String(id)),
  done: (id) => mcpInvalidate(id),
});

// Resolve a server row by id, or by name (exact, then slugged).
export function mcpServerRow({ id, name } = {}) {
  if (id)
    return db.prepare("SELECT * FROM mcp_servers WHERE id=?").get(String(id));
  const n = String(name || "").trim();

  if (!n) return null;
  const slug = n.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "").slice(0, 24);

  return (
    db.prepare("SELECT * FROM mcp_servers WHERE name=?").get(n) ||
    db.prepare("SELECT * FROM mcp_servers WHERE id=?").get(slug)
  );
}

// mcp_install / mcp_uninstall / mcp_enable / mcp_disable / mcp_list — the
// model manages the same mcp_servers registry the Connect tab edits.
export async function toolMcpAdmin(thread, tool, input) {
  if (tool === "mcp_list") {
    const rows = db.prepare("SELECT * FROM mcp_servers ORDER BY created_at").all();

    if (!rows.length) return { content: "no MCP servers configured" };

    const line = (r) => {
      const s = mcpStatus(r.id);
      const envKeys = Object.keys(JSON.parse(r.env || "{}"));

      return `${r.id} — ${r.name} · ${r.enabled ? s.status + ` (${s.tools} tools)` : r.agent_pending ? "off, waiting for the user to enable it in Plugins" : "disabled"} · scope: ${r.bot ? `bot ${r.bot}` : "all bots"}${envKeys.length ? ` · env: ${envKeys.join(",")}` : ""}${s.err ? ` · err: ${s.err}` : ""}`;
    };

    return { content: rows.map(line).join("\n") };
  }

  // A server the agent adds runs a command of its choosing, so it is stored
  // switched off and never spawned until the user enables it in Plugins.
  if (tool === "mcp_install") {
    const bot = store.bots.find((b) => b.threadId === thread?.id);

    const r = await mcpUpsertServer({
      name: input.name,
      command: input.command,
      args: input.args,
      env: input.env,
      bot: input.scope === "bot" && bot ? bot.id : null,
      byAgent: true,
    });

    return { content: `added "${input.name}" as server ${r.id}, switched off: it has not been started. The user must review and enable it in Plugins before its tools exist; tell them so. You cannot enable it yourself.` };
  }

  const row = mcpServerRow(input);

  if (!row)
    return { content: `no MCP server matches ${input.id || input.name || "?"} — mcp_list shows the ids`, is_error: true };

  if (tool === "mcp_uninstall") {
    mcpInvalidate(row.id);
    db.prepare("DELETE FROM mcp_servers WHERE id=?").run(row.id);

    return { content: `removed MCP server ${row.id} (${row.name}) — its tools are gone` };
  }

  const enabled = tool === "mcp_enable";

  if (enabled && row.agent_pending)
    return { content: `${row.id} (${row.name}) waits for the user to enable it in Plugins; only they can switch on a server you added`, is_error: true };
  db.prepare("UPDATE mcp_servers SET enabled=? WHERE id=?").run(enabled ? 1 : 0, row.id);
  mcpInvalidate(row.id);

  return { content: `${row.id} (${row.name}) ${enabled ? "enabled — its tools return next step" : "disabled — its tools are withheld"}` };
}

// skill_create / skill_update / skill_delete / skill_enable / skill_disable /
// skill_list — private skills the model follows via the system prompt.
export const skillSlug = (s) =>
  String(s || "").toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "").slice(0, 48) || "skill";

export function skillRow(input) {
  if (input?.id) {
    const r = db.prepare("SELECT * FROM skills WHERE id=?").get(String(input.id));

    if (r) return r;
  }

  if (input?.name)
    return db.prepare("SELECT * FROM skills WHERE name=? OR id=?").get(String(input.name), skillSlug(input.name));

  return null;
}

export function toolSkillAdmin(tool, input) {
  if (tool === "skill_list") {
    const rows = db.prepare("SELECT * FROM skills ORDER BY created_at").all();

    if (!rows.length) return { content: "no private skills yet" };

    return { content: rows.map((r) => `${r.id} — ${r.name}${r.enabled ? "" : " (disabled)"} · ${r.description}`).join("\n") };
  }

  if (tool === "skill_create") {
    const name = skillSlug(input.name);
    const cur = db.prepare("SELECT id FROM skills WHERE id=?").get(name);
    db.prepare(
      "INSERT OR REPLACE INTO skills(id,name,description,body,enabled,created_at) VALUES(?,?,?,?,COALESCE((SELECT enabled FROM skills WHERE id=?),1),COALESCE((SELECT created_at FROM skills WHERE id=?),?))",
    ).run(name, String(input.name || name).slice(0, 80), String(input.description || "").slice(0, 400), String(input.instructions || input.body || ""), name, name, Date.now());

    return { content: `${cur ? "updated" : "created"} private skill "${name}" — its instructions apply to every bot's next step` };
  }

  const row = skillRow(input);

  if (!row)
    return { content: `no private skill matches ${input.id || input.name || "?"} — skill_list shows the ids`, is_error: true };

  if (tool === "skill_update") {
    db.prepare("UPDATE skills SET name=?, description=?, body=? WHERE id=?").run(
      input.name !== undefined ? String(input.name).slice(0, 80) : row.name,
      input.description !== undefined ? String(input.description).slice(0, 400) : row.description,
      input.instructions !== undefined || input.body !== undefined ? String(input.instructions ?? input.body) : row.body,
      row.id,
    );

    return { content: `skill "${row.id}" updated — applies from the next step` };
  }

  if (tool === "skill_delete") {
    db.prepare("DELETE FROM skills WHERE id=?").run(row.id);

    return { content: `deleted private skill "${row.id}"` };
  }

  const enabled = tool === "skill_enable";
  db.prepare("UPDATE skills SET enabled=? WHERE id=?").run(enabled ? 1 : 0, row.id);

  return { content: `skill "${row.id}" ${enabled ? "enabled — instructions apply" : "disabled — instructions withheld"}` };
}
