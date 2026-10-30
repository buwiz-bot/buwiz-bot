// Port of src/svg/ink.ts: which text color reads on a style's fill.

use crate::js;

// Named colors that #fff contrasts with more than #1f1f1f does, and the rest.
const DARK: &str = "black blue blueviolet brown crimson darkblue darkcyan darkgreen darkmagenta darkolivegreen darkorchid darkred darkslateblue darkslategray darkviolet dimgray firebrick forestgreen green indigo maroon mediumblue mediumslateblue mediumvioletred midnightblue navy olive purple rebeccapurple royalblue saddlebrown seagreen sienna slateblue steelblue teal";
const LIGHT: &str = "aliceblue antiquewhite aqua aquamarine azure beige bisque blanchedalmond burlywood cadetblue chartreuse chocolate coral cornflowerblue cornsilk cyan darkgoldenrod darkgray darkkhaki darkorange darksalmon darkseagreen darkturquoise deeppink deepskyblue dodgerblue floralwhite fuchsia gainsboro ghostwhite gold goldenrod gray greenyellow honeydew hotpink indianred ivory khaki lavender lavenderblush lawngreen lemonchiffon lightblue lightcoral lightcyan lightgoldenrodyellow lightgray lightgreen lightpink lightsalmon lightseagreen lightskyblue lightslategray lightsteelblue lightyellow lime limegreen linen magenta mediumaquamarine mediumorchid mediumpurple mediumseagreen mediumspringgreen mediumturquoise mintcream mistyrose moccasin navajowhite oldlace olivedrab orange orangered orchid palegoldenrod palegreen paleturquoise palevioletred papayawhip peachpuff peru pink plum powderblue red rosybrown salmon sandybrown seashell silver skyblue slategray snow springgreen tan thistle tomato turquoise violet wheat white whitesmoke yellow yellowgreen";

pub const ON_LIGHT: &str = "var(--pele-fg-on-light,#1f1f1f)";
pub const ON_DARK: &str = "var(--pele-fg-on-dark,#fff)";

fn linear(c: f64) -> f64 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}
fn luminance(r: f64, g: f64, b: f64) -> f64 {
    0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b)
}
fn clamp(n: f64) -> f64 {
    n.max(0.0).min(1.0)
}

// Luminance where #1f1f1f and #fff have equal contrast.
fn even() -> f64 {
    (1.05 * (luminance(31.0 / 255.0, 31.0 / 255.0, 31.0 / 255.0) + 0.05)).sqrt() - 0.05
}

/// RE_FILL /[";]fill:([^;"]*)/: the first fill a style string sets.
pub fn style_fill(shape: &str) -> Option<&str> {
    let b = shape.as_bytes();
    let mut from = 0;
    while let Some(p) = shape[from..].find("fill:") {
        let at = from + p;
        if at > 0 && (b[at - 1] == b'"' || b[at - 1] == b';') {
            let s = at + 5;
            let e = shape[s..].find([';', '"']).map_or(shape.len(), |q| q + s);
            return Some(&shape[s..e]);
        }
        from = at + 1;
    }
    None
}

/// RE_NUMBER: the unit of a plain CSS number, Some("") for none.
fn number_unit(raw: &str) -> Option<String> {
    let s = raw.strip_prefix(['+', '-']).unwrap_or(raw);
    let b = s.as_bytes();
    let mut i = 0;
    if i < b.len() && b[i].is_ascii_digit() {
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i < b.len() && b[i] == b'.' {
            let st = i + 1;
            let mut j = st;
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            if j > st {
                i = j;
            }
        }
    } else if i < b.len() && b[i] == b'.' {
        let st = i + 1;
        let mut j = st;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        if j == st {
            return None;
        }
        i = j;
    } else {
        return None;
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let mut j = i + 1;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        let st = j;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        if j > st {
            i = j;
        }
    }
    let unit = s[i..].to_ascii_lowercase();
    match unit.as_str() {
        "" | "%" | "deg" | "grad" | "rad" | "turn" => Some(s[i..].to_string()),
        _ => None,
    }
}

fn amount(raw: &str, full: f64) -> f64 {
    match number_unit(raw) {
        None => f64::NAN,
        Some(u) if !u.is_empty() && u != "%" => f64::NAN,
        Some(u) => clamp(js::parse_float(raw) / if u.is_empty() { full } else { 100.0 }),
    }
}

fn hue(raw: &str) -> f64 {
    match number_unit(raw) {
        None => f64::NAN,
        Some(u) if u == "%" => f64::NAN,
        Some(u) => {
            let n = js::parse_float(raw);
            match u.to_ascii_lowercase().as_str() {
                "turn" => n * 360.0,
                "rad" => (n * 180.0) / std::f64::consts::PI,
                "grad" => n * 0.9,
                _ => n,
            }
        }
    }
}

fn split_ws(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut gap = false;
    for c in s.chars() {
        if js::is_space(c) {
            if !gap {
                out.push(std::mem::take(&mut cur));
                gap = true;
            }
        } else {
            cur.push(c);
            gap = false;
        }
    }
    out.push(cur);
    out
}

// The comma syntax takes no `none`, rgb() channels all or none in %, and hsl() saturation and lightness in %.
fn args(body: &str, hsl: bool) -> Option<Vec<String>> {
    let legacy = body.contains(',');
    let parts: Vec<&str> = if legacy { vec![body] } else { body.split('/').collect() };
    if parts.len() > 2 {
        return None;
    }
    let main = parts[0];
    let alpha = parts.get(1);
    let mut list: Vec<String> = if legacy { js::trim(main).split(',').map(|c| js::trim(c).to_string()).collect() } else { split_ws(js::trim(main)) };
    if legacy {
        // /\s*,\s*/ keeps the outer ends as trimmed by trim().
        let n = list.len();
        let raw: Vec<&str> = js::trim(main).split(',').collect();
        for (k, item) in list.iter_mut().enumerate() {
            let r = raw[k];
            *item = if k == 0 { js::trim_end(r).to_string() } else if k == n - 1 { js::trim_start(r).to_string() } else { js::trim(r).to_string() };
            if n == 1 {
                *item = r.to_string();
            }
        }
    }
    if if legacy { list.len() < 3 || list.len() > 4 } else { list.len() != 3 } {
        return None;
    }
    if let Some(a) = alpha {
        list.push(js::trim(a).to_string());
    }
    if !legacy {
        return Some(list.into_iter().map(|c| if c == "none" { "0".to_string() } else { c }).collect());
    }
    let from = if hsl { 1 } else { 0 };
    let percent = list[from..3].iter().filter(|c| c.ends_with('%')).count();
    if percent == if hsl { 2 } else { 3 } || (!hsl && percent == 0) {
        Some(list)
    } else {
        None
    }
}

fn is_hex(value: &str) -> bool {
    let Some(d) = value.strip_prefix('#') else { return false };
    matches!(d.len(), 3 | 4 | 6 | 8) && d.bytes().all(|b| b.is_ascii_hexdigit())
}

// Luminance and alpha; named colors use 0 or 1 to preserve the contrast choice.
fn color(raw: &str) -> Option<(f64, f64)> {
    let value = js::trim(raw).to_lowercase();
    let name = value.replacen("grey", "gray", 1);
    if DARK.split(' ').any(|n| n == name) {
        return Some((0.0, 1.0));
    }
    if LIGHT.split(' ').any(|n| n == name) {
        return Some((1.0, 1.0));
    }
    if is_hex(&value) {
        let digits: String = if value.len() < 6 { value[1..].chars().flat_map(|c| [c, c]).collect() } else { value[1..].to_string() };
        let n = |i: usize| js::parse_int(&digits[2 * i..2 * i + 2], 16) / 255.0;
        return Some((luminance(n(0), n(1), n(2)), if digits.len() == 8 { n(3) } else { 1.0 }));
    }
    // /^(rgba?|hsla?)\(([^()]*)\)$/i
    let open = value.find('(')?;
    let fname = &value[..open];
    if !matches!(fname, "rgb" | "rgba" | "hsl" | "hsla") || !value.ends_with(')') {
        return None;
    }
    let body = &value[open + 1..value.len() - 1];
    if body.contains(['(', ')']) {
        return None;
    }
    let hsl = fname.starts_with("hsl");
    let list = args(body, hsl)?;
    let alpha = if list.len() == 4 { amount(&list[3], 1.0) } else { 1.0 };
    let rgb: Vec<f64> = if !hsl {
        list[..3].iter().map(|c| amount(c, 255.0)).collect()
    } else {
        let h = (((hue(&list[0]) % 360.0) + 360.0) % 360.0) / 30.0;
        let s = amount(&list[1], 100.0);
        let l = amount(&list[2], 100.0);
        let a = s * l.min(1.0 - l);
        [0.0, 8.0, 4.0]
            .iter()
            .map(|k| {
                let t = (k + h) % 12.0;
                l - a * (-1f64).max((t - 3.0).min(9.0 - t).min(1.0))
            })
            .collect()
    };
    let lum = luminance(rgb[0], rgb[1], rgb[2]);
    if (lum + alpha).is_nan() {
        None
    } else {
        Some((lum, alpha))
    }
}

// How opaque a style's fill-opacity and opacity leave its fill, or NaN when either isn't a plain number.
fn opacity(shape: &str) -> f64 {
    let b = shape.as_bytes();
    let mut product = 1.0;
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'"' || b[i] == b';' {
            let rest = &shape[i + 1..];
            let skip = if rest.starts_with("fill-opacity:") {
                Some(13)
            } else if rest.starts_with("opacity:") {
                Some(8)
            } else {
                None
            };
            if let Some(k) = skip {
                let s = i + 1 + k;
                let e = shape[s..].find([';', '"']).map_or(shape.len(), |q| q + s);
                product *= amount(js::trim(&shape[s..e]), 1.0);
                i = e;
                continue;
            }
        }
        i += 1;
    }
    product
}

// Styles come from resolve_style(). Fills below half opacity depend on an unknown background.
pub fn ink(shape: &str, text: &str) -> Option<&'static str> {
    let fill = style_fill(shape)?;
    if style_fill(text).is_some() {
        return None;
    }
    let found = color(fill)?;
    // An opacity that isn't a plain number, such as calc() or var(), may still be low.
    if !(found.1 * opacity(shape) >= 0.5) {
        return None;
    }
    Some(if found.0 > even() { ON_LIGHT } else { ON_DARK })
}

// Unknown fill colors and opacities are treated as opaque.
pub fn paints(shape: &str) -> bool {
    let Some(fill) = style_fill(shape) else { return false };
    let fill = js::trim(fill).to_lowercase();
    if fill.is_empty() || fill == "none" || fill == "transparent" {
        return false;
    }
    let scale = opacity(shape);
    color(&fill).map_or(1.0, |c| c.1) * if scale.is_nan() { 1.0 } else { scale } > 0.0
}

pub fn ink_fill(shape: &str, text: &str, fallback: Option<&str>) -> String {
    match ink(shape, text).or(fallback) {
        Some(color) => format!(" fill=\"{}\"", color),
        None => String::new(),
    }
}
