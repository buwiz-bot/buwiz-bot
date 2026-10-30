//! files.mjs `highlight(code, ext)`: a file's lines as tokens
//! `[text, dark colour, light colour]`, in Grok's cursor-dark and
//! cursor-light palettes.
//!
//! Node runs Shiki (TextMate grammars on Oniguruma); this runs syntect with
//! two-face's grammars (Sublime syntaxes on fancy-regex) and the same two
//! themes, converted to tmTheme by `server/tools/tmtheme.mjs`. The shape is
//! Shiki's: lines split at `\r\n` and `\n`, an empty line as `[]`, colours
//! as upper-case `#RRGGBB[AA]`, adjacent tokens joined when both colours
//! and vscode-textmate's token type (comment, string, regex, other) are
//! the same; an extension with no grammar gives `[[line, "", ""]]` per `\n`
//! line. The grammars differ, so a token's colour or where tokens split can
//! differ from Shiki's; the text never does.

use std::collections::HashMap;
use std::sync::OnceLock;

use serde_json::{json, Value};
use syntect::highlighting::{Color, Highlighter, Theme, ThemeSet};
use syntect::parsing::{ParseState, Scope, ScopeStack, SyntaxReference, SyntaxSet};

/// files.mjs `LANG`: the extensions with a grammar, each as the extension
/// syntect finds that grammar by.
fn grammar_ext(ext: &str) -> Option<&'static str> {
    Some(match ext {
        "rs" => "rs",
        "ts" => "ts",
        "tsx" => "tsx",
        "js" | "mjs" | "cjs" => "js",
        // two-face has no JSX grammar of its own: TypeScriptReact reads JSX
        "jsx" => "tsx",
        "py" => "py",
        "go" => "go",
        "swift" => "swift",
        "json" | "jsonc" => "json",
        "toml" => "toml",
        "yaml" | "yml" => "yaml",
        "sh" | "bash" | "zsh" => "sh",
        "css" => "css",
        "scss" => "scss",
        "html" | "htm" => "html",
        "xml" => "xml",
        "sql" => "sql",
        "c" | "h" => "c",
        "cc" | "cpp" | "hpp" => "cpp",
        "java" => "java",
        "kt" => "kt",
        "rb" => "rb",
        "php" => "php",
        "cs" => "cs",
        "md" => "md",
        "ini" => "ini",
        "tex" => "tex",
        _ => return None,
    })
}

struct Engine {
    syntaxes: SyntaxSet,
    dark: Theme,
    light: Theme,
}

fn theme(text: &str) -> Theme {
    ThemeSet::load_from_reader(&mut std::io::Cursor::new(text.as_bytes())).expect("a bundled tmTheme")
}

fn engine() -> &'static Engine {
    static E: OnceLock<Engine> = OnceLock::new();
    E.get_or_init(|| Engine { syntaxes: two_face::syntax::extra_newlines(), dark: theme(include_str!("cursor-dark.tmTheme")), light: theme(include_str!("cursor-light.tmTheme")) })
}

/// The grammar for a file extension `highlight` colours, if any.
pub fn syntax_for(ext: &str) -> Option<&'static SyntaxReference> {
    let e = engine();
    e.syntaxes.find_syntax_by_extension(grammar_ext(ext)?)
}

/// A colour as Shiki prints it.
fn hex(c: Color) -> String {
    let rgb = format!("#{:02X}{:02X}{:02X}", c.r, c.g, c.b);
    if c.a == 0xFF {
        rgb
    } else {
        format!("{rgb}{:02X}", c.a)
    }
}

/// Shiki's lines: split at `\r\n` and `\n` (a lone `\r` stays in its line).
fn lines(code: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = code;
    while let Some(i) = rest.find('\n') {
        let line = &rest[..i];
        out.push(line.strip_suffix('\r').unwrap_or(line));
        rest = &rest[i + 1..];
    }
    out.push(rest);
    out
}

/// vscode-textmate's standard token type (`comment`, `string`, `regex`,
/// `meta.embedded` → other): a scope's own, else its parent's. Shiki keeps
/// neighbours of the same colours apart when this differs.
fn token_type(stack: &[Scope], cache: &mut HashMap<Scope, u8>) -> u8 {
    static RE: OnceLock<regex_lite::Regex> = OnceLock::new();
    let re = RE.get_or_init(|| regex_lite::Regex::new(r"\b(comment|string|regex|meta\.embedded)\b").expect("a valid regex"));
    for scope in stack.iter().rev() {
        let t = *cache.entry(*scope).or_insert_with(|| match re.captures(&scope.build_string()).map(|c| c.get(1).map_or("", |m| m.as_str()).to_owned()).as_deref() {
            Some("comment") => 1,
            Some("string") => 2,
            Some("regex") => 3,
            Some(_) => 4,
            None => 0,
        });
        if t != 0 {
            return if t == 4 { 0 } else { t };
        }
    }
    0
}

/// `highlight(code, ext)`.
pub fn highlight(code: &str, ext: &str) -> Value {
    let Some(syntax) = syntax_for(ext) else {
        return Value::Array(code.split('\n').map(|l| json!([[l, "", ""]])).collect());
    };
    let e = engine();
    let (hd, hl) = (Highlighter::new(&e.dark), Highlighter::new(&e.light));
    let mut parse = ParseState::new(syntax);
    let mut stack = ScopeStack::new();
    let mut types = HashMap::new();
    let mut styles: HashMap<Vec<Scope>, (String, String)> = HashMap::new();
    let mut failed = false;
    let mut out = Vec::new();
    for line in lines(code) {
        // the grammars match with the newline (two-face's "newlines" set)
        let text = format!("{line}\n");
        let ops = if failed { None } else { parse.parse_line(&text, &e.syntaxes).ok() };
        failed = ops.is_none();
        // a grammar the regex engine can't run: the rest in the stack's colours
        let ops = ops.unwrap_or_default();
        let mut tokens: Vec<(usize, usize, String, String, u8)> = Vec::new();
        let mut pos = 0;
        let ends = ops.iter().map(|(at, _)| *at).chain([line.len()]);
        let mut ops = ops.iter();
        for end in ends {
            let end = end.min(line.len());
            if end > pos {
                let (dark, light) =
                    styles.entry(stack.as_slice().to_vec()).or_insert_with(|| (hex(hd.style_for_stack(stack.as_slice()).foreground), hex(hl.style_for_stack(stack.as_slice()).foreground))).clone();
                let t = token_type(stack.as_slice(), &mut types);
                match tokens.last_mut() {
                    Some(last) if last.2 == dark && last.3 == light && last.4 == t => last.1 = end,
                    _ => tokens.push((pos, end, dark, light, t)),
                }
                pos = end;
            }
            if let Some((_, op)) = ops.next() {
                let _ = stack.apply(op);
            }
        }
        out.push(Value::Array(tokens.into_iter().map(|(a, b, d, l, _)| json!([&line[a..b], d, l])).collect()));
    }
    Value::Array(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn joined(v: &Value) -> Vec<String> {
        v.as_array().unwrap().iter().map(|l| l.as_array().unwrap().iter().map(|t| t[0].as_str().unwrap()).collect()).collect()
    }

    #[test]
    fn highlight_keeps_every_line_and_its_text() {
        let code = "fn main() {\r\n    // hi\n    let x = \"s\";\r}\n\n";
        let v = highlight(code, "rs");
        assert_eq!(joined(&v), ["fn main() {", "    // hi", "    let x = \"s\";\r}", "", ""]);
        assert_eq!(v[3], json!([]));
        assert_eq!(highlight("", "rs"), json!([[]]));
        assert_eq!(highlight("a\r\nb\n", "txt"), json!([[["a\r", "", ""]], [["b", "", ""]], [["", "", ""]]]));
    }

    #[test]
    fn highlight_colours_keywords_strings_and_comments_as_shiki_does() {
        let v = highlight("fn main() {\n    // hi\n    let x = \"s\";\n}", "rs");
        let find = |line: usize, text: &str| v[line].as_array().unwrap().iter().find(|t| t[0] == text).cloned().unwrap_or_else(|| panic!("{text} in {}", v[line]));
        assert_eq!(find(0, "fn"), json!(["fn", "#82D2CE", "#B3003F"]));
        assert_eq!(find(1, "// hi"), json!(["// hi", "#E4E4E45E", "#141414AD"]));
        assert_eq!(find(2, "\"s\""), json!(["\"s\"", "#E394DC", "#9E94D5"]));
        // the same colours side by side are one token, unless one is in a
        // string and the other not (Shiki's `${`, `x`, `}` in a template)
        assert_eq!(v[0][1], json!([" ", "#D6D6DD", "#141414EB"]));
        let t = highlight("const a = `t${x}`;", "ts");
        let texts: Vec<&str> = t[0].as_array().unwrap().iter().map(|t| t[0].as_str().unwrap()).collect();
        assert!(texts.contains(&"${") && texts.contains(&"}"), "{t}");
    }

    #[test]
    fn highlight_has_a_grammar_for_every_extension_node_colours() {
        let exts = [
            "rs", "ts", "tsx", "js", "jsx", "mjs", "cjs", "py", "go", "swift", "json", "jsonc", "toml", "yaml", "yml", "sh", "bash", "zsh", "css", "scss", "html", "htm", "xml", "sql", "c", "h", "cc",
            "cpp", "hpp", "java", "kt", "rb", "php", "cs", "md", "ini", "tex",
        ];
        let missing: Vec<&str> = exts.into_iter().filter(|ext| syntax_for(ext).is_none_or(|s| s.name == "Plain Text")).collect();
        assert!(missing.is_empty(), "no grammar for {missing:?}");
        assert!(syntax_for("txt").is_none());
    }
}
