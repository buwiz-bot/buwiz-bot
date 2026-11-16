// The data helper of this window (hub::sources): the widget reads and changes the
// person's data through source(name).call(action, args), never through a URL of its own.
// It resolves to the action's result and never throws: a failure is { error, message }.
const CALL_URL: string = ${url};
(globalThis as any).source = (name: string) => ({
  call: async (action: string, args?: Record<string, unknown>) => {
    try {
      const r = await fetch(CALL_URL, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ source: name, action, args: args ?? {} }),
      });
      const j = await r.json();
      if (j && j.ok) return j.result;
      const e = (j && j.error) || {};
      return { error: String(e.code || "source_error"), message: String(e.message || "the data source did not answer") };
    } catch (e: any) {
      return { error: "source_error", message: String((e && e.message) || e) };
    }
  },
});
export {};
