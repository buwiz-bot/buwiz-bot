// JavaScript semantics the port depends on to stay byte-identical with upstream: how numbers
// print, what `trim()` and `\s` take for whitespace, `Math.round`, and how a sort comparator's
// result is read. Not an upstream file.

use std::cmp::Ordering;

/// `String(n)` / template-literal formatting of a number.
pub fn num_str(v: f64) -> String {
    if v.is_nan() {
        return "NaN".into();
    }
    if v.is_infinite() {
        return if v > 0.0 { "Infinity".into() } else { "-Infinity".into() };
    }
    if v == 0.0 {
        return "0".into();
    }
    if v.fract() == 0.0 && v.abs() < 1e21 {
        return format!("{}", v as i128);
    }
    // Shortest round-trip digits, then JS's choice of plain or exponent notation.
    let sci = format!("{:e}", v);
    let (mant, exp) = sci.split_once('e').unwrap();
    let exp: i32 = exp.parse().unwrap();
    let neg = mant.starts_with('-');
    let digits: String = mant.chars().filter(|c| c.is_ascii_digit()).collect();
    let k = digits.len() as i32;
    let n = exp + 1;
    let mut out = String::new();
    if neg {
        out.push('-');
    }
    if k <= n && n <= 21 {
        out.push_str(&digits);
        for _ in 0..(n - k) {
            out.push('0');
        }
    } else if 0 < n && n <= 21 {
        out.push_str(&digits[..n as usize]);
        out.push('.');
        out.push_str(&digits[n as usize..]);
    } else if -6 < n && n <= 0 {
        out.push_str("0.");
        for _ in 0..(-n) {
            out.push('0');
        }
        out.push_str(&digits);
    } else {
        out.push_str(&digits[..1]);
        if k > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        out.push('e');
        out.push(if n - 1 >= 0 { '+' } else { '-' });
        out.push_str(&(n - 1).abs().to_string());
    }
    out
}

/// `Math.round`: halves go up, towards +Infinity.
pub fn round(x: f64) -> f64 {
    if !x.is_finite() {
        return x;
    }
    let f = x.floor();
    if x - f >= 0.5 {
        f + 1.0
    } else {
        f
    }
}

/// Whitespace as JS `\s` and `trim()` see it, for one UTF-16 code unit.
pub fn is_space_u(c: u32) -> bool {
    if c < 128 {
        return c == 32 || (9..=13).contains(&c);
    }
    c == 0xa0
        || c == 0x1680
        || (0x2000..=0x200a).contains(&c)
        || c == 0x2028
        || c == 0x2029
        || c == 0x202f
        || c == 0x205f
        || c == 0x3000
        || c == 0xfeff
}

pub fn is_space(c: char) -> bool {
    is_space_u(c as u32)
}

pub fn trim(s: &str) -> &str {
    s.trim_matches(is_space)
}

pub fn trim_start(s: &str) -> &str {
    s.trim_start_matches(is_space)
}

pub fn trim_end(s: &str) -> &str {
    s.trim_end_matches(is_space)
}

/// `\w` for a char.
pub fn is_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Reads a comparator's number as JS's sort does: NaN counts as equal.
pub fn ord(d: f64) -> Ordering {
    if d < 0.0 {
        Ordering::Less
    } else if d > 0.0 {
        Ordering::Greater
    } else {
        Ordering::Equal
    }
}

/// `a || b` for numbers: `b` when `a` is 0 or NaN.
pub fn or(a: f64, b: impl FnOnce() -> f64) -> f64 {
    if a != 0.0 && !a.is_nan() {
        a
    } else {
        b()
    }
}

/// UTF-16 length, as `.length` counts.
pub fn len16(s: &str) -> usize {
    s.chars().map(|c| c.len_utf16()).sum()
}

pub fn u16s(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

pub fn from16(s: &[u16]) -> String {
    String::from_utf16_lossy(s)
}

/// `Number(s)` for a string: decimal, hex/octal/binary literals, Infinity, or NaN.
pub fn to_number(s: &str) -> f64 {
    let t = trim(s);
    if t.is_empty() {
        return 0.0;
    }
    let (neg, body) = match t.as_bytes()[0] {
        b'-' => (true, &t[1..]),
        b'+' => (false, &t[1..]),
        _ => (false, t),
    };
    if body == "Infinity" {
        return if neg { f64::NEG_INFINITY } else { f64::INFINITY };
    }
    if body.len() > 2 && body.as_bytes()[0] == b'0' && t.as_bytes()[0] != b'-' && t.as_bytes()[0] != b'+' {
        let radix = match body.as_bytes()[1] {
            b'x' | b'X' => 16,
            b'o' | b'O' => 8,
            b'b' | b'B' => 2,
            _ => 0,
        };
        if radix != 0 {
            let mut v = 0f64;
            for c in body[2..].chars() {
                match c.to_digit(radix) {
                    Some(d) => v = v * radix as f64 + d as f64,
                    None => return f64::NAN,
                }
            }
            return v;
        }
    }
    let b = body.as_bytes();
    let mut i = 0;
    let mut digits = 0;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
        digits += 1;
    }
    if i < b.len() && b[i] == b'.' {
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
            digits += 1;
        }
    }
    if digits == 0 {
        return f64::NAN;
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let mut j = i + 1;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        let start = j;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        if j == start {
            return f64::NAN;
        }
        i = j;
    }
    if i != b.len() {
        return f64::NAN;
    }
    let v: f64 = body.parse().unwrap_or(f64::NAN);
    if neg {
        -v
    } else {
        v
    }
}

/// `parseFloat`: the longest decimal prefix, or NaN.
pub fn parse_float(s: &str) -> f64 {
    let t = trim_start(s);
    let b = t.as_bytes();
    let mut i = 0;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        i += 1;
    }
    if t[i..].starts_with("Infinity") {
        return if b.first() == Some(&b'-') { f64::NEG_INFINITY } else { f64::INFINITY };
    }
    let mut digits = 0;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
        digits += 1;
    }
    if i < b.len() && b[i] == b'.' {
        let mut j = i + 1;
        let mut frac = 0;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
            frac += 1;
        }
        if digits + frac > 0 {
            i = j;
            digits += frac;
        }
    }
    if digits == 0 {
        return f64::NAN;
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let mut j = i + 1;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        let start = j;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        if j > start {
            i = j;
        }
    }
    let text = t[..i].trim_end_matches('.');
    let text = if text.is_empty() || text == "+" || text == "-" { &t[..i] } else { text };
    text.parse().unwrap_or(f64::NAN)
}

/// `parseInt(s, radix)` for the radixes the port uses.
pub fn parse_int(s: &str, radix: u32) -> f64 {
    let t = trim_start(s);
    let mut chars = t.chars().peekable();
    let mut neg = false;
    if let Some(&c) = chars.peek() {
        if c == '+' || c == '-' {
            neg = c == '-';
            chars.next();
        }
    }
    let mut v = 0f64;
    let mut any = false;
    for c in chars {
        match c.to_digit(radix) {
            Some(d) => {
                v = v * radix as f64 + d as f64;
                any = true;
            }
            None => break,
        }
    }
    if !any {
        return f64::NAN;
    }
    if neg {
        -v
    } else {
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_print_as_in_js() {
        assert_eq!(num_str(1.5), "1.5");
        assert_eq!(num_str(-0.25), "-0.25");
        assert_eq!(num_str(1e21), "1e+21");
        assert_eq!(num_str(1.5e-7), "1.5e-7");
        assert_eq!(num_str(0.000001), "0.000001");
        assert_eq!(num_str(123456789012.34), "123456789012.34");
        assert_eq!(num_str(0.1 + 0.2), "0.30000000000000004");
        assert_eq!(round(-2.5), -2.0);
        assert_eq!(round(2.5), 3.0);
    }
}
