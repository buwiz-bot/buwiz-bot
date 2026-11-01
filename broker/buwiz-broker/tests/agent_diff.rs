//! The tool loop's differential test (plan 5.4): the same `/chat` turns run
//! by Node's agent.mjs and by the hub's tool loop, against one scripted fake
//! opencodex (server/test/fixtures/fake-opencodex.mjs, run by
//! tests/agent-diff.mjs), must send the same model requests (method, path,
//! the headers opencodex reads, the body: system prompt, tools, messages
//! with tool results) and leave the same job phases and turns. The hub runs
//! in this process (no port); nsh, `secret` and the Mac tools are the test
//! PATH stubs. Needs `node` on PATH.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::BodyExt;
use hyper::{HeaderMap, Method};
use buwiz_broker::hub::ctx::HubCtx;
use buwiz_broker::hub::traits::Clock;
use buwiz_broker::hub::types::HubRequest;
use buwiz_broker::hub::Hub;
use serde_json::{json, Value};

const CODE: &str = "import { Text } from \"nsh\";\n\nexport default function R() {\n  return <Text>hi</Text>;\n}\n";

fn server_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../server")
}

struct Driver {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Driver {
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

async fn call(hub: &Hub, method: Method, url: &str, body: &Value) -> Value {
    let mut headers = HeaderMap::new();
    headers.insert("content-type", "application/json".parse().unwrap());
    let req = HubRequest { method, url: url.to_owned(), headers, body: Bytes::from(body.to_string()), params: Vec::new() };
    let res = hub.answer(req).await;
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

fn temp(label: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("rox-agent-diff-{label}-{}-{}", std::process::id(), buwiz_broker::hub::store::now_ms()));
    std::fs::create_dir_all(d.join("widgets")).unwrap();
    d
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn agent_diff_node_and_rust_send_the_same_model_requests() {
    let scenarios = json!({
        "scripts": {
            "think-tools": [
                {"content": [{"type": "thinking", "thinking": "Let me think.", "signature": "sig-1"}, {"type": "tool_use", "id": "toolu_a", "name": "react", "input": {"emoji": "👍"}, "chunks": 4}, {"type": "tool_use", "id": "toolu_b", "name": "view_code", "input": {"name": "R99"}}], "stop_reason": "tool_use"},
                {"content": [{"type": "text", "text": "Done reacting."}], "stop_reason": "end_turn"}
            ],
            "gen": [
                {"content": [{"type": "tool_use", "id": "toolu_generate_app", "name": "generate_app", "input": {"title": "Tiny Tip", "desc": "Splits a bill", "layout": {"width": 320, "height": 200}, "slug": "tiny-tip", "description": "Splits a restaurant bill.", "code": CODE}, "chunks": 8}], "stop_reason": "tool_use"},
                {"content": [{"type": "text", "text": "Built it."}], "stop_reason": "end_turn"}
            ],
            "gen-bad": [
                {"content": [{"type": "tool_use", "id": "toolu_bad", "name": "generate_app", "input": {"title": "Bad", "code": format!("{CODE}// nsh-stub:fail\n")}}], "stop_reason": "tool_use"},
                {"content": [{"type": "text", "text": "Gave up."}], "stop_reason": "end_turn"}
            ],
            "rounds": [
                {"content": [{"type": "tool_use", "id": "toolu_react", "name": "react", "input": {"emoji": "🔁"}}], "stop_reason": "tool_use", "times": 7},
                {"content": [{"type": "text", "text": "Finally."}], "stop_reason": "end_turn"}
            ],
            "shell": [
                {"content": [{"type": "tool_use", "id": "toolu_sh", "name": "shell_run", "input": {"command": "printf hi; exit 2", "cwd": "/"}}, {"type": "tool_use", "id": "toolu_x", "name": "no_such_tool", "input": {}}], "stop_reason": "tool_use"},
                {"content": [{"type": "text", "text": "Ran."}], "stop_reason": "end_turn"}
            ]
        },
        "runs": [
            {"prompt": "[[script:text]] hello there"},
            {"prompt": "[[script:think-tools]] react please"},
            {"prompt": "[[script:note-then-tool]] check it"},
            {"prompt": "[[script:pause]] look it up"},
            {"prompt": "[[script:rounds]] keep going"},
            {"prompt": "[[script:gen]] build a tip calculator"},
            {"prompt": "[[script:gen-bad]] build it"},
            {"prompt": "[[script:shell]] run it"},
            {"prompt": "[[script:error-429]] hi"},
            {"prompt": "[[script:error-500]] hi"},
            {"prompt": "[[script:text]] again"}
        ]
    });
    let node_data = temp("node");
    let mut child = Command::new("node")
        .arg(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/agent-diff.mjs"))
        .args([node_data.display().to_string(), scenarios.to_string()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("node");
    let stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    stdout.read_line(&mut line).unwrap();
    let node: Value = serde_json::from_str(&line).unwrap_or_else(|e| panic!("the driver said {line:?}: {e}"));
    let mut driver = Driver { child, stdin, stdout };

    // this process is the Rust broker: the same environment Node ran in
    let bin = server_dir().join("test/fixtures/bin");
    let rust_data = temp("rust");
    for k in [
        "MOCK",
        "ANTHROPIC_BASE_URL",
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "GEN_PROVIDER",
        "GEN_BASE_URL",
        "GEN_MODEL",
        "GEN_MAX_TOKENS",
        "OPENROUTER_API_KEY",
        "OPENAI_API_KEY",
        "GEMINI_API_KEY",
        "LINEAR_API_KEY",
    ] {
        std::env::remove_var(k);
    }
    std::env::set_var("OPENCODEX_URL", node["url"].as_str().unwrap());
    std::env::set_var("PATH", format!("{}:{}", bin.display(), std::env::var("PATH").unwrap_or_default()));
    std::env::set_var("NSH", bin.join("nsh"));
    std::env::set_var("BUWIZ_STUB_LOG", rust_data.join("stubs.log"));
    let ctx = HubCtx::new(rust_data.clone(), Clock::system());
    let hub = Hub::with(ctx, Vec::new(), |routes, ctx| {
        buwiz_broker::hub::routes::chat::install(routes, ctx);
        buwiz_broker::hub::pipeline::install(ctx);
        buwiz_broker::hub::model::install(routes, ctx);
        buwiz_broker::hub::agent::install(routes, ctx);
    });
    let hub = Arc::new(hub);

    for (i, run) in scenarios["runs"].as_array().unwrap().iter().enumerate() {
        let thread = call(&hub, Method::POST, "/thread/new", &json!({})).await["id"].clone();
        let job = call(&hub, Method::POST, "/chat", &json!({"prompt": run["prompt"], "thread": thread})).await["id"].as_u64().unwrap();
        let mut st = Value::Null;
        for _ in 0..600 {
            st = call(&hub, Method::GET, &format!("/status?id={job}"), &Value::Null).await;
            if st["done"] == true {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let want = &node["results"][i];
        let got = driver.take();
        let prompt = run["prompt"].as_str().unwrap();
        assert_eq!(got.as_array().map(Vec::len), want["calls"].as_array().map(Vec::len), "{prompt}: the number of model calls (the job: {st}, Rust sent {got})");
        for (k, (g, w)) in got.as_array().unwrap().iter().zip(want["calls"].as_array().unwrap()).enumerate() {
            assert_eq!(g, w, "{prompt}: model call {k}");
        }
        let phases: Vec<Value> = st["events"].as_array().unwrap().iter().map(|e| e["phase"].clone()).collect();
        assert_eq!(json!(phases), want["phases"], "{prompt}: the job's phases");
        assert_eq!(st["detail"], want["detail"], "{prompt}: the job's detail");
        let thread = thread.as_str().unwrap().to_owned();
        let turns: Vec<Value> = hub.ctx().store.read(|s| s.thread(&thread).unwrap().turns.iter().map(|u| json!([u.role, u.text, u.label, u.name, u.v, u.reaction])).collect());
        assert_eq!(json!(turns), want["turns"], "{prompt}: the thread's turns");
    }
}
