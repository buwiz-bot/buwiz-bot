// v1/errors.mjs — the spec's error codes (14.1) as one error type.
//
// Every refusal the broker hands Rox is {ok:false, code, message,
// retryable} with a code from the spec, so Rox can map it to a RoxError
// or a plugin state without parsing messages. Whether a code is retryable
// is the spec's table, not the thrower's choice.

const RETRYABLE = new Set(["rate_limited", "timeout", "plugin_unavailable", "plugin_restarted", "internal"]);

export class V1Error extends Error {
  constructor(code, message, details) {
    super(message || code);
    this.code = code;
    this.retryable = RETRYABLE.has(code);

    if (details) this.details = details;
  }
}

/** Throws V1Error(code, message). */
export const refuse = (code, message, details) => {
  throw new V1Error(code, message, details);
};

/** The wire form of any error: a V1Error as is, anything else `internal`. */
export function errorBody(e) {
  const v = e instanceof V1Error ? e : new V1Error("internal", String(e?.message || e));
  const body = { ok: false, code: v.code, message: String(v.message).slice(0, 500), retryable: v.retryable };

  if (v.details) body.details = v.details;

  return body;
}
