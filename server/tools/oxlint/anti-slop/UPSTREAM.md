# anti-slop provenance

- Source: https://github.com/dmmulroy/anti-slop
- Installed from commit: `13909c9e07590f5a85d93c62712259fe14a883f7`
  (fetched 2025-10-20 via the `install-anti-slop` agent skill's bundled assets,
  which track upstream `src/`)
- Plugin entry point: `index.ts` (vendored at `tools/oxlint/anti-slop/`)
- Config: `oxlint.config.mts` at the repo root
- Versions: `oxlint` + `@oxlint/plugins` pinned exactly at 1.89.0

## Local deviations from upstream assets

- Added `package.json` with `"type": "module"` in this directory so Node
  resolves the ESM plugin under the repo's CommonJS default.
- `oxlint.config.mts` (not `.ts`) for the same reason.
- Extra repo-specific ignores in the config: `.nsh/`, `.stow/`,
  `generated/`, `widgets/R*.tsx`, `windows/`, `out/`,
  `tools/office/editor/dist/`, `app/mark/motion.mjs`, `app/mark/*.data.ts`
  (vendored/generated mark-engine assets).
- Effect plugin not enabled: no direct `effect` dependency.
