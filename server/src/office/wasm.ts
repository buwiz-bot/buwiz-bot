import { readFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const cache = new Map<string, Promise<Uint8Array>>();

/**
 * Load a BetterOffice engine wasm module.
 *
 * Bundled builds resolve the file beside the entry point
 * (`dist/office/generated/<format>_wasm_bg.wasm`, staged by clean-build).
 * Unbundled runs (tests, tsx) resolve the installed package and read its own
 * `generated/` directory. Callers should fall back to `init(undefined)` when
 * this rejects — the packages self-load wasm relative to their own module URL.
 */
export function loadOfficeWasm(format: "xlsx" | "pptx"): Promise<Uint8Array> {
  let pending = cache.get(format);
  if (!pending) {
    pending = resolveOfficeWasm(format);
    cache.set(format, pending);
  }
  return pending;
}

async function resolveOfficeWasm(format: "xlsx" | "pptx"): Promise<Uint8Array> {
  const file = `${format}_wasm_bg.wasm`;
  const bundled = await readFile(new URL(`./generated/${file}`, import.meta.url)).then(
    bytes => new Uint8Array(bytes),
    () => undefined,
  );
  if (bundled) return bundled;
  const entry = packageEntry(format === "xlsx" ? "@betteroffice/xlsx" : "@betteroffice/pptx");
  if (!entry) throw new Error(`Cannot resolve @betteroffice/${format} for wasm loading`);
  return new Uint8Array(await readFile(join(dirname(entry), "generated", file)));
}

function packageEntry(name: string): string | undefined {
  try {
    return createRequire(import.meta.url).resolve(name);
  } catch {
    // ESM-only packages reject require.resolve under some Node versions.
  }
  try {
    return fileURLToPath(import.meta.resolve(name));
  } catch {
    return undefined;
  }
}
