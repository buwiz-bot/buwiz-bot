// v1/jcs.mjs — JSON Canonicalization Scheme (RFC 8785) and I-JSON parsing.
//
// Every signed object of the Mini App Protocol (the manifest, session
// certificates, call envelopes) is signed over its JCS bytes, and Rox's
// Rust canonicalizer must produce the same bytes (both are tested against
// test/fixtures/jcs-vectors.json). Signed objects carry integers only, so
// the signing form refuses any other number instead of trusting two float
// printers to agree. `JSON.parse` keeps the last of duplicate keys; I-JSON
// (RFC 7493) refuses them, so manifests are parsed with parseIJson.

export class JcsError extends Error {}

/** A decoded JSON value's kind: null, boolean, number, string, array or object
 *  ("other" for anything JSON has no form for). The one place v1 asks a
 *  value what it is: everything else branches on these names. */
export function kindOf(v) {
  if (v === null) return "null";

  if (Array.isArray(v)) return "array";

  // oxlint-disable-next-line anti-slop/no-runtime-typeof -- this is the boundary that decodes JSON values
  const t = typeof v;

  if (t === "object") return Object.getPrototypeOf(v) === Object.prototype || Object.getPrototypeOf(v) === null ? "object" : "other";

  return ["boolean", "number", "string"].includes(t) ? t : "other";
}

export const isObject = (v) => kindOf(v) === "object";
export const isString = (v) => kindOf(v) === "string";
export const isInt = (v) => Number.isSafeInteger(v);

// A string with a lone surrogate is not I-JSON (RFC 7493 2.1).
const LONE = /[\uD800-\uDBFF](?![\uDC00-\uDFFF])|(?<![\uD800-\uDBFF])[\uDC00-\uDFFF]/;

function str(s) {
  if (LONE.test(s)) throw new JcsError("a string holds a lone surrogate");

  return JSON.stringify(s);
}

function num(n, integersOnly) {
  if (!Number.isFinite(n)) throw new JcsError("NaN and Infinity have no JSON form");

  if (integersOnly && !Number.isSafeInteger(n)) throw new JcsError(`signed objects carry safe integers only, not ${n}`);

  // ECMAScript Number serialization is RFC 8785's number form (3.2.2.3)
  return JSON.stringify(n);
}

function canon(v, integersOnly) {
  const k = kindOf(v);

  if (k === "null" || k === "boolean") return String(v);

  if (k === "number") return num(v, integersOnly);

  if (k === "string") return str(v);

  if (k === "array") return `[${v.map((x) => canon(x, integersOnly)).join(",")}]`;

  if (k !== "object") throw new JcsError(`${k} has no JSON form`);

  // keys sort by UTF-16 code units, which is what Array.prototype.sort does
  const keys = Object.keys(v).sort();

  return `{${keys.map((key) => `${str(key)}:${canon(v[key], integersOnly)}`).join(",")}}`;
}

/** RFC 8785 text of any JSON value. */
export const canonicalize = (v) => canon(v, false);

/** RFC 8785 text of a value that is about to be signed or verified:
 *  integers only (safe range), anything else throws JcsError. */
export const canonicalSigned = (v) => canon(v, true);

/** The UTF-8 bytes signatures cover. */
export const signedBytes = (v) => Buffer.from(canonicalSigned(v), "utf8");

// ── I-JSON parsing (duplicate keys refused) ──────────────────────────────

const WS = new Set([" ", "\t", "\n", "\r"]);
const NUMBER = /-?(?:0|[1-9]\d*)(?:\.\d+)?(?:[eE][+-]?\d+)?/y;

// oxlint-disable-next-line eslint/no-control-regex -- JSON forbids raw control characters in strings
const STRING = /"(?:[^"\\\u0000-\u001f]|\\(?:["\\/bfnrt]|u[0-9a-fA-F]{4}))*"/y;

function parser(text) {
  let i = 0;

  const fail = (why) => {
    throw new JcsError(`${why} at offset ${i}`);
  };

  const skip = () => {
    while (WS.has(text[i])) i++;
  };

  const lex = (re) => {
    re.lastIndex = i;
    const m = re.exec(text);

    if (!m) fail("malformed JSON");
    i += m[0].length;

    return m[0];
  };

  const string = () => {
    const s = JSON.parse(lex(STRING));

    if (LONE.test(s)) fail("a string holds a lone surrogate");

    return s;
  };

  const list = (close, item) => {
    i++;
    skip();

    if (text[i] === close) return i++;

    for (;;) {
      item();
      skip();

      if (text[i] === close) return i++;

      if (text[i] !== ",") fail(`expected , or ${close}`);
      i++;
      skip();
    }
  };

  const value = (depth) => {
    if (depth > 512) fail("nested too deep");
    skip();
    const c = text[i];

    if (c === "{") {
      const o = {};

      list("}", () => {
        if (text[i] !== '"') fail("expected a key");
        const key = string();

        if (Object.hasOwn(o, key)) fail(`duplicate key ${JSON.stringify(key).slice(0, 80)}`);
        skip();

        if (text[i] !== ":") fail("expected :");
        i++;
        // defineProperty: a "__proto__" key is data, not the prototype
        Object.defineProperty(o, key, { value: value(depth + 1), enumerable: true, writable: true, configurable: true });
      });

      return o;
    }

    if (c === "[") {
      const a = [];

      list("]", () => a.push(value(depth + 1)));

      return a;
    }

    if (c === '"') return string();

    for (const [word, v] of [["true", true], ["false", false], ["null", null]])
      if (text.startsWith(word, i)) {
        i += word.length;

        return v;
      }

    return Number(lex(NUMBER));
  };

  return () => {
    const v = value(0);

    skip();

    if (i !== text.length) fail("trailing text");

    return v;
  };
}

/** JSON text → value, refusing duplicate keys, lone surrogates and any
 *  malformed input with JcsError. */
export const parseIJson = (text) => parser(String(text))();
