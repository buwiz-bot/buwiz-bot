//! The build pipeline (plan 5.7a; Node's `pipeline.mjs`): `nsh build` and
//! `nsh check` in `.stage` (`nsh`), import fixing (`imports`), runtime
//! chunks (`chunks`) and the `generated/<name>/<slug>.v<k>.tsx` version tree
//! with its `versions` and `heads` rows (`versions`). A library: the tool
//! loop (5.4) and `/chat` (5.2) call it; it owns `versions` and `heads`.
//!
//! `generated/index.tsx` and `widgets/index.tsx` are the store's (5.1):
//! `setHead` has the store write them.

pub mod chunks;
pub mod imports;
pub mod nsh;
pub mod versions;

use std::path::{Path, PathBuf};

use crate::js;

/// The hub's nsh: the shell-out (`NSH`, else `nsh`), unless a test set one.
pub fn install(ctx: &mut crate::hub::ctx::HubCtx) {
    if ctx.nsh.is_none() {
        ctx.nsh = Some(std::sync::Arc::new(nsh::NshCli::from_env()));
    }
}

/// Where the pipeline reads and writes, under Node's data dir
/// (config.mjs's GEN_DIR, STAGE_DIR, CHUNK_DIR, WIDGETS_DIR).
#[derive(Clone, Debug)]
pub struct Dirs {
    pub gen: PathBuf,
    pub stage: PathBuf,
    pub chunks: PathBuf,
    pub widgets: PathBuf,
}

impl Dirs {
    pub fn of(data: &Path) -> Self {
        Self { gen: data.join("generated"), stage: data.join(".stage"), chunks: data.join("chunks"), widgets: data.join("widgets") }
    }
}

/// The card a failed generation shows (`errorCard`): the note, cut at 600
/// UTF-16 units, as a JSON string in a red card.
pub fn error_card(note: &str) -> String {
    format!(
        r#"import {{ Text, View }} from "nsh";

export default function R() {{
  return (
    <View class="gap-2 rounded-xl border border-red-200 bg-red-50 p-3">
      <Text class="text-sm font-semibold text-red-700">generation failed</Text>
      <Text class="text-xs text-red-600">{{{}}}</Text>
    </View>
  );
}}
"#,
        js::quote(&js::slice_utf16(note, 600))
    )
}

/// The canned module a prompt gets with no model (`mockModule`), by
/// keyword; the default one quotes the prompt.
pub fn mock_module(prompt: &str) -> String {
    let p = prompt.to_lowercase();
    let any = |words: &[&str]| words.iter().any(|w| p.contains(w));
    if any(&["counter", "count", "stepper"]) {
        return COUNTER.to_owned();
    }
    if any(&["chart", "graph", "bar", "week"]) {
        return CHART.to_owned();
    }
    if any(&["form", "book", "sign", "input"]) {
        return FORM.to_owned();
    }
    if any(&["todo", "task", "check", "list"]) {
        return TODO.to_owned();
    }
    if any(&["stock", "nvda", "aapl", "ticker", "price"]) {
        return STOCK.to_owned();
    }
    format!("{DEFAULT_HEAD}{{{}{DEFAULT_TAIL}", js::quote(&format!("You asked for: {prompt}")))
}

const COUNTER: &str = r##"import { signal, Text, View } from "nsh";

export default function R() {
  const n = signal(0);
  return (
    <View class="items-center gap-3 py-2">
      <Text class="text-5xl font-bold text-zinc-900">{n()}</Text>
      <View class="flex-row gap-3">
        <View onPress={() => n.update((v) => v - 1)} activeOpacity={0.6} class="rounded-lg bg-zinc-200 px-5 py-2"><Text class="text-lg">−</Text></View>
        <View onPress={() => n.update((v) => v + 1)} activeOpacity={0.6} class="rounded-lg bg-blue-600 px-5 py-2"><Text class="text-lg font-semibold text-white">+</Text></View>
      </View>
    </View>
  );
}
"##;
const CHART: &str = r##"import { Text, View } from "nsh";

const days = ["M", "T", "W", "T", "F", "S", "S"];
const vals = [42, 68, 35, 90, 74, 20, 55];

export default function R() {
  return (
    <View class="gap-2 py-2">
      <Text class="text-sm font-semibold text-zinc-800">Sessions this week</Text>
      <View class="h-32 flex-row items-end gap-2">
        <Each of={vals}>{(v, i) => (
          <View class="flex-1 items-center gap-1">
            <View class="w-full rounded-t-md bg-blue-500" style={{ height: v() }} />
            <Text class="text-[10px] text-zinc-500">{days[i()]}</Text>
          </View>
        )}
        </Each>
      </View>
    </View>
  );
}
"##;
const FORM: &str = r##"import { Input, signal, Text, View } from "nsh";

export default function R() {
  const name = signal("");
  const when = signal("");
  const sent = signal(false);
  return (
    <View class="gap-2 py-1">
      <Text class="text-sm font-semibold text-zinc-800">Book a table</Text>
      <Input value={name()} onChangeText={(t) => name.set(t)} placeholder="Name" class="rounded-lg border border-zinc-300 px-3 py-2 text-sm" />
      <Input value={when()} onChangeText={(t) => when.set(t)} placeholder="Friday, 8 pm" class="rounded-lg border border-zinc-300 px-3 py-2 text-sm" />
      <View onPress={() => sent.set(true)} activeOpacity={0.7} class="items-center rounded-lg bg-blue-600 py-2">
        <Text class="text-sm font-semibold text-white">Reserve</Text>
      </View>
      <If when={sent()}><Text class="text-xs text-green-600">Reserved for {name() || "—"}, {when() || "—"} ✓</Text></If>
    </View>
  );
}
"##;
const TODO: &str = r##"import { signal, Text, View } from "nsh";

const items0 = ["Ship nsh demo", "Write docs", "Test on iOS"];
const done = signal([false, false, false]);

export default function R() {
  return (
    <View class="gap-1 py-1">
      <Each of={items0}>{(t, i) => (
        <View onPress={() => done.update((d) => d.map((v, j) => (j === i() ? !v : v)))} activeOpacity={0.6} class="flex-row items-center gap-2 py-1">
          <View classList={{ "bg-blue-600 border-blue-600": done()[i()] }} class="h-4 w-4 rounded border border-zinc-400" />
          <Text classList={{ "line-through text-zinc-400": done()[i()] }} class="text-sm text-zinc-800">{t()}</Text>
        </View>
      )}
      </Each>
    </View>
  );
}
"##;
const STOCK: &str = r##"import { Text, View } from "nsh";

export default function R() {
  return (
    <View class="gap-1 py-1">
      <View class="flex-row items-baseline justify-between">
        <Text class="text-base font-bold text-zinc-900">NVDA</Text>
        <Text class="text-base font-semibold text-green-600">▲ 8.6%</Text>
      </View>
      <Text class="text-3xl font-bold text-zinc-900">$186.42</Text>
      <Text class="text-xs text-zinc-500">NVIDIA Corp · close Fri · mock data</Text>
    </View>
  );
}
"##;
const DEFAULT_HEAD: &str = r##"import { signal, Text, View } from "nsh";

export default function R() {
  const liked = signal(false);
  return (
    <View class="gap-2 py-1">
      <Text class="text-sm text-zinc-700">"##;
const DEFAULT_TAIL: &str = r##"}</Text>
      <View onPress={() => liked.update((v) => !v)} activeOpacity={0.6} class="self-start rounded-lg bg-zinc-100 px-3 py-1.5">
        <Text class="text-sm">{liked() ? "♥ liked" : "♡ like"}</Text>
      </View>
    </View>
  );
}
"##;

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    fn sha(s: &str) -> String {
        Sha256::digest(s.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn pipe_10_the_error_card_quotes_the_note_cut_at_600() {
        let card = |note: &str| {
            format!(
                "import {{ Text, View }} from \"nsh\";\n\nexport default function R() {{\n  return (\n    <View class=\"gap-2 rounded-xl border border-red-200 bg-red-50 p-3\">\n      <Text class=\"text-sm font-semibold text-red-700\">generation failed</Text>\n      <Text class=\"text-xs text-red-600\">{{{note}}}</Text>\n    </View>\n  );\n}}\n"
            )
        };
        assert_eq!(error_card("unknown"), card("\"unknown\""));
        assert_eq!(error_card("error TS2304: Cannot find name 'nope'.\n"), card("\"error TS2304: Cannot find name 'nope'.\\n\""));
        assert_eq!(error_card(&format!("{}y", "x".repeat(700))), card(&format!("\"{}\"", "x".repeat(600))));
    }

    /// pipeline.test.mjs MOCK_SHA
    #[test]
    fn pipe_11_mock_modules_by_keyword_hash_as_nodes() {
        let cases = [
            ("a counter please", "32cfaa8d3fa97ce47f96dd138b94271df0aa3e90fd10a831958caa1960889aca"),
            ("weekly bar chart", "fcd7e40baffef91d8048b1cc81105a15c46a31d5e891a2f03f4694c8cf5123ca"),
            ("a booking form", "181ebbb3fb053ddb896af1f3c88d4f172515bfe47365187cff8834c7baa1cad7"),
            ("my todo list", "265528ef17ed5b5d13f02d04225c2c0b3d96aca746a4e07887ee12c9db62df68"),
            ("NVDA stock price", "38c1b59447738dfd11a76c7ae0f26bfce317dae18579df1ceda2a1eaf7cdaf5c"),
            ("a quote: \"hi\" \\ there", "2a7c5fef082d0816ce152a024a7fae6be1f9e8a9cc3b3c25de6663cb0b50717b"),
        ];
        for (prompt, want) in cases {
            assert_eq!(sha(&mock_module(prompt)), want, "{prompt}");
        }
        assert!(mock_module("a quote: \"hi\" \\ there").contains(r#"<Text class="text-sm text-zinc-700">{"You asked for: a quote: \"hi\" \\ there"}</Text>"#));
    }
}
