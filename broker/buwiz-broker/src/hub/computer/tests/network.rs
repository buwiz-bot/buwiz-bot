//! A computer's browser network against the mock provider, and the tunnel's client against a box's
//! WebSocket on loopback that speaks box-egress-v1. Nothing leaves this Mac: every destination the
//! client is asked for is refused by the rules or resolves to loopback, which the address policy
//! refuses.

use std::sync::atomic::Ordering;
use std::time::Duration;

use buwiz_computer::mock::BEARER;
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

use super::super::network::{Change, LIMIT_BROWSER};
use super::*;

fn rules_in_box(b: &Bed) -> Value {
    b.mock.egress_rules.lock().unwrap().clone().unwrap_or(Value::Null)
}

fn staged(b: &Bed, id: &str) -> Option<Value> {
    let text = std::fs::read(Computers::dir(&b.ctx, id).join("start/rules.json")).ok()?;
    serde_json::from_slice(&text).ok()
}

#[tokio::test]
async fn a_new_local_computer_is_made_with_the_proxy_and_the_tunnel_and_starts_with_its_rules() {
    let b = bed("net-new");
    let id = awake(&b, &b.ann.0).await;
    let m = b.computers.machine(&b.ctx, &id).unwrap();
    let egress = m.egress.clone().expect("made with the proxy");
    assert!(egress.tunnel && egress.bearer.as_ref().is_some_and(|s| s.expose() == BEARER));
    // staged before the start, and put in force after it: every site, dialled by the box
    let doc = staged(&b, &id).expect("the rules are staged for the next start");
    assert_eq!((doc["exit"].as_str(), doc["mode"].as_str()), (Some("direct"), Some("all")));
    assert_eq!(rules_in_box(&b)["exit"], "direct");
    let text = std::fs::read_to_string(Computers::dir(&b.ctx, &id).join("machine.json")).unwrap();
    assert!(!staged(&b, &id).unwrap().to_string().contains(BEARER), "the rules hold no secret");
    assert!(text.contains(BEARER), "the bearer is kept beside the other secrets, in the 0600 record");
    assert_eq!(m.scrub(&format!("x {BEARER} y")), "x [hidden] y");
}

#[tokio::test]
async fn a_block_is_in_force_at_once_for_every_bot_at_the_computer_and_says_what_it_covers() {
    let b = bed("net-block");
    let id = awake(&b, &b.ann.0).await;
    b.computers.attach(&b.ctx, &b.bea.0, &id).await.unwrap();
    let out = b.computers.network_change(&b.ctx, &b.ann.0, &Change::Block("facebook.com".into()), false).await;
    assert_eq!(out["ok"], true, "{out}");
    assert!(out["message"].as_str().unwrap().contains("It's in force now."), "{out}");
    assert_eq!(rules_in_box(&b)["block"], json!(["facebook.com"]));
    assert_eq!(staged(&b, &id).unwrap()["block"], json!(["facebook.com"]), "and the next start reads it");
    // the same computer, so Bea's view is the same rules
    let s = b.computers.network_status(&b.ctx, &b.bea.0).await;
    assert_eq!(s["block"], json!(["facebook.com"]));
    assert_eq!(s["inForce"], true, "{s}");
    let notes = s["notes"].to_string();
    assert!(notes.contains("every Bot on it (Ann, Bea)") && notes.contains(LIMIT_BROWSER) && notes.contains("http://"), "{notes}");
    // a site twice is said, and nothing is added
    let again = b.computers.network_change(&b.ctx, &b.bea.0, &Change::Block("facebook.com".into()), false).await;
    assert!(again["message"].as_str().unwrap().starts_with("facebook.com is already on the block list."));
    let off = b.computers.network_change(&b.ctx, &b.ann.0, &Change::Unblock("facebook.com".into()), false).await;
    assert_eq!(off["ok"], true);
    assert_eq!(rules_in_box(&b)["block"], json!([]));
}

#[tokio::test]
async fn listed_mode_with_nothing_allowed_says_the_browser_reaches_no_site() {
    let b = bed("net-listed");
    awake(&b, &b.ann.0).await;
    let out = b.computers.network_change(&b.ctx, &b.ann.0, &Change::Mode(buwiz_computer::box_egress::Mode::Listed), false).await;
    assert!(out["message"].as_str().unwrap().contains("none is allowed yet"), "{out}");
    assert_eq!(rules_in_box(&b)["mode"], "listed");
    assert!(b.computers.network_status(&b.ctx, &b.ann.0).await["notes"].to_string().contains("the browser reaches no site"));
}

#[tokio::test]
async fn a_computer_made_before_the_proxy_is_refused_in_a_sentence_and_nothing_is_stored_or_sent() {
    let b = bed("net-old");
    let id = awake(&b, &b.ann.0).await;
    let old = Machine { egress: None, ..b.computers.machine(&b.ctx, &id).unwrap() };
    Computers::save_machine(&b.ctx, &id, &old).unwrap();
    b.computers.machines.lock().unwrap().insert(id.clone(), old);
    *b.mock.egress_rules.lock().unwrap() = None;
    for change in [Change::Block("x.example".into()), Change::Tunnel(true), Change::Mode(buwiz_computer::box_egress::Mode::Listed)] {
        let out = b.computers.network_change(&b.ctx, &b.ann.0, &change, false).await;
        assert_eq!(out["ok"], false);
        assert!(out["message"].as_str().unwrap().starts_with("This computer was made before browsing rules"), "{out}");
    }
    assert!(rules_in_box(&b).is_null(), "nothing was sent");
    assert!(super::super::network::Settings::load(&b.ctx, &id).unwrap() == Default::default(), "nothing was stored");
    let s = b.computers.network_status(&b.ctx, &b.ann.0).await;
    assert_eq!(s["supported"], false);
    assert!(s["why"].as_str().unwrap().contains("make the computer again"));
    assert!(!b.mock.calls().iter().any(|c| c == "rebuild"), "nothing is remade by itself");
}

#[tokio::test]
async fn an_image_without_the_proxy_is_never_shown_as_in_force() {
    let b = bed("net-image");
    awake(&b, &b.ann.0).await;
    b.mock.no_proxy_image.store(true, Ordering::SeqCst);
    let out = b.computers.network_change(&b.ctx, &b.ann.0, &Change::Block("x.example".into()), false).await;
    assert!(out["message"].as_str().unwrap().contains("isn't in force yet"), "{out}");
    let s = b.computers.network_status(&b.ctx, &b.ann.0).await;
    assert_eq!(s["inForce"], false);
    assert!(s["notes"].to_string().contains("no browser proxy"));
}

#[tokio::test]
async fn the_network_rule_is_refused_as_not_built_and_changes_nothing() {
    let b = bed("net-rule");
    let id = awake(&b, &b.ann.0).await;
    let out = b.computers.network_change(&b.ctx, &b.ann.0, &Change::Rule, false).await;
    assert_eq!(out["ok"], false);
    assert!(out["message"].as_str().unwrap().contains("isn't built yet"));
    assert_eq!(super::super::network::Settings::load(&b.ctx, &id).unwrap().version, 0);
}

#[tokio::test]
async fn unreadable_settings_send_no_rules_and_a_wake_puts_no_site_in_force() {
    let b = bed("net-unread");
    let id = awake(&b, &b.ann.0).await;
    b.ctx.db.with(|c| c.execute("INSERT OR REPLACE INTO computer_network(computer_id, tunnel, mode, allow, block, version) VALUES(?,0,'everything','[]','[]',3)", [&id])).unwrap();
    b.computers.sleep(&b.ctx, &id, Sleep::Light).await.unwrap();
    awake(&b, &b.ann.0).await;
    assert!(staged(&b, &id).is_none(), "nothing staged: the box starts with no rules, and refuses every site");
    let r = rules_in_box(&b);
    assert_eq!((r["mode"].as_str(), r["allow"].clone()), (Some("listed"), json!([])), "the resumed box is told no site: {r}");
    assert_eq!(b.computers.network_status(&b.ctx, &b.ann.0).await["ok"], false);
}

#[tokio::test]
async fn with_all_every_computer_is_changed_and_each_says_how_it_went() {
    let b = bed("net-all");
    awake(&b, &b.ann.0).await;
    let out = b.computers.network_change(&b.ctx, &b.ann.0, &Change::Block("ads.example".into()), true).await;
    let results = out["results"].as_array().unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["bots"], json!(["Ann"]));
    assert_eq!(results[0]["ok"], true);
}

/// A box's tunnel WebSocket on loopback: checks the bearer, says hello, then asks the client to
/// open each of `asks` and reports what came back for each id.
async fn fake_box(asks: Vec<(&'static str, u16)>) -> (String, tokio::sync::mpsc::UnboundedReceiver<String>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let tx = tx.clone();
            let asks = asks.clone();
            tokio::spawn(async move {
                let bearer_ok = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
                let seen = bearer_ok.clone();
                let Ok(mut ws) = tokio_tungstenite::accept_hdr_async(stream, move |req: &tokio_tungstenite::tungstenite::handshake::server::Request, resp| {
                    let ok = req.headers().get("authorization").and_then(|v| v.to_str().ok()) == Some(&format!("Bearer {BEARER}"));
                    seen.store(ok, Ordering::SeqCst);
                    // as the box does: the protocol the client asked for
                    let mut resp: tokio_tungstenite::tungstenite::handshake::server::Response = resp;
                    resp.headers_mut().insert("sec-websocket-protocol", "box-egress-v1".parse().unwrap());
                    Ok(resp)
                })
                .await
                else {
                    return;
                };
                let _ = tx.send(format!("attached bearer={}", bearer_ok.load(Ordering::SeqCst)));
                let _ = ws.send(Message::Text(r#"{"v":1,"type":"hello","role":"server"}"#.into())).await;
                for (id, (host, port)) in asks.iter().enumerate() {
                    let open = json!({"v": 1, "type": "open", "id": id + 1, "host": host, "port": port});
                    let _ = ws.send(Message::Text(open.to_string().into())).await;
                }
                while let Some(Ok(msg)) = ws.next().await {
                    if let Message::Text(t) = msg {
                        let v: Value = serde_json::from_str(&t).unwrap_or(Value::Null);
                        if v["type"] != "hello" {
                            let _ = tx.send(format!("{} {} {}", v["id"], v["type"].as_str().unwrap_or_default(), v["message"].as_str().unwrap_or_default()));
                        }
                    }
                }
                let _ = tx.send("closed".to_owned());
            });
        }
    });
    (url, rx)
}

async fn heard(rx: &mut tokio::sync::mpsc::UnboundedReceiver<String>) -> String {
    tokio::time::timeout(Duration::from_secs(10), rx.recv()).await.expect("the fake box heard nothing").unwrap_or_default()
}

#[tokio::test]
async fn the_tunnel_attaches_with_the_bearer_judges_each_stream_again_and_stops_with_its_computer() {
    let b = bed("net-tunnel");
    let id = awake(&b, &b.ann.0).await;
    b.computers.network_change(&b.ctx, &b.ann.0, &Change::Block("blocked.example".into()), false).await;
    // asked for a blocked name, then one the rules allow that resolves to loopback
    let (url, mut rx) = fake_box(vec![("www.blocked.example", 443), ("localhost", 443)]).await;
    *b.mock.tunnel_at.lock().unwrap() = Some(url);
    let out = b.computers.network_change(&b.ctx, &b.ann.0, &Change::Tunnel(true), false).await;
    assert_eq!(out["ok"], true, "{out}");
    assert_eq!(rules_in_box(&b)["exit"], "tunnel");
    let mut all = Vec::new();
    let mut said = Vec::new();
    while said.len() < 2 {
        let h = heard(&mut rx).await;
        all.push(h.clone());
        if !h.starts_with("attached") {
            said.push(h);
        }
    }
    assert!(all.iter().filter(|h| h.starts_with("attached")).all(|h| h == "attached bearer=true") && all[0] == "attached bearer=true", "{all:?}");
    said.sort();
    assert_eq!(said[0], "1 error blocked by this computer's rules");
    assert_eq!(said[1], "2 error destination refused", "an allowed name that resolves to this Mac's loopback is never dialled");
    let mut attached = false;
    for _ in 0..200 {
        if b.computers.network_status(&b.ctx, &b.ann.0).await["client"]["state"] == "attached" {
            attached = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(attached);
    let s = b.computers.network_status(&b.ctx, &b.ann.0).await;
    assert!(!s.to_string().contains(BEARER), "no answer carries the bearer: {s}");
    // asleep, the client goes
    b.computers.sleep(&b.ctx, &id, Sleep::Light).await.unwrap();
    assert_eq!(heard(&mut rx).await, "closed");
    assert_eq!(b.computers.tunnels.status(&id)["state"], "off");
    // awake again, it comes back; switched off, it goes for good
    awake(&b, &b.ann.0).await;
    assert_eq!(heard(&mut rx).await, "attached bearer=true");
    let _ = heard(&mut rx).await;
    let _ = heard(&mut rx).await;
    b.computers.network_change(&b.ctx, &b.ann.0, &Change::Tunnel(false), false).await;
    assert_eq!(heard(&mut rx).await, "closed");
    assert_eq!(rules_in_box(&b)["exit"], "direct");
}

#[tokio::test]
async fn the_tunnel_on_with_no_client_attached_says_the_browser_reaches_no_site() {
    let b = bed("net-noclient");
    awake(&b, &b.ann.0).await;
    // nothing listens there
    let free = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    *b.mock.tunnel_at.lock().unwrap() = Some(format!("ws://{}", free.local_addr().unwrap()));
    drop(free);
    b.computers.network_change(&b.ctx, &b.ann.0, &Change::Tunnel(true), false).await;
    let s = b.computers.network_status(&b.ctx, &b.ann.0).await;
    assert_eq!(s["tunnel"], true);
    assert_ne!(s["client"]["state"], "attached");
    assert!(s["notes"].to_string().contains("can't reach any site until it is"), "{s}");
}

#[tokio::test]
async fn two_changes_at_once_both_survive_and_the_box_ends_on_the_saved_set() {
    let b = bed("net-race");
    let id = awake(&b, &b.ann.0).await;
    let (one, two) = (Change::Block("a.example".into()), Change::Allow("b.example".into()));
    let (x, y) = tokio::join!(b.computers.network_change(&b.ctx, &b.ann.0, &one, false), b.computers.network_change(&b.ctx, &b.ann.0, &two, true));
    assert_eq!((x["ok"].clone(), y["ok"].clone()), (json!(true), json!(true)), "{x} {y}");
    let saved = super::super::network::Settings::load(&b.ctx, &id).unwrap();
    assert_eq!((saved.block.clone(), saved.allow.clone()), (vec!["a.example".to_owned()], vec!["b.example".to_owned()]));
    let r = rules_in_box(&b);
    assert_eq!((r["block"].clone(), r["allow"].clone()), (json!(["a.example"]), json!(["b.example"])));
    assert_eq!(r["version"], json!(saved.version), "the set pushed last is the set saved");
}

#[tokio::test]
async fn a_box_that_doesnt_take_its_rules_after_a_wake_gets_them_on_the_sweep() {
    let b = bed("net-behind");
    let id = awake(&b, &b.ann.0).await;
    b.computers.network_change(&b.ctx, &b.ann.0, &Change::Block("a.example".into()), false).await;
    b.computers.sleep(&b.ctx, &id, Sleep::Light).await.unwrap();
    *b.mock.egress_rules.lock().unwrap() = None;
    *b.mock.egress_problem.lock().unwrap() = Some(Problem::Guest { status: 503, message: "not answering".into() });
    awake(&b, &b.ann.0).await;
    assert!(rules_in_box(&b).is_null());
    assert!(b.computers.network_status(&b.ctx, &b.ann.0).await["notes"].to_string().contains("isn't using these rules yet"));
    // still refusing: the sweep tries, and it's still behind
    b.computers.sleep_the_idle(&b.ctx).await;
    assert!(rules_in_box(&b).is_null());
    *b.mock.egress_problem.lock().unwrap() = None;
    b.computers.sleep_the_idle(&b.ctx).await;
    assert_eq!(rules_in_box(&b)["block"], json!(["a.example"]));
    assert_eq!(b.computers.network_status(&b.ctx, &b.ann.0).await["inForce"], true);
}

#[tokio::test]
async fn a_box_that_refuses_the_bearer_is_told_by_the_clients_own_outcome() {
    let b = bed("net-401");
    awake(&b, &b.ann.0).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    *b.mock.tunnel_at.lock().unwrap() = Some(format!("ws://{}", listener.local_addr().unwrap()));
    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        while let Ok((mut s, _)) = listener.accept().await {
            let mut buf = [0u8; 2048];
            let _ = s.read(&mut buf).await;
            // a body that never says 401: only the status does
            let _ = s.write_all(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 2\r\n\r\nno").await;
        }
    });
    b.computers.network_change(&b.ctx, &b.ann.0, &Change::Tunnel(true), false).await;
    let mut state = Value::Null;
    for _ in 0..300 {
        state = b.computers.network_status(&b.ctx, &b.ann.0).await["client"]["state"].clone();
        if state == "refused" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(state, "refused");
    assert!(b.computers.network_status(&b.ctx, &b.ann.0).await["notes"].to_string().contains("refused this Mac's key"));
}
