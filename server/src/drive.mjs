// drive.mjs — Google Drive files the AI made or changed, as Library
// artifacts. A successful google-workspace write (create, import, copy,
// update/rename, doc / sheet / slide edits — never a read or a search)
// leaves a link on the AI turn that made it:
//   turn.links = [{provider: "gdrive", id, name, mime, link, action, at,
//                  account?}]
// one per file id (merged within the turn); action "created" | "edited" |
// "renamed". artifacts.mjs turns them into `gdrive:<id>` records stored
// with the connector and versioned by Drive. Names come from the write's
// own result or input, else from what reads in the same reply showed, else
// from an earlier turn's link of the same file.

export const DRIVE_SERVER = "google-workspace";

// workspace-mcp's write tools → what they do to a file (and the mime of
// the file they make, when the tool fixes it).
const DOC = "application/vnd.google-apps.document";
const SHEET = "application/vnd.google-apps.spreadsheet";
const SLIDES = "application/vnd.google-apps.presentation";
const WRITES = {
  create_drive_file: { action: "created" },
  import_to_google_doc: { action: "created", mime: DOC },
  import_to_google_sheets: { action: "created", mime: SHEET },
  import_to_google_slides: { action: "created", mime: SLIDES },
  copy_drive_file: { action: "created" },
  create_doc: { action: "created", mime: DOC },
  create_spreadsheet: { action: "created", mime: SHEET },
  create_presentation: { action: "created", mime: SLIDES },
  export_doc_to_pdf: { action: "created", mime: "application/pdf" },
  update_drive_file: { action: "edited" },
  modify_doc_text: { action: "edited", mime: DOC },
  find_and_replace_doc: { action: "edited", mime: DOC },
  insert_doc_elements: { action: "edited", mime: DOC },
  insert_doc_image: { action: "edited", mime: DOC },
  update_paragraph_style: { action: "edited", mime: DOC },
  update_doc_headers_footers: { action: "edited", mime: DOC },
  batch_update_doc: { action: "edited", mime: DOC },
  create_table_with_data: { action: "edited", mime: DOC },
  manage_doc_tab: { action: "edited", mime: DOC },
  modify_sheet_values: { action: "edited", mime: SHEET },
  format_sheet_range: { action: "edited", mime: SHEET },
  create_sheet: { action: "edited", mime: SHEET },
  manage_sheet_tab: { action: "edited", mime: SHEET },
  append_table_rows: { action: "edited", mime: SHEET },
  resize_sheet_dimensions: { action: "edited", mime: SHEET },
  move_sheet_rows: { action: "edited", mime: SHEET },
  manage_conditional_formatting: { action: "edited", mime: SHEET },
  manage_named_range: { action: "edited", mime: SHEET },
  batch_update_presentation: { action: "edited", mime: SLIDES },
};

// A Drive file's Library kind and its type's name, by mime — else by the
// name's extension (an edit's result may not say what the file is).
const DRIVE_TYPES = [
  [/google-apps\.document$/, /^$/, "document", "Google Doc"],
  [/google-apps\.spreadsheet$/, /^$/, "spreadsheet", "Google Sheet"],
  [/google-apps\.presentation$/, /^$/, "presentation", "Google Slides"],
  [/wordprocessingml|msword|opendocument\.text|rtf/, /^(docx?|odt|rtf)$/, "document", "Word document"],
  [/spreadsheetml|ms-excel|opendocument\.spreadsheet|text\/csv|tab-separated/, /^(xlsx|xlsm|xls|csv|tsv|ods)$/, "spreadsheet", "Spreadsheet"],
  [/presentationml|ms-powerpoint|opendocument\.presentation/, /^(pptx?|odp)$/, "presentation", "Presentation"],
  [/^application\/pdf$/, /^pdf$/, "pdf", "PDF"],
  [/^image\//, /^(png|jpe?g|gif|webp|heic|svg)$/, "image", "Image"],
  [/^(audio|video)\//, /^(mp3|wav|m4a|aac|mov|mp4|m4v|webm)$/, "media", "Media"],
  [/^text\//, /^(txt|md|markdown)$/, "document", "Text"],
];
export function driveType(mime, name) {
  for (const [re, , kind, type] of DRIVE_TYPES) if (re.test(mime || "")) return { kind, type };
  const ext = /\.([a-z0-9]+)$/i.exec(name || "")?.[1].toLowerCase() || "";
  for (const [, re, kind, type] of DRIVE_TYPES) if (ext && re.test(ext)) return { kind, type };
  return { kind: "file", type: ext ? ext.toUpperCase() : "File" };
}

// The file's link when a result carries none: Docs, Sheets and Slides
// have their editors, anything else Drive's viewer.
const linkFor = (id, mime) =>
  mime === DOC ? `https://docs.google.com/document/d/${id}/edit`
  : mime === SHEET ? `https://docs.google.com/spreadsheets/d/${id}/edit`
  : mime === SLIDES ? `https://docs.google.com/presentation/d/${id}/edit`
  : `https://drive.google.com/file/d/${id}/view`;
const ID = "[\\w-]{10,}";
const pick = (text, re) => text.match(re)?.[1] || "";

/** The file a successful write `tool` (called with `input`) touched, read
 *  off its result `text` — null for a write that touched none (a resumable
 *  upload session not yet used, an error reply, a folder). Result shapes
 *  (workspace-mcp 2.2):
 *    create_drive_file  "Successfully created file 'N' (ID: X) in folder … Link: L"
 *    import_to_google_* "✅ Successfully imported 'N' as …\n   Document ID: X\n … Link: L"
 *    copy_drive_file    "New file ID: X\nNew file name: N\nFile type: M\n…View copied file: L"
 *    create_doc         "Created Google Doc 'N' (ID: X) for … Link: L"
 *    create_spreadsheet "Successfully created spreadsheet 'N' for …. ID: X | URL: L | …"
 *    create_presentation "- Title: N\n- Presentation ID: X\n- URL: L"
 *    export_doc_to_pdf  "… saved to Drive as 'N' (ID: X, … bytes). PDF: L | Original: …"
 *    update_drive_file  "✅ Successfully updated file: N\n   File ID: X\n…• Name: 'A' → 'B'…View file: L"
 *    doc / sheet / slide edits name the id in their input (document_id,
 *    spreadsheet_id, presentation_id) and a docs link at most. */
export function driveWrite(tool, input = {}, text = "") {
  const w = WRITES[tool];
  text = String(text || "");
  if (!w || /^\s*Error\b/i.test(text) || /Resumable upload session|Upload URL \(no Authorization/i.test(text)) return null;
  let id = "", name = "", mime = w.mime || "", link = "", action = w.action;
  switch (tool) {
    case "create_drive_file":
      id = pick(text, new RegExp(`\\(ID: (${ID})\\)`));
      name = pick(text, /created file '(.*?)' \(ID:/) || input.file_name || "";
      mime = input.mime_type || "";
      // a folder is no file
      if (/folder/.test(mime)) return null;
      break;
    case "import_to_google_doc":
    case "import_to_google_sheets":
    case "import_to_google_slides":
      id = pick(text, new RegExp(`(?:Document|Spreadsheet|Presentation|File) ID: (${ID})`));
      name = pick(text, /Successfully imported '(.*?)' as/) || input.file_name || "";
      break;
    case "copy_drive_file":
      id = pick(text, new RegExp(`New file ID: (${ID})`));
      name = pick(text, /New file name: (.+)/);
      mime = pick(text, /File type: (\S+)/);
      if (mime === "Unknown") mime = "";
      link = pick(text, /View copied file: (https?:\S+)/);
      break;
    case "create_doc":
      id = pick(text, new RegExp(`\\(ID: (${ID})\\)`));
      name = pick(text, /Created Google Doc '(.*?)' \(ID:/) || input.title || "";
      break;
    case "create_spreadsheet":
      id = pick(text, new RegExp(`ID: (${ID}) \\|`)) || pick(text, new RegExp(`ID: (${ID})`));
      name = pick(text, /created spreadsheet '(.*?)' for/) || input.title || "";
      link = pick(text, /URL: (https?:\S+)/);
      break;
    case "create_presentation":
      id = pick(text, new RegExp(`Presentation ID: (${ID})`));
      name = pick(text, /- Title: (.+)/) || input.title || "";
      link = pick(text, /URL: (https?:\S+)/);
      break;
    case "export_doc_to_pdf":
      id = pick(text, new RegExp(`saved to Drive as '.*?' \\(ID: (${ID})`));
      name = pick(text, /saved to Drive as '(.*?)' \(ID:/);
      link = pick(text, /PDF: (https?:\S+)/);
      break;
    case "update_drive_file": {
      id = pick(text, new RegExp(`File ID: (${ID})`)) || input.file_id || "";
      name = pick(text, /Successfully updated file: (.+)/) || input.name || "";
      mime = input.mime_type || "";
      link = pick(text, /View file: (https?:\S+)/);
      // a rename and nothing else is a rename
      const changes = text.split("\n").filter((l) => /^\s*•/.test(l));
      if (changes.length && changes.every((l) => /•\s*Name:/.test(l))) action = "renamed";
      if (/No changes were made/.test(text)) return null;
      // a file sent to the trash is no file to keep in the Library
      if (changes.some((l) => /moved to trash/.test(l))) return null;
      break;
    }
    default:
      id = input.document_id || input.spreadsheet_id || input.presentation_id || input.file_id || "";
      // Docs tools take a URL or an id
      id = String(id).match(new RegExp(`/d/(${ID})`))?.[1] || String(id);
  }
  // the first file link in the result, when the tool's own label missed
  link ||= pick(text, /(https:\/\/(?:docs|drive)\.google\.com\/\S+)/);
  link = link.replace(/[).,]+$/, "");
  if (!id || !new RegExp(`^${ID}$`).test(id)) return null;
  return {
    provider: "gdrive",
    id,
    name: String(name || "").trim().replace(/^['"]|['"]$/g, ""),
    mime,
    link: link && link !== "#" && !/No link available|N\/A/.test(link) ? link : "",
    action,
  };
}

/** What any google-workspace result shows of files (searches, listings,
 *  reads): id → {name, mime, link}, added to `known`.
 *    `- Name: "N" (ID: X, Type: M, …) Link: L`   (search / list)
 *    `File: "N" (ID: X, Type: M)`                 (doc / file content)
 *    `Spreadsheet: "N" (ID: X)`, `- "N" (ID: X)`   (sheet info, listings)
 *    `- Title: N\n- Presentation ID: X`           (presentations) */
export function harvestFiles(text, known) {
  text = String(text || "");
  for (const line of text.split("\n")) {
    const m = line.match(new RegExp(`"([^"]+)" \\(ID: (${ID})(?:, Type: ([^,)\\s]+))?`));
    if (!m) continue;
    const link = line.match(/Link: (https?:\/\/\S+)/)?.[1] || "";
    known.set(m[2], { name: m[1], mime: m[3] || known.get(m[2])?.mime || "", link: link || known.get(m[2])?.link || "" });
  }
  const p = text.match(new RegExp(`- Title: (.+)\\n- Presentation ID: (${ID})`));
  if (p) known.set(p[2], { name: p[1].trim(), mime: SLIDES, link: "" });
}

/** Merge link `l` into `turn.links` by file id: the newest name, mime and
 *  link win where known; a file the turn created stays "created". */
export function recordLink(turn, l) {
  turn.links ||= [];
  const cur = turn.links.find((x) => x.provider === l.provider && x.id === l.id);
  if (!cur) return turn.links.push({ ...l, at: l.at || Date.now() }), turn.links;
  for (const k of ["name", "mime", "link", "account"]) if (l[k]) cur[k] = l[k];
  if (cur.action !== "created") cur.action = l.action;
  cur.at = l.at || Date.now();
  return turn.links;
}

// What an earlier turn of `thread` knows of file `id` (newest first).
function priorLink(thread, id) {
  for (let i = (thread?.turns?.length || 0) - 1; i >= 0; i--) {
    const l = (thread.turns[i].links || []).find((x) => x.id === id);
    if (l) return l;
  }
  return null;
}

/** After a google-workspace call by the agent: a read's files are noted
 *  (names for later writes); a successful write's file is recorded on the
 *  reply being built (`job.links` — agent.mjs puts them on its turn). */
export function noteDriveCall(job, thread, tool, input, result) {
  if (!job || result?.is_error) return;
  const text = String(result?.content || "");
  job.driveSeen ||= new Map();
  harvestFiles(text, job.driveSeen);
  const l = driveWrite(tool, input, text);
  if (!l) return;
  const seen = job.driveSeen.get(l.id) || {};
  const prior = priorLink(thread, l.id) || {};
  l.name ||= seen.name || prior.name || "";
  l.mime ||= seen.mime || prior.mime || "";
  l.link ||= seen.link || prior.link || linkFor(l.id, l.mime);
  if (input?.user_google_email) l.account = String(input.user_google_email);
  else if (prior.account) l.account = prior.account;
  job.linkTurn ||= { links: [] };
  recordLink(job.linkTurn, l);
}
