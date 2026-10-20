import { defineConfig } from "oxlint";

export default defineConfig({
  ignorePatterns: [
    ".agent/**",
    ".agents/**",
    ".claude/**",
    ".codex/**",
    ".continue/**",
    ".cursor/**",
    ".gemini/**",
    ".opencode/**",
    ".pi/**",
    ".roo/**",
    ".windsurf/**",
    // Repo-owned agent/tooling and non-source trees:
    ".nsh/**",
    ".stow/**", // nested grok-shell-stow checkout — another repo's content
    "tools/oxlint/anti-slop/**",
    "node_modules/**",
    // Generated output: model-created apps, the registry, runtime pop-outs,
    // build bundles, and the bundled office editor.
    "generated/**",
    "widgets/R*.tsx", // published Mini Apps are runtime artifacts
    "windows/**",
    "out/**",
    "server/office/editor/dist/**",
    // Vendored/generated mark engine assets: motion.mjs is a verbatim
    // extraction of Grok Bot's client engine; *.data.ts are baked solver
    // tables. Not owned style.
    "app/mark/motion.mjs",
    "app/mark/*.data.ts",
  ],
  jsPlugins: [
    { name: "anti-slop", specifier: "./tools/oxlint/anti-slop/index.ts" },
  ],
  rules: {
    "oxc/no-accumulating-spread": "error",
    "anti-slop/no-array-filter-map": "error",
    "anti-slop/no-reduce-accumulator-copy": "error",
    "anti-slop/no-chained-type-assertions": "error",
    "anti-slop/no-conditional-empty-object-spread": "error",
    "anti-slop/no-known-value-widening": "error",
    "anti-slop/no-module-mocking": "error",
    "anti-slop/no-object-parameters": "error",
    "anti-slop/no-reflect-apply": "error",
    "anti-slop/no-reflect-get": "error",
    "anti-slop/no-runtime-typeof": "error",
    "anti-slop/no-shape-in-symbol-names": "error",
    "anti-slop/no-unknown-parameters": "error",
    "anti-slop/no-unknown-returns": "error",
    "anti-slop/no-unknown-type-aliases": "error",
    "anti-slop/no-unsafe-dictionary-type": "error",
    "anti-slop/no-widen-then-assert": "error",
    "anti-slop/require-readable-spacing": "error",
    "anti-slop/require-safety-comment-for-type-assertion": "error",
  },
});
