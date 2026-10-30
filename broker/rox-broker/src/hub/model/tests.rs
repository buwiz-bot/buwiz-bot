//! The model client against the in-test fake (`testkit`): routing and
//! headers, stream accumulation split across chunks, retries and error
//! texts, the legacy generator's calls on both providers, images and
//! `/models`.

use std::sync::Arc;

use serde_json::{json, Value};

use super::testkit::{sse_text, Answer, Fake, Local, PNG};
use super::*;
use crate::hub::types::ModelChoice;

fn settings(fake: &Fake, extra: &[(&str, &str)]) -> Settings {
    let mut env: Vec<(String, String)> = vec![("OPENCODEX_URL".into(), fake.url.clone()), ("ANTHROPIC_BASE_URL".into(), format!("{}/direct", fake.url))];
    env.extend(extra.iter().map(|(k, v)| (k.to_string(), v.to_string())));
    Settings::from(move |k| env.iter().rev().find(|(n, _)| n == k).map(|(_, v)| v.clone()))
}

fn client(s: &Settings) -> ModelClient {
    ModelClient::new(Arc::new(s.clone()))
}

fn turn(model: Option<ModelChoice>) -> TurnCtx {
    TurnCtx { model, ..Default::default() }
}

fn choice(model: &str, effort: &str, fast: bool, name: Option<&str>) -> Option<ModelChoice> {
    Some(ModelChoice { model: model.into(), effort: effort.into(), fast, name: name.map(Into::into) })
}

fn message(content: Value, stop: &str) -> Value {
    json!({"id": "msg_fake", "type": "message", "role": "assistant", "model": "m", "content": content, "stop_reason": stop, "stop_sequence": null, "usage": {"input_tokens": 10, "output_tokens": 10}})
}

/// The fake's stream for a message: text in two deltas, a tool's input in
/// three pieces, thinking and its signature, other blocks whole.
fn events_of(msg: &Value) -> Vec<Value> {
    let mut start = msg.clone();
    start["content"] = json!([]);
    start["stop_reason"] = Value::Null;
    let mut ev = vec![json!({"type": "message_start", "message": start})];
    for (index, b) in msg["content"].as_array().unwrap().iter().enumerate() {
        match b["type"].as_str().unwrap() {
            "text" => {
                ev.push(json!({"type": "content_block_start", "index": index, "content_block": {"type": "text", "text": ""}}));
                let t = b["text"].as_str().unwrap();
                let mid = t.char_indices().nth(t.chars().count() / 2).map(|(i, _)| i).unwrap_or(0);
                for piece in [&t[..mid], &t[mid..]] {
                    ev.push(json!({"type": "content_block_delta", "index": index, "delta": {"type": "text_delta", "text": piece}}));
                }
            }
            "thinking" => {
                ev.push(json!({"type": "content_block_start", "index": index, "content_block": {"type": "thinking", "thinking": "", "signature": ""}}));
                ev.push(json!({"type": "content_block_delta", "index": index, "delta": {"type": "thinking_delta", "thinking": b["thinking"]}}));
                ev.push(json!({"type": "content_block_delta", "index": index, "delta": {"type": "signature_delta", "signature": b["signature"]}}));
            }
            "tool_use" | "server_tool_use" => {
                ev.push(json!({"type": "content_block_start", "index": index, "content_block": {"type": b["type"], "id": b["id"], "name": b["name"], "input": {}}}));
                let s = crate::js::stringify(&b["input"]);
                if s != "{}" {
                    let size = s.len().div_ceil(3);
                    for piece in s.as_bytes().chunks(size) {
                        ev.push(json!({"type": "content_block_delta", "index": index, "delta": {"type": "input_json_delta", "partial_json": String::from_utf8(piece.to_vec()).unwrap()}}));
                    }
                }
            }
            _ => ev.push(json!({"type": "content_block_start", "index": index, "content_block": b})),
        }
        ev.push(json!({"type": "content_block_stop", "index": index}));
    }
    ev.push(json!({"type": "message_delta", "delta": {"stop_reason": msg["stop_reason"], "stop_sequence": null}, "usage": {"output_tokens": 10}}));
    ev.push(json!({"type": "message_stop"}));
    ev
}

fn say(text: &str) -> Value {
    message(json!([{"type": "text", "text": text}]), "end_turn")
}

#[tokio::test]
async fn model_routes_each_call_and_sends_the_sdks_headers() {
    let fake = Fake::start().await;
    let s = settings(&fake, &[]);
    let api = client(&s);
    let body = json!({"model": "claude-haiku-5-5", "max_tokens": 8, "messages": [{"role": "user", "content": "hi"}]});
    for _ in 0..4 {
        fake.push(Answer::json(200, &say("ok")));
    }
    assert_eq!(api.create(&turn(None), body.clone()).await.unwrap(), say("ok"));
    api.create(&turn(choice("gpt-5.5", "low", true, None)), body.clone()).await.unwrap();
    api.create(&turn(choice("", "max", false, Some("Bo"))), body.clone()).await.unwrap();
    api.create(&turn(choice("claude-x", "", false, Some("Ada"))), body.clone()).await.unwrap();
    let got = fake.taken();
    let brief: Vec<(String, String)> = got.iter().map(|r| (r.path.clone(), r.body.clone())).collect();
    assert_eq!(
        brief,
        [
            ("/direct/v1/messages".into(), r#"{"model":"claude-haiku-5-5","max_tokens":8,"messages":[{"role":"user","content":"hi"}]}"#.into()),
            ("/v1/messages".into(), r#"{"model":"gpt-5.5--fast","max_tokens":8,"messages":[{"role":"user","content":"hi"}],"output_config":{"effort":"low"},"reasoning_effort":"low"}"#.into()),
            (
                "/direct/v1/messages".into(),
                r#"{"model":"claude-haiku-5-5","max_tokens":8,"messages":[{"role":"user","content":"hi"}],"output_config":{"effort":"max"},"reasoning_effort":"max"}"#.into()
            ),
            ("/v1/messages".into(), r#"{"model":"claude-x","max_tokens":8,"messages":[{"role":"user","content":"hi"}]}"#.into()),
        ]
    );
    let r = &got[0];
    assert_eq!(
        (r.method.as_str(), r.header("x-api-key"), r.header("anthropic-version"), r.header("content-type"), r.header("accept"), r.header("authorization")),
        ("POST", Some("opencodex"), Some("2023-06-01"), Some("application/json"), Some("application/json"), None)
    );
    // a direct key and an auth token, as the SDK reads them
    let s = settings(&fake, &[("ANTHROPIC_API_KEY", "sk-1"), ("ANTHROPIC_AUTH_TOKEN", "tok")]);
    fake.push(Answer::json(200, &say("ok")));
    fake.push(Answer::json(200, &say("ok")));
    client(&s).create(&turn(None), body.clone()).await.unwrap();
    client(&s).create(&turn(choice("m", "", false, None)), body).await.unwrap();
    let got = fake.taken();
    assert_eq!((got[0].header("x-api-key"), got[0].header("authorization")), (Some("sk-1"), Some("Bearer tok")));
    assert_eq!((got[1].header("x-api-key"), got[1].header("authorization")), (Some("opencodex"), Some("Bearer tok")));
}

#[tokio::test]
async fn model_stream_accumulates_every_block_kind_split_across_chunks() {
    let fake = Fake::start().await;
    let s = settings(&fake, &[]);
    let api = client(&s);
    let msg = message(
        json!([
            {"type": "thinking", "thinking": "Let me think — é.", "signature": "sig-1"},
            {"type": "server_tool_use", "id": "srvtoolu_1", "name": "web_search", "input": {"query": "rox"}},
            {"type": "web_search_tool_result", "tool_use_id": "srvtoolu_1", "content": [{"type": "web_search_result", "url": "https://example.com/", "title": "Example", "encrypted_content": "x", "page_age": null}]},
            {"type": "text", "text": "Found it, naïvely."},
            {"type": "tool_use", "id": "toolu_1", "name": "generate_app", "input": {"title": "Card", "code": "export default function R() { return <Text>hi</Text>; }"}},
            {"type": "tool_use", "id": "toolu_2", "name": "react", "input": {}},
        ]),
        "tool_use",
    );
    let mut events = events_of(&msg);
    // citations arrive as their own deltas on a text block
    events.insert(events.len() - 2, json!({"type": "content_block_start", "index": 6, "content_block": {"type": "text", "text": "", "citations": null}}));
    events.insert(events.len() - 2, json!({"type": "content_block_delta", "index": 6, "delta": {"type": "text_delta", "text": "Cited."}}));
    events.insert(
        events.len() - 2,
        json!({"type": "content_block_delta", "index": 6, "delta": {"type": "citations_delta", "citation": {"type": "web_search_result_location", "url": "https://example.com/", "cited_text": "x"}}}),
    );
    events.insert(events.len() - 2, json!({"type": "content_block_stop", "index": 6}));
    // a ping and CRLF line ends, cut every 7 bytes (mid-line, mid-character)
    let text = format!("event: ping\ndata: {{\"type\":\"ping\"}}\n\n{}", sse_text(&events)).replace('\n', "\r\n");
    fake.push(Answer::sse(&text, 7));
    let rx = api.stream(&turn(None), json!({"model": "m", "max_tokens": 5, "messages": []})).await.unwrap();
    let mut seen = Vec::new();
    let mut placeholder = None;
    let got = sse::collect(rx, |e, acc| {
        seen.push(e["type"].as_str().unwrap().to_owned());
        if e["type"] == "content_block_delta" && e["index"] == 4 {
            placeholder = acc.partial_input(4);
        }
    })
    .await
    .unwrap();
    assert_eq!(seen.len(), events.len());
    assert_eq!(placeholder, Some(json!({"title": "Card", "code": "export default function R() { return <Text>hi</Text>; }"})));
    let mut want = msg.clone();
    want["content"].as_array_mut().unwrap().push(json!({"type": "text", "text": "Cited.", "citations": [{"type": "web_search_result_location", "url": "https://example.com/", "cited_text": "x"}]}));
    want["parsed_output"] = Value::Null;
    assert_eq!(got, want);
    assert_eq!(
        crate::js::stringify(&got["content"][4]),
        r#"{"type":"tool_use","id":"toolu_1","name":"generate_app","input":{"title":"Card","code":"export default function R() { return <Text>hi</Text>; }"}}"#
    );
    let req = &fake.taken()[0];
    assert_eq!(req.body, r#"{"model":"m","max_tokens":5,"messages":[],"stream":true}"#);
}

#[tokio::test]
async fn model_retries_429_and_5xx_twice_then_reports_the_sdks_error() {
    let fake = Fake::start().await;
    let s = settings(&fake, &[]);
    let api = client(&s);
    let body = json!({"model": "m", "max_tokens": 5, "messages": []});
    let err = |status: u16, kind: &str, msg: &str| Answer::status(status, &json!({"type": "error", "error": {"type": kind, "message": msg}}), &[("retry-after-ms", "1")]);
    fake.push(err(429, "rate_limit_error", "slow down"));
    fake.push(err(429, "rate_limit_error", "slow down"));
    fake.push(Answer::sse(&sse_text(&events_of(&say("After the retries."))), 64));
    let rx = api.stream(&turn(None), body.clone()).await.unwrap();
    assert_eq!(text_of(&sse::collect(rx, |_, _| {}).await.unwrap()), "After the retries.");
    assert_eq!(fake.taken().len(), 3);
    for _ in 0..3 {
        fake.push(err(500, "api_error", "upstream exploded"));
    }
    let e = api.stream(&turn(None), body.clone()).await.unwrap_err();
    assert_eq!(e, r#"llm-api 500: 500 {"type":"error","error":{"type":"api_error","message":"upstream exploded"}}"#);
    assert_eq!(fake.taken().len(), 3);
    // 408 and 409 retry too; 400 never; the server's x-should-retry wins
    fake.push(err(408, "timeout", "t"));
    fake.push(err(409, "conflict", "c"));
    fake.push(Answer::json(200, &say("ok")));
    assert!(api.create(&turn(None), body.clone()).await.is_ok());
    assert_eq!(fake.taken().len(), 3);
    fake.push(err(400, "invalid_request_error", "bad request"));
    assert_eq!(api.create(&turn(None), body.clone()).await.unwrap_err(), "llm-api 400: 400 {\"type\":\"error\",\"error\":{\"type\":\"invalid_request_error\",\"message\":\"bad request\"}}");
    fake.push(Answer::status(503, &json!({"message": "busy"}), &[("x-should-retry", "false")]));
    assert_eq!(api.create(&turn(None), body.clone()).await.unwrap_err(), "llm-api 503: 503 busy");
    fake.push(Answer::status(400, &json!({"message": "x"}), &[("x-should-retry", "true"), ("retry-after-ms", "1")]));
    fake.push(Answer::json(200, &say("ok")));
    assert!(api.create(&turn(None), body.clone()).await.is_ok());
    assert_eq!(fake.taken().len(), 4);
    // the SDK refuses a non-streaming call that may take over 10 minutes
    let big = json!({"model": "m", "max_tokens": 30000, "messages": []});
    assert!(api.create(&turn(None), big.clone()).await.unwrap_err().starts_with("llm-api : Streaming is required for operations that may take longer than 10 minutes."));
    assert!(fake.taken().is_empty());
    fake.push(Answer::sse(&sse_text(&events_of(&say("long"))), 64));
    assert!(api.stream(&turn(None), big).await.is_ok());
}

#[tokio::test]
async fn model_connection_errors_retry_then_read_connection_error() {
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let s = Settings::from(|k| (k == "OPENCODEX_URL").then(|| format!("http://127.0.0.1:{port}")));
    let started = std::time::Instant::now();
    let e = client(&s).create(&turn(None), json!({"model": "m", "max_tokens": 5, "messages": []})).await.unwrap_err();
    assert_eq!(e, "llm-api : Connection error.");
    // two backoffs: ~0.5 s and ~1 s, less up to a quarter of jitter
    assert!(started.elapsed() >= std::time::Duration::from_millis(1100), "{:?}", started.elapsed());
}

#[tokio::test]
async fn model_a_stream_that_dies_or_reports_an_error_fails_with_its_text() {
    let fake = Fake::start().await;
    let s = settings(&fake, &[]);
    let api = client(&s);
    let body = json!({"model": "m", "max_tokens": 5, "messages": []});
    // closed before the first event
    fake.push(Answer::Die(Vec::new()));
    let mut events = 0;
    let rx = api.stream(&turn(None), body.clone()).await.unwrap();
    assert_eq!(sse::collect(rx, |_, _| events += 1).await.unwrap_err(), "llm-api : terminated");
    assert_eq!(events, 0);
    // closed after the first event
    let first = sse_text(&events_of(&say("partial"))[..1]);
    fake.push(Answer::Die(first.into_bytes()));
    let rx = api.stream(&turn(None), body.clone()).await.unwrap();
    assert_eq!(sse::collect(rx, |_, _| events += 1).await.unwrap_err(), "llm-api : terminated");
    assert_eq!(events, 1);
    // an error event
    fake.push(Answer::sse("event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"Overloaded\"}}\n\n", 64));
    let rx = api.stream(&turn(None), body.clone()).await.unwrap();
    assert_eq!(sse::collect(rx, |_, _| {}).await.unwrap_err(), r#"llm-api : {"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#);
    // a body with no events, and one that ends before message_stop
    fake.push(Answer::sse("", 64));
    let rx = api.stream(&turn(None), body.clone()).await.unwrap();
    assert_eq!(sse::collect(rx, |_, _| {}).await.unwrap_err(), "llm-api : request ended without sending any chunks");
    let cut = events_of(&say("x"));
    fake.push(Answer::sse(&sse_text(&cut[..cut.len() - 1]), 64));
    let rx = api.stream(&turn(None), body).await.unwrap();
    assert_eq!(sse::collect(rx, |_, _| {}).await.unwrap_err(), "llm-api : stream ended without producing a Message with role=assistant");
}

const CODE: &str = "import { Text } from \"nsh\";\n\nexport default function R() {\n  return <Text>hi</Text>;\n}";

#[tokio::test]
async fn model_ask_model_streams_the_module_with_its_follow_ups() {
    let fake = Fake::start().await;
    let s = settings(&fake, &[]);
    let api = client(&s);
    fake.push(Answer::sse(&sse_text(&events_of(&say(&format!("Fixed it\n```tsx\n{CODE}\n```")))), 9));
    let mut typed = 0;
    let ask = Ask { prompt: "build", prev_error: Some("error TS2304\n"), prev_code: Some("bad"), name: "R4", linear: false };
    let m = ask_model(&s, &api, &turn(choice("gpt-5.5", "high", false, Some("Ada"))), &ask, |_| unreachable!(), || typed += 1).await.unwrap();
    assert_eq!(m, Module { caption: "Fixed it".into(), code: CODE.into() });
    assert_eq!(typed, 2);
    let r = fake.taken().remove(0).json();
    assert_eq!(r.as_object().unwrap().keys().collect::<Vec<_>>(), ["model", "max_tokens", "system", "messages", "output_config", "reasoning_effort", "stream"]);
    assert_eq!((&r["model"], &r["max_tokens"], &r["system"]), (&json!("gpt-5.5"), &json!(20000), &json!(prompts::system_for("R4", false))));
    assert_eq!(
        r["messages"],
        json!([{"role": "user", "content": "build"}, {"role": "assistant", "content": "bad"}, {"role": "user", "content": "The nsh compiler rejected it:\nerror TS2304\n\nReturn a corrected module only."}])
    );
    // truncation and a reply without a module are the generator's to retry
    fake.push(Answer::sse(&sse_text(&events_of(&message(json!([{"type": "text", "text": "```tsx\nexport default function R() { return <"}]), "max_tokens"))), 64));
    let ask = Ask { prompt: "p", prev_error: Some("module truncated at the token limit"), name: "R5", ..Default::default() };
    assert_eq!(ask_model(&s, &api, &turn(None), &ask, |_| unreachable!(), || {}).await.unwrap_err(), TRUNCATED);
    assert_eq!(fake.taken()[0].json()["messages"][1]["content"], json!("Your reply was cut off at the token limit. Return the SAME module but shorter: fewer comments, tighter code, no extras."));
    fake.push(Answer::sse(&sse_text(&events_of(&say("I would rather chat."))), 64));
    assert_eq!(ask_model(&s, &api, &turn(None), &Ask { prompt: "p", name: "R5", ..Default::default() }, |_| unreachable!(), || {}).await.unwrap_err(), NO_MODULE);
    fake.taken();
    fake.push(Answer::status(400, &json!({"type": "error", "error": {"type": "invalid_request_error", "message": "bad request"}}), &[]));
    assert_eq!(
        ask_model(&s, &api, &turn(None), &Ask { prompt: "p", name: "R5", ..Default::default() }, |_| unreachable!(), || {}).await.unwrap_err(),
        r#"llm-api 400: 400 {"type":"error","error":{"type":"invalid_request_error","message":"bad request"}}"#
    );
    assert_eq!(fake.taken().len(), 1, "a 400 is not retried");
    // MOCK answers with the pipeline's mock module
    let mock = settings(&fake, &[("MOCK", "1")]);
    let m = ask_model(&mock, &api, &turn(None), &Ask { prompt: "a timer", name: "R1", ..Default::default() }, |p| format!("mock {p}"), || {}).await.unwrap();
    assert_eq!(m, Module { caption: String::new(), code: "mock a timer".into() });
    assert!(fake.taken().is_empty());
}

#[tokio::test]
async fn model_classify_and_chat_reply_on_the_anthropic_path() {
    let fake = Fake::start().await;
    let s = settings(&fake, &[]);
    let api = client(&s);
    fake.push(Answer::json(200, &say(" ui please")));
    fake.push(Answer::json(200, &say("CHAT")));
    fake.push(Answer::status(400, &json!({"message": "no"}), &[]));
    assert_eq!(classify_intent(&s, &api, &turn(None), "make a timer").await, "ui");
    assert_eq!(classify_intent(&s, &api, &turn(None), "hello").await, "chat");
    assert_eq!(classify_intent(&s, &api, &turn(None), "x").await, "ui", "a failed classifier generates anyway");
    let r = fake.taken().remove(0).json();
    assert_eq!(r, json!({"model": "claude-haiku-5-5", "max_tokens": 8, "system": prompts::CLASSIFY, "messages": [{"role": "user", "content": "make a timer"}]}));
    let mut history: Vec<Value> = (0..12).map(|i| json!({"role": if i % 2 == 0 { "user" } else { "ai" }, "text": format!("t{i}")})).collect();
    history.push(json!({"role": "notice", "text": "routine ran"}));
    history.push(json!({"role": "ai", "name": "R3", "text": "x"}));
    history.push(json!({"role": "ai"}));
    history.push(json!({"role": "user"}));
    fake.push(Answer::json(200, &say("  Hi there.  ")));
    assert_eq!(chat_reply(&s, &api, &turn(choice("", "low", false, None)), &history, json!("hello")).await, "Hi there.");
    let r = fake.taken().remove(0).json();
    assert_eq!(r.as_object().unwrap().keys().collect::<Vec<_>>(), ["model", "max_tokens", "system", "messages", "output_config", "reasoning_effort"]);
    assert_eq!(r["system"], json!(prompts::CHAT));
    let msgs = r["messages"].as_array().unwrap();
    assert_eq!(msgs.len(), 11);
    assert_eq!(msgs[0], json!({"role": "assistant", "content": "t5"}));
    assert_eq!(msgs[1], json!({"role": "user", "content": "t6"}));
    assert_eq!(msgs[7], json!({"role": "assistant", "content": "[built widget R3]"}));
    assert_eq!(msgs[8], json!({"role": "assistant", "content": ""}));
    assert_eq!(msgs[9], json!({"role": "user"}));
    assert_eq!(msgs[10], json!({"role": "user", "content": "hello"}));
    for _ in 0..3 {
        fake.push(Answer::status(500, &json!({"type": "error", "error": {"type": "api_error", "message": "down"}}), &[("retry-after-ms", "1")]));
    }
    assert_eq!(chat_reply(&s, &api, &turn(None), &[], json!("hi")).await, r#"_(model error — 500 {"type":"error","error":{"type":"api_error","message":"down"}})_"#);
    let mock = settings(&fake, &[("MOCK", "1")]);
    fake.taken();
    assert_eq!(chat_reply(&mock, &api, &turn(None), &[], json!("hi")).await, mock::CHAT_REPLY);
    assert_eq!(classify_intent(&mock, &api, &turn(None), "hi").await, "ui");
    assert!(fake.taken().is_empty());
}

fn completion(text: &str, finish: &str) -> Value {
    json!({"id": "chatcmpl-fake", "object": "chat.completion", "model": "m", "choices": [{"index": 0, "message": {"role": "assistant", "content": text}, "finish_reason": finish}]})
}

#[tokio::test]
async fn model_the_openai_path_classifies_chats_and_generates_over_chat_completions() {
    let fake = Fake::start().await;
    let s = settings(&fake, &[("GEN_PROVIDER", "openai"), ("GEN_BASE_URL", &format!("{}/v1", fake.url)), ("OPENROUTER_API_KEY", "test-key")]);
    let api = client(&s);
    assert_eq!(s.model, "openai/gpt-4o-mini");
    fake.push(Answer::json(200, &completion("CHAT", "stop")));
    assert_eq!(classify_intent(&s, &api, &turn(None), "hello").await, "chat");
    let r = fake.taken().remove(0);
    assert_eq!((r.path.as_str(), r.header("authorization"), r.header("content-type")), ("/v1/chat/completions", Some("Bearer test-key"), Some("application/json")));
    assert_eq!(
        r.body,
        crate::js::stringify(&json!({"model": "openai/gpt-4o-mini", "messages": [{"role": "system", "content": prompts::CLASSIFY}, {"role": "user", "content": "hello"}], "max_tokens": 8}))
    );
    // BUG: chatReply's system prompt never reaches the OpenAI path (MODEL-04)
    fake.push(Answer::json(200, &completion(" Hi from the fake. ", "stop")));
    let content = json!([{"type": "text", "text": "look"}, {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "AAAA"}}]);
    assert_eq!(chat_reply(&s, &api, &turn(None), &[json!({"role": "user", "text": "before"})], content).await, "Hi from the fake.");
    assert_eq!(
        fake.taken().remove(0).body,
        crate::js::stringify(
            &json!({"model": "openai/gpt-4o-mini", "messages": [{"role": "user", "content": "before"}, {"role": "user", "content": [{"type": "text", "text": "look"}, {"type": "image_url", "image_url": {"url": "data:image/png;base64,AAAA"}}]}], "max_tokens": 1024})
        )
    );
    assert_eq!(chat_reply(&s, &api, &turn(None), &[json!({"role": "user"})], json!("x")).await, "_(model error — Cannot read properties of undefined (reading 'map'))_");
    fake.push(Answer::json(200, &json!({"choices": []})));
    assert_eq!(chat_reply(&s, &api, &turn(None), &[], json!("x")).await, "(empty reply)");
    // the generator: truncation, then a module; an HTTP error is llm-api
    fake.taken();
    fake.push(Answer::json(200, &completion("```tsx\nexport default function R() { return <", "length")));
    let ask = Ask { prompt: "a card", name: "R2", ..Default::default() };
    assert_eq!(ask_model(&s, &api, &turn(None), &ask, |_| unreachable!(), || {}).await.unwrap_err(), TRUNCATED);
    fake.push(Answer::json(200, &completion(&format!("A card\n```tsx\n{CODE}\n```"), "stop")));
    let ask = Ask { prompt: "a card", prev_error: Some(TRUNCATED), name: "R2", ..Default::default() };
    assert_eq!(ask_model(&s, &api, &turn(None), &ask, |_| unreachable!(), || {}).await.unwrap(), Module { caption: "A card".into(), code: CODE.into() });
    let got = fake.taken();
    let first = got[0].json();
    assert_eq!(first.as_object().unwrap().keys().collect::<Vec<_>>(), ["model", "messages", "temperature", "max_tokens"]);
    assert_eq!((&first["temperature"], &first["max_tokens"], &first["messages"][0]["content"]), (&json!(0.4), &json!(20000), &json!(prompts::system_for("R2", false))));
    assert_eq!(
        got[1].json()["messages"].as_array().unwrap()[1..],
        [
            json!({"role": "user", "content": "a card"}),
            json!({"role": "user", "content": "Your reply was cut off at the token limit. Return the SAME module but shorter: fewer comments, tighter code, no extras."})
        ]
    );
    fake.push(Answer::status(500, &json!({"error": {"message": "down"}}), &[]));
    assert_eq!(ask_model(&s, &api, &turn(None), &ask, |_| unreachable!(), || {}).await.unwrap_err(), r#"llm-api 500: {"error":{"message":"down"}}"#);
    assert_eq!(fake.taken().len(), 1, "no retries on the OpenAI path");
    // no key: the template literal's "undefined"
    let nokey = settings(&fake, &[("GEN_PROVIDER", "openai"), ("GEN_BASE_URL", &format!("{}/v1", fake.url))]);
    fake.push(Answer::json(200, &completion("UI", "stop")));
    classify_intent(&nokey, &api, &turn(None), "x").await;
    assert_eq!(fake.taken()[0].header("authorization"), Some("Bearer undefined"));
}

#[test]
fn model_extract_code_reads_the_fence_and_caption() {
    let m = extract_code(&format!("  Here it is\n```tsx\n{CODE}\n```\nEnjoy ```js\nx\n``` it  ")).unwrap();
    assert_eq!(m, Module { caption: "Here it is\n\nEnjoy  it".into(), code: CODE.into() });
    let m = extract_code("```\nfunction R() { return <Text>x</Text>; }\n```").unwrap();
    assert_eq!(m.code, "function R() { return <Text>x</Text>; }\nexport default R;\n");
    let m = extract_code("````tsx  \n \nexport default () => <A/>\n```").unwrap();
    assert_eq!(m.code, "export default () => <A/>");
    assert_eq!(extract_code("```python\nprint(1)\n```").unwrap_err(), NO_MODULE);
    assert_eq!(extract_code("export default 1").unwrap_err(), NO_MODULE);
    assert_eq!(extract_code("").unwrap_err(), NO_MODULE);
    assert_eq!(extract_code("export default function X() { return <A/> }").unwrap().caption, "");
}

#[test]
fn model_streaming_stays_off_once_turned_off() {
    assert!(agent_stream());
    set_agent_stream(false);
    assert!(!agent_stream());
    set_agent_stream(true);
}

#[test]
fn model_settings_read_as_config_mjs() {
    let s = Settings::from(|_| None);
    assert_eq!((s.provider.as_str(), s.model.as_str(), &s.max_tokens, s.mock), ("anthropic", "claude-haiku-5-5", &json!(20000), false));
    assert_eq!(
        (s.opencodex.as_str(), s.direct_base.as_str(), s.direct_key.as_str(), s.base_url.as_str()),
        ("http://localhost:8080", "http://localhost:8080", "opencodex", "https://openrouter.ai/api/v1")
    );
    let s = Settings::from(|k| match k {
        "GEN_MAX_TOKENS" => Some("abc".into()),
        "ANTHROPIC_BASE_URL" => Some(String::new()),
        "OPENCODEX_URL" => Some("http://oc".into()),
        "MOCK" => Some("1".into()),
        _ => None,
    });
    assert_eq!((&s.max_tokens, s.direct_base.as_str(), s.mock), (&Value::Null, "http://oc", true));
    assert_eq!(Settings::from(|k| (k == "GEN_MAX_TOKENS").then(|| "8000".into())).max_tokens, json!(8000));
}

#[tokio::test]
async fn model_images_draw_through_the_relay_or_fall_back_to_an_svg() {
    let fake = Fake::start().await;
    let s = settings(&fake, &[]);
    let api = client(&s);
    let local = Local::default();
    let dir = std::env::temp_dir().join(format!("rox-model-images-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let b64 = base64::engine::general_purpose::STANDARD.encode(PNG);
    use base64::Engine;
    // which models draw: an image modality or the OpenAI login; a minute's cache
    fake.push(Answer::json(200, &json!({"data": [{"id": "painter", "capabilities": {"output_modalities": ["text", "image"]}}, {"id": "gpt-5.5--fast", "owned_by": "openai"}, {"id": "claude"}]})));
    assert!(images::image_capable(&s.opencodex, "painter").await);
    assert!(images::image_capable(&s.opencodex, "gpt-5.5--fast").await);
    assert!(!images::image_capable(&s.opencodex, "claude").await);
    assert_eq!(fake.taken().iter().map(|r| (r.method.clone(), r.path.clone())).collect::<Vec<_>>(), [("GET".to_owned(), "/v1/models".to_owned())]);
    // the relay
    let out = dir.join("a.png");
    fake.push(Answer::json(200, &json!({"created": 1, "data": [{"b64_json": b64}]})));
    let opts = images::ImageOpts { size: "512x512".into(), background: Some("transparent".into()), ..Default::default() };
    assert_eq!(images::generate_image(&s, &api, &turn(None), &local, "a cat", &out, &opts).await.unwrap(), "chatgpt");
    assert_eq!(std::fs::read(&out).unwrap(), PNG);
    let r = fake.taken().remove(0);
    assert_eq!((r.path.as_str(), r.body.as_str()), ("/v1/images/generations", r#"{"prompt":"a cat","size":"512x512","n":1,"background":"transparent"}"#));
    let raw = format!("{}.raw", out.display());
    assert_eq!(local.calls.lock().unwrap().remove(0), ["sips", "-s", "format", "png", &raw, "--out", &out.display().to_string()]);
    assert!(!std::path::Path::new(&raw).exists());
    // an edit sends the source as a data URL
    let edited = dir.join("b.png");
    fake.push(Answer::json(200, &json!({"data": [{"b64_json": b64}]})));
    assert_eq!(images::edit_image(&s, &local, &out, "make it blue", &edited, "1024x1024").await.unwrap(), "chatgpt");
    assert_eq!(fake.taken()[0].body, format!(r#"{{"prompt":"make it blue","size":"1024x1024","images":[{{"image_url":"data:image/png;base64,{b64}"}}]}}"#));
    fake.push(Answer::json(400, &json!({"error": {"message": "no edits today"}})));
    assert_eq!(images::edit_image(&s, &local, &out, "x", &edited, "1024x1024").await.unwrap_err(), "image edit failed: no edits today");
    fake.push(Answer::json(502, &json!({})));
    assert_eq!(images::edit_image(&s, &local, &out, "x", &edited, "1024x1024").await.unwrap_err(), "image edit failed: 502");
    // the relay fails, no keys (secret has none): the chat model draws an SVG
    fake.taken();
    local.calls.lock().unwrap().clear();
    fake.push(Answer::json(500, &json!({"error": {"message": "quota"}})));
    fake.push(Answer::json(200, &say("Sure: <svg viewBox=\"0 0 1 1\"></svg> done")));
    let svg_out = dir.join("c.png");
    let backend = images::generate_avatar_image(&s, &api, &turn(choice("gpt-5.5", "", false, None)), &local, "a fox", &svg_out).await.unwrap();
    assert_eq!(backend, "claude-svg");
    assert_eq!(std::fs::read(&svg_out).unwrap(), PNG);
    let got = fake.taken();
    assert_eq!(got[1].path, "/v1/messages");
    let body = got[1].json();
    assert_eq!(body["model"], json!("gpt-5.5"));
    assert_eq!(body["max_tokens"], json!(8000));
    assert!(body["messages"][0]["content"].as_str().unwrap().starts_with("Draw a polished, detailed flat-illustration profile avatar as a single SVG"));
    assert!(body["messages"][0]["content"].as_str().unwrap().ends_with("Subject: a fox. Reply with only the <svg>…</svg> markup."));
    let calls: Vec<String> = local.calls.lock().unwrap().iter().map(|c| c[0].clone()).collect();
    assert!(calls.starts_with(&["secret".to_owned(), "secret".to_owned()]) || std::env::var("GEMINI_API_KEY").is_ok() || std::env::var("OPENAI_API_KEY").is_ok());
    assert!(calls.ends_with(&["qlmanage".to_owned(), "mv".to_owned()]));
    // the OpenAI provider has no Messages client to draw with
    let oai = settings(&fake, &[("GEN_PROVIDER", "openai")]);
    fake.push(Answer::json(500, &json!({"error": {"message": "quota"}})));
    assert_eq!(
        images::generate_image(&oai, &api, &turn(None), &local, "x", &dir.join("d.png"), &Default::default()).await.unwrap_err(),
        "no image model available (add GEMINI or OPENAI quota) — opencodex: quota"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn model_models_route_folds_fast_twins_efforts_images_and_connections() {
    let fake = Fake::start().await;
    let s = settings(&fake, &[]);
    let home = std::env::temp_dir().join(format!("rox-model-home-{}", std::process::id()));
    std::fs::create_dir_all(home.join(".opencodex")).unwrap();
    std::fs::write(home.join(".opencodex/config.json"), r#"{"defaultProvider":"openai","providers":{"openai":{"authMode":"chatgpt","label":"ChatGPT"},"anthropic-apikey":{"authMode":"apikey"}}}"#)
        .unwrap();
    let models = json!({"object": "list", "data": [
        {"id": "claude-haiku-5-5", "owned_by": "anthropic-apikey", "reasoning_efforts": [{"value": "low", "label": "Low Effort"}, {"value": "high", "label": "High"}], "reasoning_effort": "low"},
        {"id": "claude-haiku-5-5--fast", "owned_by": "anthropic-apikey"},
        {"id": "gpt-5.5", "owned_by": "openai", "reasoning_efforts": [{"value": "medium"}]},
        {"id": "openrouter/qwen-3", "owned_by": "openrouter"},
        {"id": "painter", "owned_by": "xai", "capabilities": {"output_modalities": ["text", "image"]}},
        {"id": "plain"},
    ]});
    fake.push(Answer::json(200, &models));
    fake.push(Answer::json(200, &models));
    let got = crate::hub::routes::models::models_json(&s, &home).await;
    assert_eq!(
        crate::js::stringify(&got),
        crate::js::stringify(&json!({
            "ok": true,
            "default": "claude-haiku-5-5",
            "models": [
                {"id": "claude-haiku-5-5", "owner": "anthropic-apikey", "fast": true, "efforts": [{"value": "low", "label": "Low"}, {"value": "high", "label": "High"}], "defaultEffort": "low", "image": false, "connection": "anthropic-apikey", "group": "Anthropic", "subscription": true},
                {"id": "gpt-5.5", "owner": "openai", "fast": false, "efforts": [{"value": "medium", "label": "medium"}], "defaultEffort": "", "image": true, "connection": "openai", "group": "ChatGPT", "subscription": true},
                {"id": "openrouter/qwen-3", "owner": "openrouter", "fast": false, "efforts": [], "defaultEffort": "", "image": false, "connection": "openrouter", "group": "OpenRouter", "subscription": true},
                {"id": "painter", "owner": "xai", "fast": false, "efforts": [], "defaultEffort": "", "image": true, "connection": "xai", "group": "xAI", "subscription": true},
                {"id": "plain", "owner": "", "fast": false, "efforts": [], "defaultEffort": "", "image": false, "connection": "openai", "group": "ChatGPT", "subscription": true},
            ],
        }))
    );
    assert_eq!(fake.taken().iter().map(|r| (r.method.clone(), r.path.clone())).collect::<Vec<_>>(), [("GET".to_owned(), "/v1/models".to_owned()), ("GET".to_owned(), "/v1/models".to_owned())]);
    fake.push(Answer::json(500, &json!({"error": {"message": "models unavailable"}})));
    assert_eq!(crate::hub::routes::models::models_json(&s, &home).await, json!({"ok": false, "default": "claude-haiku-5-5", "models": [], "error": format!("opencodex at {} unreachable", fake.url)}));
    let _ = std::fs::remove_dir_all(&home);
}
