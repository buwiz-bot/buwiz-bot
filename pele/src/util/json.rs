// JSON.parse, for `%%{init: …}%%` directives (upstream calls the built-in). Not an upstream file.

use super::yaml::{Map, Value};

struct J<'a> {
    s: &'a [u8],
    i: usize,
}

impl J<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && matches!(self.s[self.i], b' ' | b'\t' | b'\n' | b'\r') {
            self.i += 1;
        }
    }

    fn value(&mut self, depth: usize) -> Option<Value> {
        if depth > 512 {
            return None;
        }
        self.ws();
        match *self.s.get(self.i)? {
            b'{' => {
                self.i += 1;
                let mut map = Map::default();
                self.ws();
                if self.s.get(self.i) == Some(&b'}') {
                    self.i += 1;
                    return Some(Value::Map(map));
                }
                loop {
                    self.ws();
                    let key = self.string()?;
                    self.ws();
                    if self.s.get(self.i) != Some(&b':') {
                        return None;
                    }
                    self.i += 1;
                    let v = self.value(depth + 1)?;
                    map.set(&key, v);
                    self.ws();
                    match self.s.get(self.i)? {
                        b',' => self.i += 1,
                        b'}' => {
                            self.i += 1;
                            return Some(Value::Map(map));
                        }
                        _ => return None,
                    }
                }
            }
            b'[' => {
                self.i += 1;
                let mut out = Vec::new();
                self.ws();
                if self.s.get(self.i) == Some(&b']') {
                    self.i += 1;
                    return Some(Value::Seq(out));
                }
                loop {
                    out.push(self.value(depth + 1)?);
                    self.ws();
                    match self.s.get(self.i)? {
                        b',' => self.i += 1,
                        b']' => {
                            self.i += 1;
                            return Some(Value::Seq(out));
                        }
                        _ => return None,
                    }
                }
            }
            b'"' => self.string().map(Value::Str),
            b't' => self.lit("true", Value::Bool(true)),
            b'f' => self.lit("false", Value::Bool(false)),
            b'n' => self.lit("null", Value::Null),
            _ => self.number(),
        }
    }

    fn lit(&mut self, word: &str, v: Value) -> Option<Value> {
        if self.s[self.i..].starts_with(word.as_bytes()) {
            self.i += word.len();
            Some(v)
        } else {
            None
        }
    }

    fn number(&mut self) -> Option<Value> {
        let s = self.i;
        if self.s.get(self.i) == Some(&b'-') {
            self.i += 1;
        }
        let digits = |j: &mut Self| {
            let st = j.i;
            while j.i < j.s.len() && j.s[j.i].is_ascii_digit() {
                j.i += 1;
            }
            j.i - st
        };
        if self.s.get(self.i) == Some(&b'0') {
            self.i += 1;
        } else if digits(self) == 0 {
            return None;
        }
        if self.s.get(self.i) == Some(&b'.') {
            self.i += 1;
            if digits(self) == 0 {
                return None;
            }
        }
        if matches!(self.s.get(self.i), Some(b'e' | b'E')) {
            self.i += 1;
            if matches!(self.s.get(self.i), Some(b'+' | b'-')) {
                self.i += 1;
            }
            if digits(self) == 0 {
                return None;
            }
        }
        std::str::from_utf8(&self.s[s..self.i]).ok()?.parse().ok().map(Value::Num)
    }

    fn string(&mut self) -> Option<String> {
        if self.s.get(self.i) != Some(&b'"') {
            return None;
        }
        self.i += 1;
        let mut units: Vec<u16> = Vec::new();
        loop {
            let c = *self.s.get(self.i)?;
            match c {
                b'"' => {
                    self.i += 1;
                    return Some(String::from_utf16_lossy(&units));
                }
                b'\\' => {
                    let e = *self.s.get(self.i + 1)?;
                    self.i += 2;
                    match e {
                        b'"' => units.push(34),
                        b'\\' => units.push(92),
                        b'/' => units.push(47),
                        b'b' => units.push(8),
                        b'f' => units.push(12),
                        b'n' => units.push(10),
                        b'r' => units.push(13),
                        b't' => units.push(9),
                        b'u' => {
                            let h = std::str::from_utf8(self.s.get(self.i..self.i + 4)?).ok()?;
                            if !h.bytes().all(|b| b.is_ascii_hexdigit()) {
                                return None;
                            }
                            units.push(u16::from_str_radix(h, 16).ok()?);
                            self.i += 4;
                        }
                        _ => return None,
                    }
                }
                0..=0x1f => return None,
                _ => {
                    let rest = std::str::from_utf8(&self.s[self.i..]).ok()?;
                    let ch = rest.chars().next()?;
                    let mut buf = [0u16; 2];
                    units.extend_from_slice(ch.encode_utf16(&mut buf));
                    self.i += ch.len_utf8();
                }
            }
        }
    }
}

pub fn parse_json(text: &str) -> Result<Value, ()> {
    let mut j = J { s: text.as_bytes(), i: 0 };
    let v = j.value(0).ok_or(())?;
    j.ws();
    if j.i != j.s.len() {
        return Err(());
    }
    Ok(v)
}
