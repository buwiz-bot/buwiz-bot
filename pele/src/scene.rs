// The drawing as a tree of SVG elements, read back from the SVG that the diagram renderers write
// (which port upstream's string building line for line, so that the output stays byte for
// byte). Hosts that draw natively walk this tree; `svg::write` turns it back into the same
// bytes. Paints stay symbolic: an attribute names a `var(--pele-*)` token with its fallback, a
// private alias set on the root (`--_fg`), or a literal color. Not an upstream file.

use crate::js;

#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    Element(Element),
    /// Text as written, entities still escaped.
    Text(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Element {
    pub name: String,
    /// Attributes in document order, values as written (escaped).
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Node>,
    /// Written as `<name/>` rather than `<name></name>`.
    pub empty: bool,
}

impl Element {
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Scene {
    pub root: Element,
}

/// A paint as an attribute or style gives it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Paint {
    None,
    /// `var(--name, fallback)`: `name` without the leading `--`.
    Var { name: String, fallback: Option<String> },
    Literal(String),
}

impl Paint {
    pub fn parse(value: &str) -> Paint {
        let v = js::trim(value);
        if v == "none" || v.is_empty() {
            return Paint::None;
        }
        if let Some(inner) = v.strip_prefix("var(").and_then(|r| r.strip_suffix(')')) {
            let (name, fallback) = match split_top(inner) {
                Some((n, f)) => (n, Some(js::trim(f).to_string())),
                None => (inner, None),
            };
            let name = js::trim(name);
            if let Some(name) = name.strip_prefix("--") {
                return Paint::Var { name: name.to_string(), fallback };
            }
        }
        Paint::Literal(v.to_string())
    }
}

/// Splits `a,b` at its first comma outside parentheses.
fn split_top(s: &str) -> Option<(&str, &str)> {
    let mut depth = 0;
    for (i, c) in s.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => return Some((&s[..i], &s[i + 1..])),
            _ => {}
        }
    }
    None
}

/// Undoes `esc`: the four entities it writes.
pub fn unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&amp;", "&")
}

/// The declarations of a `style` attribute, in order: `--_fg:var(--pele-fg,#1f1f1f)` gives
/// ("--_fg", "var(--pele-fg,#1f1f1f)").
pub fn declarations(style: &str) -> Vec<(String, String)> {
    let style = unescape(style);
    let mut out = Vec::new();
    let mut depth = 0;
    let mut start = 0;
    let mut parts = Vec::new();
    for (i, c) in style.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            ';' if depth == 0 => {
                parts.push(style[start..i].to_string());
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(style[start..].to_string());
    for p in parts {
        if let Some((k, v)) = p.split_once(':') {
            let k = js::trim(k);
            if !k.is_empty() {
                out.push((k.to_string(), js::trim(v).to_string()));
            }
        }
    }
    out
}

impl Scene {
    /// Reads SVG as the renderers write it: double-quoted attributes, no comments, no CDATA.
    pub fn parse(svg: &str) -> Option<Scene> {
        let mut stack: Vec<Element> = vec![Element { name: String::new(), attrs: Vec::new(), children: Vec::new(), empty: false }];
        let b = svg.as_bytes();
        let mut i = 0;
        while i < b.len() {
            if b[i] != b'<' {
                let end = svg[i..].find('<').map_or(svg.len(), |p| p + i);
                stack.last_mut()?.children.push(Node::Text(svg[i..end].to_string()));
                i = end;
                continue;
            }
            if b.get(i + 1) == Some(&b'/') {
                let end = svg[i..].find('>')? + i;
                let name = &svg[i + 2..end];
                let el = stack.pop()?;
                if el.name != name {
                    return None;
                }
                stack.last_mut()?.children.push(Node::Element(el));
                i = end + 1;
                continue;
            }
            let mut p = i + 1;
            while p < b.len() && !matches!(b[p], b' ' | b'>' | b'/') {
                p += 1;
            }
            let name = svg[i + 1..p].to_string();
            let mut attrs = Vec::new();
            loop {
                while p < b.len() && b[p] == b' ' {
                    p += 1;
                }
                match b.get(p)? {
                    b'/' => {
                        if b.get(p + 1) != Some(&b'>') {
                            return None;
                        }
                        stack.last_mut()?.children.push(Node::Element(Element { name, attrs, children: Vec::new(), empty: true }));
                        i = p + 2;
                        break;
                    }
                    b'>' => {
                        stack.push(Element { name, attrs, children: Vec::new(), empty: false });
                        i = p + 1;
                        break;
                    }
                    _ => {
                        let eq = svg[p..].find("=\"")? + p;
                        let key = svg[p..eq].to_string();
                        let close = svg[eq + 2..].find('"')? + eq + 2;
                        attrs.push((key, svg[eq + 2..close].to_string()));
                        p = close + 1;
                    }
                }
            }
        }
        let mut top = stack.pop()?;
        if !stack.is_empty() || top.children.len() != 1 {
            return None;
        }
        match top.children.pop()? {
            Node::Element(root) => Some(Scene { root }),
            Node::Text(_) => None,
        }
    }

    /// The private aliases the root's style sets (`_fg` → `var(--pele-fg,#1f1f1f)`), and its
    /// other declarations (`font`, `font-family`, …).
    pub fn root_style(&self) -> Vec<(String, String)> {
        self.root.attr("style").map(declarations).unwrap_or_default()
    }

    pub fn width(&self) -> f64 {
        self.root.attr("width").map_or(0.0, js::to_number)
    }

    pub fn height(&self) -> f64 {
        self.root.attr("height").map_or(0.0, js::to_number)
    }
}
