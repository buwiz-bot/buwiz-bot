//! `fixImports`: drop imports nsh can't resolve (react and friends) and add
//! the nsh names a module uses without importing them, from where nsh 0.3
//! serves them (`nsh` and `nsh/base`; it removed `nsh/canvas`, `nsh/image`,
//! `nsh/list`, `nsh/ui/mobile` and the other alias modules). A name counts
//! as used where it is an identifier in the code, not a word in a string or
//! a comment. The import patterns are Node's; `\s` reads ASCII here (its
//! `\s` also takes Unicode spaces, which no generated import uses).

use std::sync::OnceLock;

use regex_lite::{Captures, Regex};

use crate::js;

/// Names the model may use without importing them, from "nsh".
const CORE_NAMES: [&str; 18] = [
    "batch", "Canvas", "cleanup", "computed", "context", "effect", "Image", "Keyboard", "List", "mount", "Scroll", "signal", "split", "Text", "untracked", "Video", "View", "WebView",
];

/// Names from nsh's other modules. (`Card`, `TextField`, `StatusBar`,
/// `SafeAreaView` and `KeyboardAvoidingView` have no home in nsh 0.3: an
/// import of them would fail the check anyway, so none is added.)
const MODULE_NAMES: [(&str, &str); 2] = [("Button", "nsh/base"), ("Input", "nsh/base")];

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
    let used = identifiers(&code);
    let missing = |n: &str| used.contains(n) && !imported.contains(n);
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

/// The identifiers of `code`, outside comments and string literals. A
/// template literal's text is skipped and its `${…}` read as code. A quote
/// that doesn't close on its own line is no string (an apostrophe in JSX
/// text: `<Text>Don't</Text>`), and a name after a lone `.` is a property,
/// not a use. Regex literals are not told apart from division.
fn identifiers(code: &str) -> std::collections::HashSet<&str> {
    let b = code.as_bytes();
    let ident = |c: u8| c.is_ascii_alphanumeric() || c == b'_' || c == b'$' || c >= 0x80;
    let mut out = std::collections::HashSet::new();
    // the brace depth of each open `${` (a `}` at it resumes the template)
    let mut templates: Vec<usize> = Vec::new();
    let mut depth = 0usize;
    let mut i = 0;
    // in a template's text, from i: to its end, or into a `${`
    let template_text = |mut i: usize, templates: &mut Vec<usize>, depth: usize| -> usize {
        while i < b.len() {
            match b[i] {
                b'\\' => i += 2,
                b'`' => return i + 1,
                b'$' if b.get(i + 1) == Some(&b'{') => {
                    templates.push(depth);
                    return i + 2;
                }
                _ => i += 1,
            }
        }
        i
    };
    while i < b.len() {
        let c = b[i];
        match c {
            b'/' if b.get(i + 1) == Some(&b'/') => i = code[i..].find('\n').map_or(b.len(), |n| i + n),
            b'/' if b.get(i + 1) == Some(&b'*') => i = code[i + 2..].find("*/").map_or(b.len(), |n| i + 2 + n + 2),
            b'\'' | b'"' => {
                let mut j = i + 1;
                while j < b.len() && b[j] != c && b[j] != b'\n' {
                    j += if b[j] == b'\\' { 2 } else { 1 };
                }
                i = if j < b.len() && b[j] == c { j + 1 } else { i + 1 };
            }
            b'`' => i = template_text(i + 1, &mut templates, depth),
            b'{' => {
                depth += 1;
                i += 1;
            }
            b'}' => {
                if templates.last() == Some(&depth) {
                    templates.pop();
                    i = template_text(i + 1, &mut templates, depth);
                } else {
                    depth = depth.saturating_sub(1);
                    i += 1;
                }
            }
            _ if ident(c) && !c.is_ascii_digit() => {
                let start = i;
                while i < b.len() && ident(b[i]) {
                    i += 1;
                }
                let member = start > 0 && b[start - 1] == b'.' && !(start > 2 && &b[start - 3..start] == b"...");
                if !member {
                    out.insert(&code[start..i]);
                }
            }
            _ if ident(c) => {
                while i < b.len() && ident(b[i]) {
                    i += 1;
                }
            }
            _ => i += 1,
        }
    }
    out
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
            // nsh 0.3's homes: Image and List from "nsh", Button from
            // nsh/base, Card nowhere
            (
                "import { View } from \"nsh\";\nexport default () => <View><Button /><Card /><List /><Image src=\"a\" /></View>;\n",
                "import { Button } from \"nsh/base\";\nimport { View, Image, List } from \"nsh\";\nexport default () => <View><Button /><Card /><List /><Image src=\"a\" /></View>;\n",
            ),
            (
                "export default () => <Canvas draw={() => {}} />;\nconst f = <Input value=\"\" />;\n",
                "import { Input } from \"nsh/base\";\nimport { Canvas } from \"nsh\";\nexport default () => <Canvas draw={() => {}} />;\nconst f = <Input value=\"\" />;\n",
            ),
            ("export default () => \"TextView mounted\";\n", "export default () => \"TextView mounted\";\n"),
            // a word in a string or a comment is no use of the name
            ("export default () => \"a View here\";\n", "export default () => \"a View here\";\n"),
            ("// a List of Image cards, an Input\n/* View */ export default () => 'Button';\n", "// a List of Image cards, an Input\n/* View */ export default () => 'Button';\n"),
            ("const s = `an Image of ${n} Canvas`;\n", "const s = `an Image of ${n} Canvas`;\n"),
            // a template's ${…} is code; a property is no use
            ("const s = `x ${signal(1)} ${{ a: 1 }.a}`; o.View;\n", "import { signal } from \"nsh\";\nconst s = `x ${signal(1)} ${{ a: 1 }.a}`; o.View;\n"),
            // an apostrophe in JSX text opens no string
            ("export default () => [<b>Don't</b>, <Text>x</Text>];\n", "import { Text } from \"nsh\";\nexport default () => [<b>Don't</b>, <Text>x</Text>];\n"),
        ];
        for (input, want) in cases {
            assert_eq!(fix_imports(input), want, "{input}");
        }
    }
}
