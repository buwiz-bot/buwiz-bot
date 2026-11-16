// BetterOffice editor shell — bundled by esbuild into dist/editor.js.
// Mounts the real @betteroffice/*-react editors (same components the
// betteroffice demo wraps) against a generated-file path, saving edits
// back over POST /file/write. Everything after mount is the engine's UI.
import { createRoot } from "react-dom/client";
import { createElement as h, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { DocxEditor, configureDefaultFonts } from "@betteroffice/docx-react";
import { PptxEditor } from "@betteroffice/pptx-react";
import { XlsxEditor } from "@betteroffice/xlsx-react";
import * as fontsModule from "@betteroffice/fonts";
import { BUNDLED_FONTS, loadBundledFontBytes } from "@betteroffice/fonts";

const q = new URLSearchParams(location.search);
const path = q.get("path") || "";
const kind = q.get("kind") || "";
const raw = "/file/raw/" + path.split("/").map(encodeURIComponent).join("/");
const name = path.split("/").pop() || `document.${kind}`;
const fontBase = "/office/vendor/fonts/assets/";

// docx layout/measuring wants { fonts: module, baseUrl } — passing a provider
// instance silently leaves it fontless, and unmeasured glyphs never wrap.
configureDefaultFonts?.({ fonts: fontsModule, baseUrl: fontBase });
async function pptxFaces() {
  const faces = [];
  for (const f of BUNDLED_FONTS.filter((f) => !f.script)) {
    try {
      const bytes = new Uint8Array(await loadBundledFontBytes(f, { baseUrl: fontBase }));
      for (const fam of new Set([f.metricCompatWith, f.family].filter(Boolean)))
        faces.push({ family: fam, bold: f.weight === 700, italic: f.style === "italic", bytes });
    } catch {}
  }
  return faces;
}

function b64(buf) {
  const u8 = buf instanceof Uint8Array ? buf : new Uint8Array(buf);
  let bin = "";
  for (let i = 0; i < u8.length; i += 0x8000) bin += String.fromCharCode(...u8.subarray(i, i + 0x8000));
  return btoa(bin);
}

function saveBytes(bytes) {
  return fetch("/file/write", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ path, data: b64(bytes) }),
  });
}

// Rox's Save (its ⌘S, its ⌘K menu): the editor's own save — the same
// export its toolbar's Save runs, whose bytes land in onSave and so in
// /file/write (which keeps each save as a new version). An editor with no
// save() on its ref gets the Cmd+S its own keymap answers.
let editorRef = null;
window.roxSave = async () => {
  const ed = editorRef?.current;
  if (ed?.save) {
    await ed.flushPendingInput?.();
    const out = await ed.save();
    return !!out;
  }
  const target = document.activeElement || document.body;
  target.dispatchEvent(new KeyboardEvent("keydown", { key: "s", code: "KeyS", metaKey: true, bubbles: true, cancelable: true }));
  return true;
};

function App() {
  const ref = useRef(null);
  editorRef = ref;
  const [bytes, setBytes] = useState(null);
  const [fonts, setFonts] = useState(null);
  const [err, setErr] = useState(null);
  useEffect(() => {
    fetch(raw)
      .then((r) => (r.ok ? r.arrayBuffer() : Promise.reject(new Error(`file: ${r.status}`))))
      .then((b) => setBytes(b))
      .catch((e) => setErr(String(e.message || e)));
    if (kind === "pptx") pptxFaces().then(setFonts);
  }, []);
  // Editors wire onSave into effect deps — an unstable closure re-runs the
  // effect on every render and loops into React #185. Same for props objects.
  const onSave = useCallback((b) => saveBytes(b).catch(() => {}), []);
  // className is NOT passed to the editors: it would replace their scoped
  // .oox-root class that styles.css is compiled under, leaving all chrome
  // unstyled (the overflow rail then measures ~15kpx and loops into #185).
  const common = useMemo(() => ({ ref, fileName: name, documentName: name, onSave }), [onSave]);
  const fileU8 = useMemo(() => (bytes ? new Uint8Array(bytes) : null), [bytes]);
  if (err) return h("div", { id: "err" }, `Couldn't open — ${err}`);
  if (!bytes || (kind === "pptx" && !fonts)) return h("div", { id: "load" }, "Opening…");
  // documentBuffer takes an ArrayBuffer (a Uint8Array breaks an internal
  // identity memo → React #185); the pptx/xlsx `file` props take Uint8Array.
  // showFileOpen: false — the viewer is bound to one generated file, so File >
  // Open (and Cmd+O) are hidden; Save writes back to that file.
  const editor =
    kind === "docx"
      ? h(DocxEditor, { ...common, documentBuffer: bytes, showFileOpen: false, showRuler: true, showZoomControl: true })
      : kind === "xlsx"
        ? h(XlsxEditor, { ...common, file: fileU8 })
        : h(PptxEditor, { ...common, file: fileU8, fonts });
  // The window's own title bar carries the name — the page adds no chrome.
  return h("div", { id: "shell" }, h("div", { className: "bo-editor" }, editor));
}

createRoot(document.getElementById("root")).render(h(App));
