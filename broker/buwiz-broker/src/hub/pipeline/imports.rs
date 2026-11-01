//! `fixImports`: drop imports nsh can't resolve (react and friends) and add
//! the nsh names a module uses without importing them. The patterns are
//! Node's; `\s` and `\b` read ASCII here, as JavaScript's `\b` does (its
//! `\s` also takes Unicode spaces, which no generated import uses).

use std::sync::OnceLock;

use regex_lite::{Captures, Regex};

use crate::js;

/// Names the model may use without importing them, from "nsh".
const CORE_NAMES: [&str; 13] = ["batch", "cleanup", "computed", "context", "effect", "Input", "mount", "Scroll", "signal", "split", "Text", "untracked", "View"];

/// Names from nsh's other modules, in Node's key order.
const MODULE_NAMES: [(&str, &str); 12] = [
    ("Canvas", "nsh/canvas"),
    ("Image", "nsh/image"),
    ("List", "nsh/list"),
    ("Video", "nsh/video"),
    ("WebView", "nsh/webview"),
    ("Button", "nsh/ui/mobile"),
    ("Card", "nsh/ui/mobile"),
    ("Keyboard", "nsh/ui/mobile"),
    ("KeyboardAvoidingView", "nsh/ui/mobile"),
    ("SafeAreaView", "nsh/ui/mobile"),
    ("StatusBar", "nsh/ui/mobile"),
    ("TextField", "nsh/ui/mobile"),
];

fn re(slot: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    slot.get_or_init(|| Regex::new(pattern).expect("a valid pattern"))
}

fn trim(s: &str) -> &str {
    s.trim_matches(js::is_space)
}

pub fn fix_imports(code: &str) -> String {
    static FOREIGN: OnceLock<Regex> = OnceLock::new();
    static NAMED: OnceLock<Regex> = OnceLock::new();
    static AS: OnceLock<Regex> = OnceLock::new();
    static NSH: OnceLock<Regex> = OnceLock::new();
    let foreign = re(&FOREIGN, r#"(?m)^[ \t]*import[^\n]*from\s*["'](react|react-native|solid-js|preact|vue|svelte)[^\n]*\n"#);
    let mut code = foreign.replace_all(code, "").into_owned();

    let named = re(&NAMED, r#"import\s*\{([^}]*)\}\s*from\s*["'][^"']+["']"#);
    let alias = re(&AS, r"\s+as\s+");
    let mut imported = std::collections::HashSet::new();
    for m in named.captures_iter(&code) {
        for s in m[1].split(',') {
            let first = alias.split(trim(s)).next().unwrap_or("");
            imported.insert(trim(first).to_owned());
        }
    }
    let missing = |n: &str| Regex::new(&format!(r"\b{n}\b")).expect("a name").is_match(&code) && !imported.contains(n);
    let core: Vec<&str> = CORE_NAMES.into_iter().filter(|n| missing(n)).collect();
    let mut by_mod: Vec<(&str, Vec<&str>)> = Vec::new();
    for (n, module) in MODULE_NAMES {
        if missing(n) {
            match by_mod.iter_mut().find(|(m, _)| *m == module) {
                Some((_, names)) => names.push(n),
                None => by_mod.push((module, vec![n])),
            }
        }
    }

    let from_nsh = re(&NSH, r#"import\s*\{([^}]*)\}\s*from\s*["']nsh["']"#);
    if !core.is_empty() && from_nsh.is_match(&code) {
        code = from_nsh
            .replacen(&code, 1, |c: &Captures| {
                let names: Vec<&str> = c[1].split(',').map(trim).chain(core.iter().copied()).collect();
                format!("import {{ {} }} from \"nsh\"", names.join(", "))
            })
            .into_owned();
    } else if !core.is_empty() {
        code = format!("import {{ {} }} from \"nsh\";\n{code}", core.join(", "));
    }

    let extra: Vec<String> = by_mod.iter().map(|(module, names)| format!("import {{ {} }} from \"{module}\";", names.join(", "))).collect();
    if !extra.is_empty() {
        code = format!("{}\n{code}", extra.join("\n"));
    }
    code
}

#[cfg(test)]
mod tests {
    use super::*;

    const CODE: &str = "import { Text } from \"nsh\";\n\nexport default function R() {\n  return <Text>hi</Text>;\n}\n";

    /// pipeline.test.mjs PIPE-02
    #[test]
    fn pipe_02_fix_imports() {
        let cases = [
            (CODE, CODE),
            (
                "import React from \"react\";\nimport { useState } from 'react-native';\nexport default function R() {\n  const n = signal(0);\n  return <View><Text>{n()}</Text></View>;\n}\n",
                "import { signal, Text, View } from \"nsh\";\nexport default function R() {\n  const n = signal(0);\n  return <View><Text>{n()}</Text></View>;\n}\n",
            ),
            (
                "import { View as V } from \"nsh\";\nconst a = signal(1); effect(() => a());\nexport default () => <V><Text>x</Text></V>;\n",
                "import { View as V, effect, signal, Text } from \"nsh\";\nconst a = signal(1); effect(() => a());\nexport default () => <V><Text>x</Text></V>;\n",
            ),
            (
                "import { View } from \"nsh\";\nexport default () => <View><Button /><Card /><List /><Image src=\"a\" /></View>;\n",
                "import { Image } from \"nsh/image\";\nimport { List } from \"nsh/list\";\nimport { Button, Card } from \"nsh/ui/mobile\";\nimport { View } from \"nsh\";\nexport default () => <View><Button /><Card /><List /><Image src=\"a\" /></View>;\n",
            ),
            ("export default () => \"TextView mounted\";\n", "export default () => \"TextView mounted\";\n"),
            ("export default () => \"a View here\";\n", "import { View } from \"nsh\";\nexport default () => \"a View here\";\n"),
        ];
        for (input, want) in cases {
            assert_eq!(fix_imports(input), want, "{input}");
        }
    }
}
