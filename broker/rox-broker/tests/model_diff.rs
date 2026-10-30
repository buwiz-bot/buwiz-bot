//! The model client's differential test (plan 5.3): the same calls made by
//! Node's model.mjs and by the hub's model client, against one scripted
//! fake opencodex (server/test/fixtures/fake-opencodex.mjs, run by
//! tests/model-diff.mjs), must send the same requests (method, path, the
//! headers opencodex reads, the body) and come back with the same result
//! (the module, the classification, the reply, the message, the error
//! text). Needs `node` on PATH, as the hub's other integration tests do.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::Arc;

use rox_broker::hub::model::{self, images, sse, Ask, ModelClient, Settings};
use rox_broker::hub::traits::{LocalCommand, LocalOutput, LocalTools, ModelApi};
use rox_broker::hub::types::{BoxFuture, ModelChoice, TurnCtx};
use serde_json::{json, Value};

const CODE: &str = "import { Text } from \"nsh\";\n\nexport default function R() {\n  return <Text>hi</Text>;\n}";

fn server_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../server")
}

struct Driver {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Driver {
    fn start(mode: &str, data: &Path, scenarios: &Value) -> (Self, Value) {
        let mut child = Command::new("node")
            .arg(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/model-diff.mjs"))
            .args([mode, &data.display().to_string(), &scenarios.to_string()])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("node");
        let stdin = child.stdin.take().unwrap();
        let mut stdout = BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        stdout.read_line(&mut line).unwrap();
        let first: Value = serde_json::from_str(&line).unwrap_or_else(|e| panic!("the driver said {line:?}: {e}"));
        (Self { child, stdin, stdout }, first)
    }

    fn take(&mut self) -> Value {
        self.stdin.write_all(b"take\n").unwrap();
        self.stdin.flush().unwrap();
        let mut line = String::new();
        self.stdout.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    }
}

impl Drop for Driver {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The test PATH stubs (server/test/fixtures/bin) run for real.
struct Stubs;

impl LocalTools for Stubs {
    fn run(&self, cmd: LocalCommand) -> BoxFuture<'static, Result<LocalOutput, String>> {
        Box::pin(async move {
            let bin = server_dir().join("test/fixtures/bin");
            let program = if bin.join(&cmd.program).exists() { bin.join(&cmd.program).display().to_string() } else { cmd.program.clone() };
            let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap_or_default());
            let out = tokio::process::Command::new(program).args(&cmd.args).env("PATH", path).output().await.map_err(|e| e.to_string())?;
            Ok(LocalOutput { code: out.status.code(), stdout: out.stdout, stderr: out.stderr })
        })
    }
}

fn ctx_of(v: &Value) -> TurnCtx {
    let model = (!v.is_null()).then(|| ModelChoice {
        model: v["model"].as_str().unwrap_or("").to_owned(),
        effort: v["effort"].as_str().unwrap_or("").to_owned(),
        fast: v["fast"].as_bool().unwrap_or(false),
        name: v["name"].as_str().map(str::to_owned),
    });
    TurnCtx { model, ..Default::default() }
}

fn settings(mode: &str, url: &str) -> Settings {
    let url = url.to_owned();
    let openai = mode == "openai";
    Settings::from(move |k| match k {
        "OPENCODEX_URL" => Some(url.clone()),
        "ANTHROPIC_BASE_URL" if !openai => Some(format!("{url}/direct")),
        "GEN_PROVIDER" if openai => Some("openai".to_owned()),
        "GEN_BASE_URL" if openai => Some(format!("{url}/v1")),
        "OPENROUTER_API_KEY" if openai => Some("test-key".to_owned()),
        _ => None,
    })
}

async fn run(s: &Settings, api: &dyn ModelApi, data: &Path, sc: &Value) -> Value {
    let turn = ctx_of(&sc["ctx"]);
    let str_of = |k: &str| sc[k].as_str();
    match sc["kind"].as_str().unwrap() {
        "ask" => {
            let ask = Ask { prompt: str_of("prompt").unwrap(), prev_error: str_of("prevError"), prev_code: str_of("prevCode"), name: str_of("name").unwrap(), linear: sc["linear"] == json!(true) };
            match model::ask_model(s, api, &turn, &ask, |_| unreachable!(), || {}).await {
                Ok(m) => json!({ "caption": m.caption, "code": m.code }),
                Err(e) => json!({ "error": e }),
            }
        }
        "classify" => json!(model::classify_intent(s, api, &turn, str_of("prompt").unwrap()).await),
        "chat" => json!(model::chat_reply(s, api, &turn, sc["turns"].as_array().unwrap(), sc["prompt"].clone()).await),
        "create" => api.create(&turn, sc["body"].clone()).await.unwrap_or_else(|e| json!({ "error": e })),
        "stream" => match api.stream(&turn, sc["body"].clone()).await {
            Ok(rx) => sse::collect(rx, |_, _| {}).await.unwrap_or_else(|e| json!({ "error": e })),
            Err(e) => json!({ "error": e }),
        },
        "imageModels" => json!(images::image_models(&s.opencodex).await),
        "generate" => {
            let opts =
                images::ImageOpts { size: sc["opts"]["size"].as_str().unwrap_or("1024x1024").to_owned(), background: sc["opts"]["background"].as_str().map(str::to_owned), ..Default::default() };
            match images::generate_image(s, api, &turn, &Stubs, str_of("prompt").unwrap(), &data.join("rust-gen.png"), &opts).await {
                Ok(b) => json!(b),
                Err(e) => json!({ "error": e }),
            }
        }
        "edit" => match images::edit_image(s, &Stubs, &data.join("src.png"), str_of("prompt").unwrap(), &data.join("rust-edit.png"), str_of("size").unwrap()).await {
            Ok(b) => json!(b),
            Err(e) => json!({ "error": e }),
        },
        k => panic!("no scenario kind {k}"),
    }
}

/// Runs the scenarios on both sides and compares them one by one.
async fn differ(mode: &str, scenarios: Value) {
    let data = std::env::temp_dir().join(format!("rox-model-diff-{mode}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&data);
    std::fs::create_dir_all(&data).unwrap();
    let (mut driver, node) = Driver::start(mode, &data, &scenarios);
    let url = node["url"].as_str().unwrap().to_owned();
    let s = settings(mode, &url);
    let api = ModelClient::new(Arc::new(s.clone()));
    for (i, sc) in scenarios["list"].as_array().unwrap().iter().enumerate() {
        let want = &node["results"][i];
        let result = run(&s, &api, &data, sc).await;
        let requests = driver.take();
        assert_eq!(requests, want["requests"], "scenario {i} ({sc}): the requests differ");
        assert_eq!(result, want["result"], "scenario {i} ({sc}): the results differ");
    }
    let _ = std::fs::remove_dir_all(&data);
}

fn say(text: &str) -> Value {
    json!({ "content": [{ "type": "text", "text": text }], "stop_reason": "end_turn" })
}

#[tokio::test]
async fn model_diff_anthropic_requests_and_results_match_node() {
    let module = format!("Fixed it\n```tsx\n{CODE}\n```");
    let body = json!({ "model": "claude-haiku-5-5", "max_tokens": 20000, "system": "sys", "tools": [{ "name": "react", "input_schema": { "type": "object" } }], "messages": [{ "role": "user", "content": "[[script:tool-then-text]] go" }] });
    let scenarios = json!({
        "scripts": {
            "mod": [say(&module)],
            "trunc": [{ "content": [{ "type": "text", "text": "```tsx\nexport default function R() { return <" }], "stop_reason": "max_tokens" }],
            "cut-tool": [{ "content": [{ "type": "tool_use", "id": "toolu_x", "name": "generate_app", "input": { "title": "Card", "code": CODE }, "chunks": 5 }], "stop_reason": "max_tokens" }],
            "bad": [{ "status": 400, "error": "bad request" }],
        },
        "list": [
            { "kind": "ask", "ctx": null, "prompt": "[[script:mod]] a card", "name": "R1" },
            { "kind": "ask", "ctx": { "model": "gpt-5.5", "effort": "low", "fast": true }, "prompt": "[[script:mod]] again", "prevError": "error TS2304: Cannot find name 'nope'.\n", "prevCode": "bad code", "name": "R2" },
            { "kind": "ask", "ctx": { "name": "Ada" }, "prompt": "[[script:trunc]] cut", "prevError": "module truncated at the token limit", "name": "R3" },
            { "kind": "ask", "ctx": { "model": "", "effort": "max", "fast": false, "name": "Bo" }, "prompt": "plain", "prevError": "model did not return a module", "prevCode": "", "name": "R4" },
            { "kind": "ask", "ctx": null, "prompt": "[[script:error-429]] retry me", "name": "R5" },
            { "kind": "ask", "ctx": null, "prompt": "[[script:bad]] refused", "name": "R6" },
            { "kind": "classify", "ctx": { "model": "claude-x", "effort": "high", "fast": false, "name": "Ada" }, "prompt": "[[script:mod]] make a timer" },
            { "kind": "classify", "ctx": null, "prompt": "hello" },
            { "kind": "chat", "ctx": null, "prompt": "how are you?", "turns": [
                { "role": "user", "text": "hi" }, { "role": "ai", "text": "Hello!" }, { "role": "notice", "text": "ran" },
                { "role": "ai", "name": "R1", "text": "" }, { "role": "ai" }, { "role": "user" }
            ] },
            { "kind": "chat", "ctx": { "model": "m2", "effort": "", "fast": false }, "prompt": "[[script:error-500]] fail", "turns": [] },
            { "kind": "create", "ctx": { "model": "gpt-5.5", "effort": "medium", "fast": false }, "body": body },
            { "kind": "create", "ctx": null, "body": { "model": "claude-haiku-5-5", "max_tokens": 30000, "messages": [{ "role": "user", "content": "too long" }] } },
            { "kind": "create", "ctx": null, "body": { "model": "claude-haiku-5-5", "max_tokens": 100, "messages": [{ "role": "user", "content": "[[script:error-500]] x" }] } },
            { "kind": "stream", "ctx": null, "body": body },
            { "kind": "stream", "ctx": null, "body": { "model": "m", "max_tokens": 100, "messages": [{ "role": "user", "content": "[[script:thinking]] x" }] } },
            { "kind": "stream", "ctx": { "model": "gpt-5.5", "effort": "", "fast": true }, "body": { "model": "m", "max_tokens": 100, "messages": [{ "role": "user", "content": "[[script:pause]] x" }] } },
            { "kind": "stream", "ctx": null, "body": { "model": "m", "max_tokens": 100, "messages": [{ "role": "user", "content": "[[script:cut-tool]] x" }] } },
            { "kind": "stream", "ctx": null, "body": { "model": "m", "max_tokens": 100, "messages": [{ "role": "user", "content": "[[script:die]] x" }] } },
            { "kind": "stream", "ctx": null, "body": { "model": "m", "max_tokens": 100, "messages": [{ "role": "user", "content": "[[script:error-500]] x" }] } },
            { "kind": "imageModels" },
            { "kind": "generate", "ctx": null, "prompt": "a red fox", "opts": { "size": "512x512", "background": "transparent" } },
            { "kind": "edit", "prompt": "make it blue", "size": "1024x1024" },
            // last: a Linear key stays set once read
            { "kind": "ask", "ctx": null, "prompt": "[[script:mod]] linear board", "name": "R9", "linear": true },
        ],
    });
    differ("anthropic", scenarios).await;
}

#[tokio::test]
async fn model_diff_openai_requests_and_results_match_node() {
    let scenarios = json!({
        "scripts": {
            "oai-ui": [{ "classify": "UI", "content": [{ "type": "text", "text": "```tsx\nexport default function R() { return <" }], "stop_reason": "max_tokens" }, say(&format!("A card\n```tsx\n{CODE}\n```"))],
            "oai-chat": [{ "classify": "CHAT", "content": [{ "type": "text", "text": " Hi from the fake. " }], "stop_reason": "end_turn" }],
            "oai-dead": [{ "status": 500, "error": "down" }],
        },
        "list": [
            { "kind": "classify", "ctx": null, "prompt": "[[script:oai-chat]] hello" },
            { "kind": "classify", "ctx": null, "prompt": "[[script:oai-ui]] a card" },
            { "kind": "chat", "ctx": null, "prompt": "[[script:oai-chat]] hello", "turns": [{ "role": "user", "text": "earlier" }, { "role": "ai", "name": "R2" }] },
            { "kind": "chat", "ctx": null, "prompt": "[[script:oai-chat]] look", "turns": [{ "role": "user", "text": [{ "type": "text", "text": "this" }, { "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": "AAAA" } }] }] },
            { "kind": "chat", "ctx": null, "prompt": "x", "turns": [{ "role": "user" }] },
            { "kind": "ask", "ctx": null, "prompt": "[[script:oai-ui]] a card", "name": "R1" },
            { "kind": "ask", "ctx": null, "prompt": "[[script:oai-ui]] a card", "prevError": "module truncated at the token limit", "name": "R1" },
            { "kind": "ask", "ctx": null, "prompt": "[[script:oai-dead]] another", "name": "R2" },
        ],
    });
    differ("openai", scenarios).await;
}
