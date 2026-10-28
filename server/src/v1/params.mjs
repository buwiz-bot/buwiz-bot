// v1/params.mjs — a routine's `params` (spec 5.9, "Routine instances"):
// the settings one instance of a routine runs with, declared by the plugin
// as a small JSON Schema and given by whoever makes the instance.
//
// The schema is a subset: {type: "object", properties: {<name>: <prop>},
// required?: [<name>], additionalProperties?: false}, where a prop is
// {type: "string"|"integer"|"number"|"boolean", description?, enum?,
// maxLength?} or {type: "array", items: <a non-array prop>, maxItems?,
// description?}. Property names are lower snake case. Params are checked
// against it when an instance is made or changed; extra keys are refused.

const NAME = /^[a-z][a-z0-9_]{0,39}$/;
const SCALARS = new Set(["string", "integer", "number", "boolean"]);
const KEYS = new Set(["type", "description", "enum", "maxLength", "items", "maxItems"]);
export const PARAMS_MAX_BYTES = 4096;

const isObj = (v) => !!v && typeof v === "object" && !Array.isArray(v);

/** Why `schema` is no params schema ("" when it is one). */
export function schemaProblem(schema) {
  if (!isObj(schema) || schema.type !== "object" || !isObj(schema.properties)) return 'must be {type: "object", properties: {...}}';

  for (const k of Object.keys(schema)) if (!["type", "properties", "required", "additionalProperties", "description"].includes(k)) return `has unknown key ${k}`;

  if (schema.additionalProperties !== undefined && schema.additionalProperties !== false) return "additionalProperties may only be false";
  const names = Object.keys(schema.properties);

  if (!names.length || names.length > 16) return "declares 1 to 16 properties";

  for (const n of names) {
    if (!NAME.test(n)) return `property ${n.slice(0, 40)} is not a lower snake case name`;
    const p = schema.properties[n];
    const why = propProblem(p, true);

    if (why) return `property ${n} ${why}`;
  }

  if (schema.required !== undefined && (!Array.isArray(schema.required) || schema.required.some((r) => !names.includes(r))))
    return "required names only declared properties";

  if (JSON.stringify(schema).length > PARAMS_MAX_BYTES) return `is over ${PARAMS_MAX_BYTES} bytes`;

  return "";
}

function propProblem(p, arrays) {
  if (!isObj(p)) return "must be an object";

  for (const k of Object.keys(p)) if (!KEYS.has(k)) return `has unknown key ${k}`;

  if (p.description !== undefined && typeof p.description !== "string") return "description must be a string";

  if (p.type === "array") {
    if (!arrays) return "may not nest arrays";

    if (p.maxItems !== undefined && !(Number.isInteger(p.maxItems) && p.maxItems > 0)) return "maxItems must be a positive integer";

    return propProblem(p.items, false) && `items ${propProblem(p.items, false)}`;
  }

  if (!SCALARS.has(p.type)) return "type must be string, integer, number, boolean or array";

  if (p.items !== undefined || p.maxItems !== undefined) return "has items but is no array";

  if (p.enum !== undefined && (!Array.isArray(p.enum) || !p.enum.length || p.enum.some((v) => valueProblem({ type: p.type }, v)))) return `enum must list ${p.type} values`;

  if (p.maxLength !== undefined && !(p.type === "string" && Number.isInteger(p.maxLength) && p.maxLength > 0)) return "maxLength is for strings, a positive integer";

  return "";
}

function valueProblem(p, v) {
  if (p.type === "array") {
    if (!Array.isArray(v)) return "must be a list";

    if (p.maxItems && v.length > p.maxItems) return `has more than ${p.maxItems} items`;

    for (const x of v) {
      const why = valueProblem(p.items, x);

      if (why) return `item ${why}`;
    }

    return "";
  }

  const ok = { string: typeof v === "string", integer: Number.isInteger(v), number: typeof v === "number" && Number.isFinite(v), boolean: typeof v === "boolean" }[p.type];

  if (!ok) return `must be a ${p.type}`;

  if (p.enum && !p.enum.includes(v)) return `must be one of ${p.enum.join(", ")}`;

  if (p.maxLength && v.length > p.maxLength) return `is longer than ${p.maxLength}`;

  return "";
}

/** Why `params` don't fit `schema` ("" when they do). A routine without a
 *  schema takes no params ({}). */
export function paramsProblem(schema, params) {
  if (!isObj(params)) return "params must be an object";

  if (JSON.stringify(params).length > PARAMS_MAX_BYTES) return `params are over ${PARAMS_MAX_BYTES} bytes`;

  if (!schema) return Object.keys(params).length ? "this routine takes no params" : "";

  for (const k of Object.keys(params)) if (!schema.properties[k]) return `params.${k.slice(0, 40)} is not one of ${Object.keys(schema.properties).join(", ")}`;

  for (const k of schema.required || []) if (params[k] === undefined) return `params.${k} is required`;

  for (const [k, v] of Object.entries(params)) {
    const why = valueProblem(schema.properties[k], v);

    if (why) return `params.${k} ${why}`;
  }

  return "";
}

const clip = (s, n) => (s.length > n ? `${s.slice(0, n - 1)}…` : s);

/** One line for an instance's params, in the schema's order: "taxpayers:
 *  tp_1…, tp_2 · forms: 2550Q". "" for none. */
export function paramsSummary(schema, params) {
  const keys = schema ? Object.keys(schema.properties).filter((k) => params?.[k] !== undefined) : Object.keys(params || {});

  return clip(
    keys
      .map((k) => {
        const v = params[k];

        return `${k}: ${Array.isArray(v) ? v.map((x) => clip(String(x), 24)).join(", ") : clip(String(v), 40)}`;
      })
      .join(" · "),
    160,
  );
}
