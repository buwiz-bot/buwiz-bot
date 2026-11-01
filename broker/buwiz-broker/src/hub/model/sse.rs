//! A Messages stream as the SDK (`@anthropic-ai/sdk` 0.133) reads it: the
//! body split into lines (`LineDecoder`), lines into server-sent events
//! (`SSEDecoder`), and the events folded into the final message
//! (`MessageStream#accumulateMessage`): text and its citations, a tool's
//! `input_json_delta` pieces parsed into `input` (none → `{}`), thinking
//! with its signature, server tool blocks and their results as they start.

use serde_json::{json, Map, Value};
use tokio::sync::mpsc::Receiver;

/// The body's bytes as lines: `\n`, `\r\n` and a lone `\r` end one.
#[derive(Default)]
pub struct LineDecoder {
    buf: Vec<u8>,
}

impl LineDecoder {
    pub fn decode(&mut self, chunk: &[u8]) -> Vec<String> {
        self.buf.extend_from_slice(chunk);
        let mut lines = Vec::new();
        loop {
            let Some(i) = self.buf.iter().position(|b| *b == b'\n' || *b == b'\r') else { break };
            if self.buf[i] == b'\r' && i + 1 == self.buf.len() {
                // a `\r` at the end may be the first half of `\r\n`
                break;
            }
            let end = if self.buf[i] == b'\r' && self.buf[i + 1] == b'\n' { i + 2 } else { i + 1 };
            lines.push(String::from_utf8_lossy(&self.buf[..i]).into_owned());
            self.buf.drain(..end);
        }
        lines
    }

    /// What is left at the body's end.
    pub fn flush(&mut self) -> Vec<String> {
        if self.buf.is_empty() {
            return Vec::new();
        }
        let rest = std::mem::take(&mut self.buf);
        let text = String::from_utf8_lossy(&rest);
        vec![text.trim_end_matches('\r').to_owned()]
    }
}

/// One server-sent event.
#[derive(Debug, PartialEq)]
pub struct Sse {
    pub event: Option<String>,
    pub data: String,
}

#[derive(Default)]
pub struct SseDecoder {
    event: Option<String>,
    data: Vec<String>,
}

impl SseDecoder {
    pub fn decode(&mut self, line: &str) -> Option<Sse> {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() {
            if self.event.is_none() && self.data.is_empty() {
                return None;
            }
            return Some(Sse { event: self.event.take(), data: std::mem::take(&mut self.data).join("\n") });
        }
        if line.starts_with(':') {
            return None;
        }
        let (field, value) = line.split_once(':').unwrap_or((line, ""));
        let value = value.strip_prefix(' ').unwrap_or(value);
        match field {
            "event" => self.event = Some(value.to_owned()),
            "data" => self.data.push(value.to_owned()),
            _ => {}
        }
        None
    }
}

/// The events the SDK hands on (`Stream.fromSSEResponse`); `ping` and
/// unknown names are skipped, `error` ends the stream with its body.
pub enum Item {
    Event(Value),
    Skip,
    /// the stream's `error` event: the APIError's message (no status)
    Error(String),
}

pub fn item(sse: &Sse) -> Result<Item, String> {
    match sse.event.as_deref() {
        Some("message_start" | "message_delta" | "message_stop" | "content_block_start" | "content_block_delta" | "content_block_stop") => {
            serde_json::from_str(&sse.data).map(Item::Event).map_err(|e| e.to_string())
        }
        Some("error") => {
            let body = serde_json::from_str::<Value>(&sse.data).unwrap_or_else(|_| Value::String(sse.data.clone()));
            Ok(Item::Error(super::anthropic::error_message(None, Some(&body), None)))
        }
        _ => Ok(Item::Skip),
    }
}

/// The message so far (`MessageStream`'s snapshot), event by event.
#[derive(Default)]
pub struct Accumulator {
    snapshot: Option<Value>,
    /// each tool block's `input_json_delta` text so far, by index
    bufs: std::collections::BTreeMap<usize, String>,
    /// the message `message_stop` closed (`receivedMessages`' last)
    done: Option<Value>,
}

const ORDER: &str = "Unexpected event order, got";

impl Accumulator {
    /// The message so far.
    pub fn snapshot(&self) -> Option<&Value> {
        self.snapshot.as_ref()
    }

    /// A tool block's input as streamed so far (the SDK's lazy `input`).
    pub fn partial_input(&self, index: usize) -> Option<Value> {
        self.bufs.get(&index).map(|buf| if buf.is_empty() { json!({}) } else { partial_parse(buf).unwrap_or(Value::Null) })
    }

    /// Folds one event in; `Err` is the SDK's AnthropicError message.
    pub fn push(&mut self, event: &Value) -> Result<(), String> {
        let kind = event.get("type").and_then(Value::as_str).unwrap_or("");
        if kind == "message_start" {
            if self.snapshot.is_some() {
                return Err(format!("{ORDER} {kind} before receiving \"message_stop\""));
            }
            self.snapshot = Some(event.get("message").cloned().unwrap_or(Value::Null));
            return Ok(());
        }
        let Some(snapshot) = self.snapshot.as_mut() else {
            return Err(format!("{ORDER} {kind} before \"message_start\""));
        };
        let Some(msg) = snapshot.as_object_mut() else { return Err("the message is not an object".to_owned()) };
        match kind {
            "message_stop" => {
                self.materialize_all()?;
                let mut msg = self.snapshot.clone().unwrap_or(Value::Null);
                if let Some(m) = msg.as_object_mut() {
                    m.insert("parsed_output".to_owned(), Value::Null);
                }
                self.done = Some(msg);
            }
            "message_delta" => {
                let delta = event.get("delta").cloned().unwrap_or(Value::Null);
                for key in ["stop_reason", "stop_sequence", "stop_details"] {
                    set_or_drop(msg, key, delta.get(key));
                }
                let usage = event.get("usage").cloned().unwrap_or(Value::Null);
                if let Some(u) = msg.get_mut("usage").and_then(Value::as_object_mut) {
                    set_or_drop(u, "output_tokens", usage.get("output_tokens"));
                }
                if let Some(c) = delta.get("container").filter(|c| !c.is_null()) {
                    msg.insert("container".to_owned(), c.clone());
                }
                if let Some(u) = msg.get_mut("usage").and_then(Value::as_object_mut) {
                    for key in ["input_tokens", "cache_creation_input_tokens", "cache_read_input_tokens", "server_tool_use", "output_tokens_details"] {
                        if let Some(v) = usage.get(key).filter(|v| !v.is_null()) {
                            u.insert(key.to_owned(), v.clone());
                        }
                    }
                }
            }
            "content_block_start" => {
                let block = event.get("content_block").cloned().unwrap_or(Value::Null);
                if let Some(content) = msg.get_mut("content").and_then(Value::as_array_mut) {
                    content.push(block);
                }
            }
            "content_block_delta" => {
                let index = event.get("index").and_then(Value::as_u64).unwrap_or(u64::MAX) as usize;
                let delta = event.get("delta").cloned().unwrap_or(Value::Null);
                let Some(block) = msg.get_mut("content").and_then(Value::as_array_mut).and_then(|c| c.get_mut(index)) else { return Ok(()) };
                let block_type = block.get("type").and_then(Value::as_str).unwrap_or("").to_owned();
                let Some(b) = block.as_object_mut() else { return Ok(()) };
                match delta.get("type").and_then(Value::as_str).unwrap_or("") {
                    "text_delta" if block_type == "text" => {
                        let text = b.get("text").and_then(Value::as_str).unwrap_or("").to_owned() + &string_or_undefined(delta.get("text"));
                        b.insert("text".to_owned(), Value::String(text));
                    }
                    "citations_delta" if block_type == "text" => {
                        let mut list = b.get("citations").and_then(Value::as_array).cloned().unwrap_or_default();
                        list.push(delta.get("citation").cloned().unwrap_or(Value::Null));
                        b.insert("citations".to_owned(), Value::Array(list));
                    }
                    "input_json_delta" if block_type == "tool_use" || block_type == "server_tool_use" => {
                        let piece = string_or_undefined(delta.get("partial_json"));
                        let first = !self.bufs.contains_key(&index);
                        self.bufs.entry(index).or_default().push_str(&piece);
                        if first {
                            // withLazyInput: `input` moves after the other keys
                            b.shift_remove("input");
                            b.insert("input".to_owned(), json!({}));
                        }
                    }
                    "thinking_delta" if block_type == "thinking" => {
                        let text = js_concat(b.get("thinking"), delta.get("thinking"));
                        b.insert("thinking".to_owned(), Value::String(text));
                    }
                    "signature_delta" if block_type == "thinking" => {
                        set_or_drop(b, "signature", delta.get("signature"));
                    }
                    _ => {}
                }
            }
            "content_block_stop" => {
                let index = event.get("index").and_then(Value::as_u64).unwrap_or(u64::MAX) as usize;
                self.materialize(index)?;
            }
            _ => {}
        }
        Ok(())
    }

    /// A tool block's input parsed from its buffer (the getter's first read).
    fn materialize(&mut self, index: usize) -> Result<(), String> {
        let Some(buf) = self.bufs.remove(&index) else { return Ok(()) };
        let input = if buf.is_empty() { json!({}) } else { partial_parse(&buf)? };
        if let Some(b) = self.snapshot.as_mut().and_then(|s| s.get_mut("content")).and_then(Value::as_array_mut).and_then(|c| c.get_mut(index)).and_then(Value::as_object_mut) {
            b.insert("input".to_owned(), input);
        }
        Ok(())
    }

    fn materialize_all(&mut self) -> Result<(), String> {
        let pending: Vec<usize> = self.bufs.keys().copied().collect();
        for i in pending {
            self.materialize(i)?;
        }
        Ok(())
    }

    /// The final message once the body ended (`finalMessage()`).
    pub fn finish(self) -> Result<Value, String> {
        if self.snapshot.is_none() {
            return Err("request ended without sending any chunks".to_owned());
        }
        self.done.ok_or_else(|| "stream ended without producing a Message with role=assistant".to_owned())
    }
}

/// `obj.key = value`, where an undefined value drops out of the JSON.
fn set_or_drop(obj: &mut Map<String, Value>, key: &str, value: Option<&Value>) {
    match value {
        Some(v) => {
            obj.insert(key.to_owned(), v.clone());
        }
        None => {
            obj.shift_remove(key);
        }
    }
}

fn string_or_undefined(v: Option<&Value>) -> String {
    v.map(crate::js::string_of).unwrap_or_else(|| "undefined".to_owned())
}

/// `a + b` for two values JavaScript adds as strings.
fn js_concat(a: Option<&Value>, b: Option<&Value>) -> String {
    string_or_undefined(a) + &string_or_undefined(b)
}

/// Drains a `ModelApi::stream` receiver into the final message, calling
/// `on_event(event, snapshot)` after each event is folded in (the SDK's
/// `streamEvent`, what the tool loop's placeholders read). Errors come back
/// in the client's `llm-api <status>: <message>` form.
pub async fn collect(mut rx: Receiver<Result<Value, String>>, mut on_event: impl FnMut(&Value, &Accumulator)) -> Result<Value, String> {
    let mut acc = Accumulator::default();
    while let Some(item) = rx.recv().await {
        let event = item?;
        acc.push(&event).map_err(|m| super::anthropic::llm(None, &m))?;
        on_event(&event, &acc);
    }
    acc.finish().map_err(|m| super::anthropic::llm(None, &m))
}

// ---- the SDK's vendored partial-json-parser ----

#[derive(Clone, Debug)]
struct Token {
    kind: &'static str,
    value: String,
    unterminated: bool,
}

fn tok(kind: &'static str, value: &str) -> Token {
    Token { kind, value: value.to_owned(), unterminated: false }
}

fn tokenize(input: &str) -> Vec<Token> {
    let chars: Vec<char> = input.chars().collect();
    let mut current = 0;
    let mut tokens = Vec::new();
    while current < chars.len() {
        let c = chars[current];
        match c {
            '\\' => {
                current += 1;
                continue;
            }
            '{' | '}' => tokens.push(tok("brace", &c.to_string())),
            '[' | ']' => tokens.push(tok("paren", &c.to_string())),
            ':' => tokens.push(tok("separator", ":")),
            ',' => tokens.push(tok("delimiter", ",")),
            '"' => {
                let start = current + 1;
                let mut end = start;
                let mut dangling = false;
                loop {
                    match chars[end.min(chars.len())..].iter().position(|x| *x == '"') {
                        None => {
                            dangling = true;
                            break;
                        }
                        Some(p) => end += p,
                    }
                    let mut backslashes = 0;
                    let mut i = end as isize - 1;
                    while i >= start as isize && chars[i as usize] == '\\' {
                        backslashes += 1;
                        i -= 1;
                    }
                    if backslashes % 2 == 0 {
                        break;
                    }
                    end += 1;
                }
                if dangling {
                    current = chars.len();
                } else {
                    tokens.push(tok("string", &chars[start..end].iter().collect::<String>()));
                    current = end + 1;
                }
                continue;
            }
            c if crate::js::is_space(c) => {}
            c if c.is_ascii_digit() || c == '-' || c == '.' => {
                let mut value = String::new();
                if c == '-' {
                    value.push(c);
                    current += 1;
                }
                while let Some(&ch) = chars.get(current) {
                    let last = value.chars().last();
                    if ch.is_ascii_digit() || ch == '.' || ch == 'e' || ch == 'E' || ((ch == '-' || ch == '+') && matches!(last, Some('e' | 'E'))) {
                        value.push(ch);
                        current += 1;
                    } else {
                        break;
                    }
                }
                tokens.push(Token { kind: "number", value, unterminated: current == chars.len() });
                continue;
            }
            c if c.is_ascii_alphabetic() => {
                let mut value = String::new();
                while let Some(&ch) = chars.get(current).filter(|ch| ch.is_ascii_alphabetic()) {
                    value.push(ch);
                    current += 1;
                }
                if value == "true" || value == "false" || value == "null" {
                    tokens.push(tok("name", &value));
                } else {
                    // an unknown word, e.g. `nul`: skipped with the character after it
                    current += 1;
                }
                continue;
            }
            _ => {}
        }
        current += 1;
    }
    tokens
}

fn is_json_number(s: &str) -> bool {
    // ^-?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][-+]?[0-9]+)?$
    let b = s.as_bytes();
    let mut i = 0;
    if b.get(i) == Some(&b'-') {
        i += 1;
    }
    match b.get(i) {
        Some(b'0') => i += 1,
        Some(c) if c.is_ascii_digit() => {
            while b.get(i).is_some_and(u8::is_ascii_digit) {
                i += 1;
            }
        }
        _ => return false,
    }
    if b.get(i) == Some(&b'.') {
        i += 1;
        let s = i;
        while b.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        if i == s {
            return false;
        }
    }
    if matches!(b.get(i), Some(b'e' | b'E')) {
        i += 1;
        if matches!(b.get(i), Some(b'-' | b'+')) {
            i += 1;
        }
        let s = i;
        while b.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        if i == s {
            return false;
        }
    }
    i == b.len()
}

fn strip(tokens: Vec<Token>) -> Vec<Token> {
    let mut open: Vec<&str> = Vec::new();
    for t in &tokens {
        if t.kind == "brace" || t.kind == "paren" {
            if t.value == "{" || t.value == "[" {
                open.push(if t.value == "{" { "{" } else { "[" });
            } else {
                open.pop();
            }
        }
    }
    let innermost = open.last().copied();
    let mut length = tokens.len();
    while length > 0 {
        let last = &tokens[length - 1];
        match last.kind {
            "separator" => {
                length -= 1;
                continue;
            }
            "number" if last.unterminated || !is_json_number(&last.value) => {
                length -= 1;
                continue;
            }
            "string" => {
                let before = if length >= 2 { tokens.get(length - 2) } else { None };
                if innermost == Some("{") && before.is_some_and(|b| b.kind == "delimiter" || (b.kind == "brace" && b.value == "{")) {
                    length -= 1;
                    continue;
                }
            }
            "delimiter" => length -= 1,
            _ => {}
        }
        break;
    }
    tokens.into_iter().take(length).collect()
}

fn unstrip(mut tokens: Vec<Token>) -> Vec<Token> {
    let mut tail: Vec<char> = Vec::new();
    let remove_last = |tail: &mut Vec<char>, c: char| {
        // splice(lastIndexOf(c), 1): -1 removes the last element
        match tail.iter().rposition(|x| *x == c) {
            Some(i) => {
                tail.remove(i);
            }
            None => {
                tail.pop();
            }
        }
    };
    for t in &tokens {
        match (t.kind, t.value.as_str()) {
            ("brace", "{") => tail.push('}'),
            ("brace", _) => remove_last(&mut tail, '}'),
            ("paren", "[") => tail.push(']'),
            ("paren", _) => remove_last(&mut tail, ']'),
            _ => {}
        }
    }
    for c in tail.into_iter().rev() {
        tokens.push(if c == '}' { tok("brace", "}") } else { tok("paren", "]") });
    }
    tokens
}

fn generate(tokens: &[Token]) -> String {
    tokens.iter().map(|t| if t.kind == "string" { format!("\"{}\"", t.value) } else { t.value.clone() }).collect()
}

/// The SDK's `partialParse`: a JSON prefix closed off and parsed.
pub fn partial_parse(input: &str) -> Result<Value, String> {
    let text = generate(&unstrip(strip(tokenize(input))));
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_partial_parse_closes_what_a_cut_stream_left_open() {
        assert_eq!(partial_parse(r#"{"title":"Card","code":"impo"#).unwrap(), json!({"title": "Card"}));
        assert_eq!(partial_parse(r#"{"a":[1,2,{"b":tr"#).unwrap(), json!({"a": [1, 2, {}]}));
        assert_eq!(partial_parse(r#"{"a":12"#).unwrap(), json!({}));
        assert_eq!(partial_parse(r#"{"a":12,"#).unwrap(), json!({"a": 12}));
        assert_eq!(partial_parse(r#"{"a":"x\"y","b":"#).unwrap(), json!({"a": "x\"y"}));
        assert_eq!(partial_parse(r#"{"a":null,"b":[true,false]}"#).unwrap(), json!({"a": null, "b": [true, false]}));
        assert_eq!(partial_parse(r#"{"n":-1.5e+3}"#).unwrap(), json!({"n": -1500.0}));
        assert!(partial_parse("5").is_err());
    }

    #[test]
    fn model_lines_split_on_every_line_end_across_chunks() {
        let mut d = LineDecoder::default();
        assert_eq!(d.decode(b"event: a\r"), Vec::<String>::new());
        assert_eq!(d.decode(b"\ndata: {\"x\":\"\xc3"), vec!["event: a"]);
        assert_eq!(d.decode(b"\xa9\"}\n\nlast\rnext"), vec!["data: {\"x\":\"\u{e9}\"}", "", "last"]);
        assert_eq!(d.flush(), vec!["next"]);
        let mut s = SseDecoder::default();
        assert_eq!(s.decode(": comment"), None);
        assert_eq!(s.decode("event:x"), None);
        assert_eq!(s.decode("data: 1"), None);
        assert_eq!(s.decode("data:2"), None);
        assert_eq!(s.decode(""), Some(Sse { event: Some("x".to_owned()), data: "1\n2".to_owned() }));
        assert_eq!(s.decode(""), None);
    }

    #[test]
    fn model_accumulates_every_block_kind() {
        let mut acc = Accumulator::default();
        let events = [
            json!({"type": "message_start", "message": {"id": "m", "type": "message", "role": "assistant", "content": [], "stop_reason": null, "usage": {"input_tokens": 3, "output_tokens": 1}}}),
            json!({"type": "content_block_start", "index": 0, "content_block": {"type": "thinking", "thinking": "", "signature": ""}}),
            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "thinking_delta", "thinking": "Hm"}}),
            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "signature_delta", "signature": "sig"}}),
            json!({"type": "content_block_stop", "index": 0}),
            json!({"type": "content_block_start", "index": 1, "content_block": {"type": "text", "text": "", "citations": null}}),
            json!({"type": "content_block_delta", "index": 1, "delta": {"type": "text_delta", "text": "Hel"}}),
            json!({"type": "content_block_delta", "index": 1, "delta": {"type": "text_delta", "text": "lo"}}),
            json!({"type": "content_block_delta", "index": 1, "delta": {"type": "citations_delta", "citation": {"url": "u"}}}),
            json!({"type": "content_block_stop", "index": 1}),
            json!({"type": "content_block_start", "index": 2, "content_block": {"type": "tool_use", "id": "t", "name": "x", "input": {}, "caller": "c"}}),
            json!({"type": "content_block_delta", "index": 2, "delta": {"type": "input_json_delta", "partial_json": "{\"a\":"}}),
            json!({"type": "content_block_delta", "index": 2, "delta": {"type": "input_json_delta", "partial_json": "[1]}"}}),
            json!({"type": "content_block_stop", "index": 2}),
            json!({"type": "content_block_start", "index": 3, "content_block": {"type": "tool_use", "id": "u", "name": "y", "input": {}}}),
            json!({"type": "content_block_stop", "index": 3}),
            json!({"type": "message_delta", "delta": {"stop_reason": "tool_use", "stop_sequence": null}, "usage": {"output_tokens": 9}}),
            json!({"type": "message_stop"}),
        ];
        for e in &events {
            acc.push(e).unwrap();
        }
        let msg = acc.finish().unwrap();
        assert_eq!(
            crate::js::stringify(&msg),
            r#"{"id":"m","type":"message","role":"assistant","content":[{"type":"thinking","thinking":"Hm","signature":"sig"},{"type":"text","text":"Hello","citations":[{"url":"u"}]},{"type":"tool_use","id":"t","name":"x","caller":"c","input":{"a":[1]}},{"type":"tool_use","id":"u","name":"y","input":{}}],"stop_reason":"tool_use","usage":{"input_tokens":3,"output_tokens":9},"stop_sequence":null,"parsed_output":null}"#
        );
    }

    #[test]
    fn model_accumulator_errors_read_as_the_sdks() {
        let mut acc = Accumulator::default();
        assert_eq!(acc.push(&json!({"type": "content_block_start"})).unwrap_err(), "Unexpected event order, got content_block_start before \"message_start\"");
        acc.push(&json!({"type": "message_start", "message": {"content": [], "usage": {}}})).unwrap();
        assert_eq!(acc.push(&json!({"type": "message_start", "message": {}})).unwrap_err(), "Unexpected event order, got message_start before receiving \"message_stop\"");
        assert_eq!(acc.finish().unwrap_err(), "stream ended without producing a Message with role=assistant");
        assert_eq!(Accumulator::default().finish().unwrap_err(), "request ended without sending any chunks");
    }
}
