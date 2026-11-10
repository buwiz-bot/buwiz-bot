//! `request_user_form` against the mock computer: the card, what's stripped from it, the fill's
//! rules on the Bot's own browser, the routes, and that a typed secret goes nowhere but the page.

use buwiz_computer::mock::MockPage;
use buwiz_computer::ActiveTab;

use super::together::{answer_of, open_req};
use super::*;
use crate::hub::tools::login::{self, Fill, LoginWait, Pressed};

/// A marker that stands for a password in these tests. It's fake, and must turn up only in what
/// the mock says was typed into the page.
const MARK: &str = "fake-secret-marker-9f3a";
const SITE: &str = "shop.example";
const PAGE: &str = "https://www.shop.example/login";

fn login_bed(label: &str) -> Bed {
    let b = bed(label);
    b.mock.set_page(Some(MockPage::sign_in(PAGE)));
    b
}

fn card_input() -> Value {
    json!({"title": "Sign in to Shop", "domain": SITE, "samePage": true, "submit": true, "fields": [
        {"id": "email", "label": "Email", "type": "email", "value": "model-put-this@example.com"},
        {"id": "password", "label": "Password", "type": "password", "value": "model-smuggled-pw"},
    ]})
}

fn values() -> Value {
    json!({"email": "kid@example.com", "password": MARK})
}

/// The pending login card in `chat`.
async fn login_card(b: &Bed, chat: &str) -> Value {
    for _ in 0..600 {
        let found = b.ctx.store.read(|s| s.thread(chat).and_then(|t| t.turns.iter().filter_map(|u| u.notice.clone()).rfind(|n| n["kind"] == login::KIND && n["state"] == "pending")));
        if let Some(n) = found {
            return n;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("no login card");
}

/// Calls the tool in a task, and finds its card.
async fn asked(b: &Bed, input: Value) -> (Value, tokio::task::JoinHandle<Out>) {
    let r = Arc::new(run(b, &b.ann.1, None));
    let call = {
        let r = r.clone();
        tokio::spawn(async move { super::call(&r, login::NAME, input).await })
    };
    (login_card(b, &b.ann.1).await, call)
}

async fn submit(b: &Bed, card: &Value, values: Value, saved: bool) -> (u16, Value) {
    login::host_submit(&b.ctx, &json!({"chat": card["chat"], "id": card["id"], "values": values, "savedLogin": saved})).await
}

/// Everything the broker kept or said of the chat: every turn, notices and all.
fn transcript(b: &Bed) -> String {
    b.ctx.store.read(|s| s.thread(&b.ann.1).map(|t| format!("{:?}", t.turns)).unwrap_or_default())
}

fn acts(b: &Bed) -> Vec<String> {
    b.mock.calls().into_iter().filter(|c| c.contains(" act ")).collect()
}

#[test]
fn the_card_keeps_no_value_the_model_gave_and_says_where_it_types() {
    let form = login::sanitize(&card_input()).unwrap();
    let card = form.to_json().to_string();
    for smuggled in ["model-put-this@example.com", "model-smuggled-pw", "\"value\""] {
        assert!(!card.contains(smuggled), "{smuggled} in {card}");
    }
    assert_eq!(form.site, SITE);
    assert!(form.fields[1].is_secret() && form.fields[1].wants_a_password_box());
    let otp = login::sanitize(&json!({"title": "Code", "domain": SITE, "fields": [{"id": "c", "label": "Code", "type": "otp"}]})).unwrap();
    assert!(otp.fields[0].is_secret() && !otp.fields[0].wants_a_password_box(), "a code may go in a plain box");

    assert_eq!(login::site_of("https://WWW.Shop.Example:8443/login?x=1").as_deref(), Some(SITE));
    assert_eq!(login::site_of("127.0.0.1").as_deref(), Some("127.0.0.1"));
    assert_eq!(login::site_of("not a host"), None);
    assert!(login::on_site("https://accounts.shop.example/", SITE));
    assert!(!login::on_site("https://shop.example.evil.test/", SITE));
    assert!(!login::on_site("about:blank", SITE));
    assert!(!login::on_site("data:text/html,https://shop.example/", SITE));

    let dup = json!({"title": "x", "domain": SITE, "fields": [{"id": "a", "label": "A"}, {"id": "a", "label": "B"}]});
    assert!(login::sanitize(&dup).unwrap_err().contains("unique"));
    let nowhere = json!({"title": "x", "fields": [{"id": "a", "label": "A"}]});
    assert!(login::sanitize(&nowhere).unwrap_err().contains("domain"));
    let captcha = json!({"title": "x", "domain": SITE, "challengeKind": "captcha", "fields": [{"id": "a", "label": "A"}]});
    assert!(login::sanitize(&captcha).unwrap_err().contains("computer_ask_person"));
}

#[test]
fn return_is_pressed_only_by_the_reference_rules() {
    let form = |same: bool, submit: bool, n: usize, kind: Option<&str>| {
        let fields: Vec<Value> = (0..n).map(|i| json!({"id": format!("f{i}"), "label": format!("F{i}"), "type": if i == n - 1 { "password" } else { "email" }})).collect();
        login::sanitize(&json!({"title": "t", "domain": SITE, "samePage": same, "submit": submit, "challengeKind": kind, "fields": fields})).unwrap()
    };
    assert!(!login::should_press_return(&form(true, true, 2, None), 0), "nothing typed");
    assert!(login::should_press_return(&form(false, true, 2, None), 1), "asked for");
    assert!(!login::should_press_return(&form(false, false, 2, None), 1), "a stepped form typed its first field only");
    assert!(login::should_press_return(&form(false, false, 1, None), 1), "one field");
    assert!(login::should_press_return(&form(true, false, 2, None), 2), "one page, typed in full, with a password");
    assert!(!login::should_press_return(&form(true, false, 2, None), 1), "one page, half typed");
    assert!(login::should_press_return(&form(true, false, 2, Some("password")), 1), "a password challenge that typed one field");
}

#[tokio::test]
async fn the_tool_is_offered_with_the_computer_and_gone_with_its_switch_or_its_person() {
    let b = login_bed("login-offered");
    awake(&b, &b.ann.0).await;
    let r = run(&b, &b.ann.1, None);
    assert!(offered(&r).await.contains(&login::NAME.to_owned()));
    assert!(tools::prompt(&r).await.contains("request_user_form"), "the model is told to prefer it for a sign-in");

    // a routine's turn has nobody to fill the card
    let routine = run(&b, &b.ann.1, Some("rt_1"));
    assert!(!offered(&routine).await.contains(&login::NAME.to_owned()));

    set(&b, &b.ann.0, tools::KEY, "off");
    let off = run(&b, &b.ann.1, None);
    assert!(!offered(&off).await.contains(&login::NAME.to_owned()));
    let out = super::call(&off, login::NAME, card_input()).await;
    assert_eq!(out.is_error, Some(true), "{}", words(&out));
    assert!(b.ctx.store.read(|s| s.thread(&b.ann.1).unwrap().turns.iter().all(|t| t.notice.as_ref().is_none_or(|n| n["kind"] != login::KIND))), "no card with the switch off");
}

#[tokio::test]
async fn a_one_page_login_is_typed_by_ref_into_its_own_fields_and_the_model_reads_filled_and_no_secret() {
    let b = login_bed("login-fill");
    awake(&b, &b.ann.0).await;
    b.mock.calls.lock().unwrap().clear();
    login::logged().clear();
    let (card, call) = asked(&b, card_input()).await;
    assert!(!card.to_string().contains("model-smuggled-pw") && !card.to_string().contains("model-put-this"), "{card}");
    assert_eq!(card["form"]["domain"], SITE);

    let (status, reply) = submit(&b, &card, values(), false).await;
    assert_eq!(status, 200, "{reply}");
    let out = call.await.unwrap();
    let said = words(&out);
    assert_eq!(out.is_error, None, "{said}");
    assert!(said.contains("It was filled into the page.") && said.contains("do not claim login succeeded"), "{said}");
    assert!(said.contains("Return was pressed") && said.contains("Shared fields: email=kid@example.com."), "{said}");
    assert!(!said.to_lowercase().contains("signed in"), "never told it signed in: {said}");
    assert!(!said.contains("password="), "a secret field isn't named: {said}");

    // each value went into its own field, the password into the sensitive one
    assert_eq!(b.mock.typed(), [("Email".to_owned(), "kid@example.com".to_owned()), ("Password".to_owned(), MARK.to_owned())]);
    let acts = acts(&b);
    assert_eq!(acts.len(), 3, "two types and an Enter: {acts:?}");
    assert!(acts[2].contains("act key @e3"), "Enter in the password field: {acts:?}");
    for a in &acts {
        assert!(a.contains(&format!("allow=[\"{SITE}\",\"*.{SITE}\"]")), "every act carries the card's site: {a}");
    }
    assert!(b.mock.calls().iter().filter(|c| c.contains(" snap ")).all(|c| c.contains(SITE)), "every snap carries it too");
    assert!(b.mock.calls().iter().any(|c| c.starts_with("active_tab")), "the front tab was read");
    assert!(!b.mock.calls().iter().any(|c| c.starts_with("screenshot")), "no picture of a fill");

    let settled = login::find(&b.ctx, &b.ann.1, card["id"].as_str().unwrap()).unwrap();
    assert_eq!(settled["state"], "submitted");
    assert_eq!(settled["outcomes"], json!([{"id": "email", "filled": true}, {"id": "password", "filled": true}]));
    assert_eq!(reply["offerSave"], json!({"origin": SITE, "username": "kid@example.com"}), "the save offer names where and who, never the password");

    // the marker is in no tool result, transcript row, notice, reply, call log or log line
    for (what, text) in [("result", said.clone()), ("transcript", transcript(&b)), ("reply", reply.to_string()), ("calls", b.mock.calls().join("\n")), ("log", login::logged().join("\n"))] {
        assert!(!text.contains(MARK), "the secret is in the {what}");
    }
    assert!(login::logged().iter().any(|l| l.contains("field password") && l.contains(&format!("len={}", MARK.len()))), "{:?}", login::logged());
}

#[tokio::test]
async fn a_password_goes_only_into_a_field_the_box_calls_sensitive_and_a_plain_value_only_into_one_it_doesnt() {
    let b = login_bed("login-sensitive");
    awake(&b, &b.ann.0).await;
    // a "Password" box the box doesn't call sensitive: no password goes in it
    let mut page = MockPage::sign_in(PAGE);
    page.elements[2].as_object_mut().unwrap().remove("sensitive");
    b.mock.set_page(Some(page));
    let (card, call) = asked(&b, card_input()).await;
    let (status, reply) = submit(&b, &card, values(), false).await;
    assert_eq!(status, 200, "{reply}");
    let said = words(&call.await.unwrap());
    assert!(said.contains("could not be filled") && said.contains("isn't one"), "{said}");
    assert_eq!(b.mock.typed(), [("Email".to_owned(), "kid@example.com".to_owned())], "the password wasn't typed");
    assert_eq!(login::find(&b.ctx, &b.ann.1, card["id"].as_str().unwrap()).unwrap()["state"], "fill_failed");

    // an email field whose only match is sensitive: the email doesn't go there
    let mut page = MockPage::sign_in(PAGE);
    page.elements.retain(|e| e["label"] != "Email" && e["label"] != "Search");
    b.mock.set_page(Some(page));
    b.mock.typed.lock().unwrap().clear();
    let (card, call) = asked(&b, json!({"title": "t", "domain": SITE, "fields": [{"id": "email", "label": "Email", "type": "email"}]})).await;
    submit(&b, &card, json!({"email": "kid@example.com"}), false).await;
    assert!(words(&call.await.unwrap()).contains("could not be filled"));
    assert!(b.mock.typed().is_empty(), "{:?}", b.mock.typed());
}

#[tokio::test]
async fn a_page_off_the_cards_site_types_nothing_and_the_card_stays_open() {
    let b = login_bed("login-wrong-page");
    awake(&b, &b.ann.0).await;
    let (card, call) = asked(&b, card_input()).await;

    // the box's extension says another page is in front
    *b.mock.front.lock().unwrap() = Some(ActiveTab { url: Some("https://evil.test/login".into()), focus: None, seq: Some(3) });
    let (status, reply) = submit(&b, &card, values(), false).await;
    assert_eq!((status, reply["code"].as_str()), (403, Some("wrong-page")), "{reply}");
    assert!(reply["message"].as_str().unwrap().contains("evil.test, not shop.example"), "{reply}");
    // nothing in front at all
    *b.mock.front.lock().unwrap() = Some(ActiveTab::default());
    assert_eq!(submit(&b, &card, values(), false).await.1["code"], "wrong-page");
    // the extension agrees, but box-exec's own look is elsewhere
    *b.mock.front.lock().unwrap() = None;
    b.mock.set_page(Some(MockPage::sign_in("https://evil.test/login")));
    *b.mock.front.lock().unwrap() = Some(ActiveTab { url: Some(PAGE.into()), focus: None, seq: Some(4) });
    assert_eq!(submit(&b, &card, values(), false).await.1["code"], "wrong-page");
    assert!(b.mock.typed().is_empty(), "nothing typed off the site");
    assert!(!call.is_finished(), "the card is still open");
    assert_eq!(login::find(&b.ctx, &b.ann.1, card["id"].as_str().unwrap()).unwrap()["state"], "pending");

    // on the right page it goes
    *b.mock.front.lock().unwrap() = None;
    b.mock.set_page(Some(MockPage::sign_in(PAGE)));
    assert_eq!(submit(&b, &card, values(), false).await.0, 200);
    assert!(words(&call.await.unwrap()).contains("filled into the page"));
}

#[tokio::test]
async fn a_page_that_moves_mid_fill_stops_it_before_the_next_value() {
    let b = login_bed("login-moves");
    awake(&b, &b.ann.0).await;
    let mut page = MockPage::sign_in(PAGE);
    page.moves_to = Some("https://evil.test/".into());
    b.mock.set_page(Some(page));
    let (card, call) = asked(&b, card_input()).await;
    assert_eq!(submit(&b, &card, values(), false).await.0, 200);
    let said = words(&call.await.unwrap());
    assert!(said.contains("could not be filled") && said.contains("evil.test"), "{said}");
    assert_eq!(b.mock.typed(), [("Email".to_owned(), "kid@example.com".to_owned())]);
    assert_eq!(login::find(&b.ctx, &b.ann.1, card["id"].as_str().unwrap()).unwrap()["outcomes"], json!([{"id": "email", "filled": true}, {"id": "password", "filled": false}]));
}

#[tokio::test]
async fn a_stepped_card_types_its_first_field_and_presses_nothing() {
    let b = login_bed("login-stepped");
    awake(&b, &b.ann.0).await;
    let mut input = card_input();
    input["samePage"] = json!(false);
    input["submit"] = json!(false);
    let (card, call) = asked(&b, input).await;
    submit(&b, &card, values(), false).await;
    let said = words(&call.await.unwrap());
    assert!(said.contains("Only the FIRST field was typed") && said.contains("Nothing was pressed"), "{said}");
    assert_eq!(b.mock.typed().len(), 1);
    assert!(!acts(&b).iter().any(|a| a.contains("act key")), "{:?}", acts(&b));
}

#[tokio::test]
async fn a_press_the_box_wants_confirmed_is_left_for_the_person() {
    let b = login_bed("login-confirm");
    awake(&b, &b.ann.0).await;
    let mut page = MockPage::sign_in(PAGE);
    page.confirm_on_enter = Some("Place order".into());
    b.mock.set_page(Some(page));
    let (card, call) = asked(&b, card_input()).await;
    let (_, reply) = submit(&b, &card, values(), false).await;
    let said = words(&call.await.unwrap());
    assert!(said.contains("asks a person to confirm \"Place order\"") && said.contains("don't press it for them"), "{said}");
    assert!(reply["notice"]["note"].as_str().unwrap().contains("press it yourself"), "{reply}");
    let keys: Vec<String> = acts(&b).into_iter().filter(|a| a.contains("act key")).collect();
    assert_eq!(keys.len(), 1, "no second press with a confirm: {keys:?}");
}

#[tokio::test]
async fn the_boxs_refusals_are_plain_sentences_with_nothing_of_the_box_in_them() {
    let b = login_bed("login-errors");
    awake(&b, &b.ann.0).await;
    // a stale ref is snapped again, once
    *b.mock.browser_refusal.lock().unwrap() = Some(("type".into(), 409, "stale_ref".into()));
    let (card, call) = asked(&b, card_input()).await;
    submit(&b, &card, values(), false).await;
    assert!(words(&call.await.unwrap()).contains("filled into the page"), "a stale ref is tried again from a fresh snap");

    for (code, status, words_) in [("pressed_other", 409, "landed on another part"), ("browser_timeout", 504, "didn't answer in time"), ("domain_not_allowed", 403, "isn't on shop.example"), ("action_refused", 422, "wouldn't take the typing"), ("busy", 429, "was busy")] {
        b.mock.typed.lock().unwrap().clear();
        *b.mock.browser_refusal.lock().unwrap() = Some(("type".into(), status, code.into()));
        let (card, call) = asked(&b, card_input()).await;
        submit(&b, &card, values(), false).await;
        let said = words(&call.await.unwrap());
        assert!(said.contains(words_) && !said.contains("mock "), "{code}: {said}");
        let settled = login::find(&b.ctx, &b.ann.1, card["id"].as_str().unwrap()).unwrap();
        assert_eq!(settled["state"], "fill_failed", "{code}");
        // a type that went out and wasn't answered may have landed: both are told so
        let maybe = matches!(code, "pressed_other" | "browser_timeout");
        assert_eq!(said.contains(login::MAYBE_TYPED), maybe, "{code}: {said}");
        assert_eq!(settled["uncertain"], maybe, "{code}: {settled}");
        assert_eq!(settled["note"].as_str().unwrap().contains("may have been typed"), maybe, "{code}");
    }

    // no browser on a pipe
    b.mock.set_page(None);
    let (card, call) = asked(&b, card_input()).await;
    let (status, reply) = submit(&b, &card, values(), false).await;
    assert_eq!(status, 403, "no page in front: the card stays open {reply}");
    b.mock.set_page(Some(MockPage::sign_in(PAGE)));
    *b.mock.browser_refusal.lock().unwrap() = Some(("snap".into(), 503, "browser_unavailable".into()));
    submit(&b, &card, values(), false).await;
    assert!(words(&call.await.unwrap()).contains("No browser is open"));
}

#[tokio::test]
async fn an_image_from_before_the_browser_layer_is_told_to_update_and_nothing_is_typed() {
    let b = login_bed("login-old-image");
    awake(&b, &b.ann.0).await;
    *b.mock.caps.lock().unwrap() = json!({"chrome": {"enabled": false, "ready": false}});
    let (card, call) = asked(&b, card_input()).await;
    let (_, reply) = submit(&b, &card, values(), false).await;
    assert_eq!(reply["notice"]["state"], "fill_failed", "{reply}");
    assert_eq!(reply["notice"]["note"], login::TOO_OLD);
    assert!(words(&call.await.unwrap()).contains("too old"));
    assert!(b.mock.typed().is_empty() && acts(&b).is_empty());
}

#[tokio::test]
async fn a_saved_login_is_refused_on_a_shared_computer_and_a_typed_one_still_goes() {
    let b = login_bed("login-shared");
    let id = awake(&b, &b.ann.0).await;
    b.computers.attach(&b.ctx, &b.bea.0, &id).await.unwrap();
    let (card, call) = asked(&b, card_input()).await;
    let (_, pre) = login::host_saved(&b.ctx, &json!({"chat": card["chat"], "id": card["id"]}));
    assert_eq!((pre["usable"].as_bool(), pre["code"].as_str()), (Some(false), Some(login::SHARED_COMPUTER)), "{pre}");
    let (status, reply) = submit(&b, &card, values(), true).await;
    assert_eq!((status, reply["code"].as_str()), (403, Some(login::SHARED_COMPUTER)), "{reply}");
    assert_eq!(reply["message"], login::SHARED_COMPUTER_MESSAGE);
    assert!(b.mock.typed().is_empty());
    assert!(!call.is_finished(), "the card stays open to type it by hand");
    assert_eq!(submit(&b, &card, values(), false).await.0, 200, "typed by hand it goes");
    call.await.unwrap();

    // on a computer of its own, a saved login goes, and no save is offered for it
    let c = login_bed("login-own");
    awake(&c, &c.ann.0).await;
    let (card, call) = asked(&c, card_input()).await;
    assert_eq!(login::host_saved(&c.ctx, &json!({"chat": card["chat"], "id": card["id"]})).1["usable"], true);
    let (status, reply) = submit(&c, &card, values(), true).await;
    assert_eq!(status, 200, "{reply}");
    assert!(reply.get("offerSave").is_none(), "{reply}");
    call.await.unwrap();
}

#[tokio::test]
async fn dismissed_timed_out_and_answered_cards_end_the_wait_and_type_nothing() {
    let b = login_bed("login-dismiss");
    awake(&b, &b.ann.0).await;
    let (card, call) = asked(&b, card_input()).await;
    let (status, _) = login::host_dismiss(&b.ctx, &json!({"chat": card["chat"], "id": card["id"], "mode": "dismissed"})).await;
    assert_eq!(status, 200);
    let said = words(&call.await.unwrap());
    assert!(said.contains("dismissed") && said.contains("do not raise the same card again"), "{said}");
    let (status, again) = submit(&b, &card, values(), false).await;
    assert_eq!((status, again["code"].as_str()), (409, Some("stale")), "an answered card types nothing");
    assert!(b.mock.typed().is_empty());

    let b = bed_tweaked("login-timeout", Computers::new, |ctx| ctx.ext.insert(LoginWait(Duration::from_millis(50))));
    b.mock.set_page(Some(MockPage::sign_in(PAGE)));
    awake(&b, &b.ann.0).await;
    let (card, call) = asked(&b, card_input()).await;
    assert_eq!(words(&call.await.unwrap()), login::TIMED_OUT);
    assert_eq!(login::find(&b.ctx, &b.ann.1, card["id"].as_str().unwrap()).unwrap()["state"], "timed_out");
    assert_eq!(submit(&b, &card, values(), false).await.0, 409);
}

#[tokio::test]
async fn open_the_screen_is_the_hand_over_and_the_model_hears_when_it_comes_back() {
    let b = login_bed("login-escalate");
    awake(&b, &b.ann.0).await;
    let (card, call) = asked(&b, card_input()).await;
    let (status, reply) = login::host_dismiss(&b.ctx, &json!({"chat": card["chat"], "id": card["id"], "mode": "escalated"})).await;
    assert_eq!((status, reply["notice"]["state"].as_str()), (200, Some("escalated")), "{reply}");
    for _ in 0..600 {
        if b.computers.status(&b.ctx, &b.ann.0).await.get("takeover").is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(b.computers.status(&b.ctx, &b.ann.0).await["takeover"]["reason"].as_str().unwrap().contains("Sign in to Shop"));
    assert!(b.computers.hand_back(&b.ann.0));
    let out = call.await.unwrap();
    assert_eq!(words(&out), login::HANDED_BACK, "text only, no picture");
}

#[tokio::test]
async fn the_routes_take_the_host_token_and_answer_401_without_it() {
    let b = login_bed("login-auth");
    awake(&b, &b.ann.0).await;
    let bare = |path: &str| HubRequest { method: hyper::Method::POST, url: path.to_owned(), headers: hyper::HeaderMap::new(), body: Bytes::from(json!({"chat": "c", "id": "i", "values": values()}).to_string()), params: vec![b.ann.0.clone()] };
    for (path, res) in [
        ("/v1/host/computer/login/submit", login_submit(b.ctx.clone(), bare("/v1/host/computer/login/submit")).await),
        ("/v1/host/computer/login/dismiss", login_dismiss(b.ctx.clone(), bare("/v1/host/computer/login/dismiss")).await),
        ("/v1/host/computer/login/saved", login_saved(b.ctx.clone(), bare("/v1/host/computer/login/saved")).await),
        ("/v1/host/computer/bot/b_ann/session", session(b.ctx.clone(), bare("/v1/host/computer/bot/b_ann/session")).await),
    ] {
        let (status, body) = answer_of(res).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{path}");
        assert!(!body.to_string().contains(MARK), "{path}");
    }
    let mut req = open_req(&b.ann.0, json!({"chat": "c", "id": "missing", "values": {}}));
    req.url = "/v1/host/computer/login/submit".into();
    assert_eq!(answer_of(login_submit(b.ctx.clone(), req).await).await.0, StatusCode::NOT_FOUND, "with the token it's read");
}

#[tokio::test]
async fn a_person_saves_and_loads_a_session_of_the_bots_own_screen() {
    let b = login_bed("login-session");
    awake(&b, &b.ann.0).await;
    let (status, v) = answer_of(session(b.ctx.clone(), open_req(&b.ann.0, json!({"op": "save", "name": "shop"}))).await).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert_eq!((v["op"].as_str(), v["cookies"].as_u64(), v["origins"].as_u64()), (Some("save"), Some(2), Some(1)), "counts, never values: {v}");
    assert!(b.mock.calls().iter().any(|c| c == "browser :2 state save shop"), "{:?}", b.mock.calls());
    let (status, _) = answer_of(session(b.ctx.clone(), open_req(&b.ann.0, json!({"op": "load", "name": "shop"}))).await).await;
    assert_eq!(status, StatusCode::OK);
    for bad in [json!({"op": "save", "name": "../x"}), json!({"op": "drop", "name": "x"}), json!({"op": "save", "name": ""})] {
        assert_eq!(answer_of(session(b.ctx.clone(), open_req(&b.ann.0, bad.clone())).await).await.0, StatusCode::BAD_REQUEST, "{bad}");
    }
    *b.mock.caps.lock().unwrap() = json!({});
    let (status, v) = answer_of(session(b.ctx.clone(), open_req(&b.ann.0, json!({"op": "save", "name": "shop"}))).await).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(v["message"], login::TOO_OLD);
}

#[test]
fn a_log_line_takes_a_length_and_never_a_value() {
    let form = login::sanitize(&card_input()).unwrap();
    let line = login::field_log_line(&form.fields[1], false, MARK.len(), "typed");
    assert!(line.contains("field password") && line.contains("secret=true") && line.contains(&format!("len={}", MARK.len())), "{line}");
    // a plain field's line says secret when it went into a sensitive element
    assert!(login::field_log_line(&form.fields[0], true, 3, "typed").contains("secret=true"));
    let fill = Fill::Filled { outcomes: vec![], pressed: Pressed::Yes };
    assert!(!login::result_text(&form, &fill, &Default::default()).contains("password="));
}

#[test]
fn secrecy_is_not_the_models_to_turn_off() {
    let form = login::sanitize(&json!({"title": "t", "domain": SITE, "samePage": true, "fields": [
        {"id": "f1", "label": "Password", "type": "text", "secret": false},
        {"id": "user_pin", "label": "Your number", "type": "text"},
        {"id": "f3", "label": "Verification code", "type": "text"},
        {"id": "f4", "label": "Card number", "type": "tel"},
        {"id": "f5", "label": "Nickname", "type": "current-password"},
        {"id": "email", "label": "Email", "type": "email"},
    ]})).unwrap();
    let f = |id: &str| form.fields.iter().find(|x| x.id == id).unwrap().clone();
    for id in ["f1", "user_pin", "f4", "f5"] {
        assert!(f(id).is_secret() && f(id).wants_a_password_box(), "{id}");
    }
    assert_eq!(f("f3").kind, "otp");
    assert!(f("f3").is_secret() && !f("f3").wants_a_password_box());
    assert!(!f("email").is_secret());
    assert_eq!(form.to_json()["fields"][0]["secret"], true, "masked on the card");
    for (word, hint) in [("Security answer", login::Hint::Secret), ("CVC", login::Hint::Secret), ("one-time code", login::Hint::Code), ("2FA", login::Hint::Code), ("Email address", login::Hint::Plain), ("Spinach", login::Hint::Plain)] {
        assert_eq!(login::hint_of(word), hint, "{word}");
    }

    // only a plain field that landed in a plain element is shared
    let values: std::collections::BTreeMap<String, buwiz_computer::Secret> = [("email", "kid@example.com"), ("f1", MARK)].into_iter().map(|(k, v)| (k.to_owned(), buwiz_computer::Secret::new(v))).collect();
    let outcome = |id: &str, sensitive: bool| login::Outcome { id: id.into(), filled: true, sensitive };
    assert_eq!(login::shared_values(&form, &values, &[outcome("email", false), outcome("f1", false)]).len(), 1);
    assert!(login::shared_values(&form, &values, &[outcome("email", true)]).is_empty(), "a value that went into a sensitive element is a secret");
}

#[tokio::test]
async fn a_password_the_model_called_text_is_masked_typed_into_the_password_box_and_never_returned() {
    let b = login_bed("login-disguised");
    awake(&b, &b.ann.0).await;
    login::logged().clear();
    let (card, call) = asked(&b, json!({"title": "Sign in", "domain": SITE, "samePage": true, "fields": [
        {"id": "email", "label": "Email", "type": "email"},
        {"id": "secret_thing", "label": "Password", "type": "text"},
    ]})).await;
    assert_eq!(card["form"]["fields"][1]["secret"], true, "{card}");
    let (status, reply) = submit(&b, &card, json!({"email": "kid@example.com", "secret_thing": MARK}), false).await;
    assert_eq!(status, 200, "{reply}");
    let said = words(&call.await.unwrap());
    assert!(said.contains("Shared fields: email=kid@example.com.") && !said.contains("secret_thing"), "{said}");
    assert_eq!(b.mock.typed()[1], ("Password".to_owned(), MARK.to_owned()), "into the sensitive box");
    for (what, text) in [("result", said), ("transcript", transcript(&b)), ("reply", reply.to_string()), ("calls", b.mock.calls().join("\n")), ("log", login::logged().join("\n"))] {
        assert!(!text.contains(MARK), "the secret is in the {what}");
    }
    assert!(login::logged().iter().any(|l| l.contains("field secret_thing") && l.contains("secret=true")));
}

#[tokio::test]
async fn a_hand_over_that_fails_leaves_the_card_open_and_says_why() {
    let b = login_bed("login-escalate-fails");
    awake(&b, &b.ann.0).await;
    let (card, call) = asked(&b, card_input()).await;
    *b.mock.stray_problem.lock().unwrap() = Some(Problem::Failed("the desktop couldn't be cleared".into()));
    let (status, reply) = login::host_dismiss(&b.ctx, &json!({"chat": card["chat"], "id": card["id"], "mode": "escalated"})).await;
    assert_eq!((status, reply["code"].as_str()), (409, Some("not_handed_over")), "{reply}");
    assert!(reply["message"].as_str().unwrap().starts_with("The screen wasn't handed over"), "{reply}");
    assert_eq!(login::find(&b.ctx, &b.ann.1, card["id"].as_str().unwrap()).unwrap()["state"], "pending");
    assert!(b.computers.status(&b.ctx, &b.ann.0).await.get("takeover").is_none(), "the screen isn't the person's");
    assert!(!call.is_finished());
    // fixed, it goes
    *b.mock.stray_problem.lock().unwrap() = None;
    let (status, _) = login::host_dismiss(&b.ctx, &json!({"chat": card["chat"], "id": card["id"], "mode": "escalated"})).await;
    assert_eq!(status, 200);
    for _ in 0..600 {
        if b.computers.hand_back(&b.ann.0) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(words(&call.await.unwrap()), login::HANDED_BACK);
}

#[test]
fn the_site_is_the_domain_and_a_live_host_never_moves_it() {
    let card = |extra: Value| {
        let mut v = json!({"title": "t", "fields": [{"id": "a", "label": "Email"}]});
        v.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        login::sanitize(&v)
    };
    assert_eq!(card(json!({"domain": SITE, "liveHost": "evil.test"})).unwrap().site, SITE, "another site is ignored");
    assert_eq!(card(json!({"domain": SITE, "liveHost": "accounts.shop.example"})).unwrap().site, SITE);
    assert_eq!(card(json!({"domain": SITE, "liveHost": "%% not a host"})).unwrap().site, SITE, "junk doesn't void the domain");
    assert!(card(json!({"liveHost": SITE})).unwrap_err().contains("domain"), "the site is the domain's to say");
    let form = card(json!({"domain": SITE, "liveHost": "evil.test"})).unwrap();
    assert_eq!(form.to_json()["domain"], SITE);
    assert!(!form.to_json().to_string().contains("evil.test"), "the card names only the site");
}

#[tokio::test]
async fn a_live_host_on_another_site_types_nothing_there() {
    let b = login_bed("login-livehost");
    awake(&b, &b.ann.0).await;
    b.mock.set_page(Some(MockPage::sign_in("https://evil.test/login")));
    let mut input = card_input();
    input["liveHost"] = json!("evil.test");
    let (card, call) = asked(&b, input).await;
    assert_eq!(card["form"]["domain"], SITE);
    let (status, reply) = submit(&b, &card, values(), false).await;
    assert_eq!((status, reply["code"].as_str()), (403, Some("wrong-page")), "{reply}");
    assert!(b.mock.typed().is_empty());
    assert!(b.mock.calls().iter().filter(|c| c.contains(" snap ") || c.contains(" act ")).all(|c| !c.contains("evil.test")), "the allow-list is the card's site");
    call.abort();
}

#[tokio::test]
async fn a_submit_with_every_field_empty_types_nothing_and_says_so() {
    let b = login_bed("login-empty");
    awake(&b, &b.ann.0).await;
    let (card, call) = asked(&b, json!({"title": "Sign in to Shop", "domain": SITE, "samePage": true, "fields": [{"id": "email", "label": "Email"}, {"id": "password", "label": "Password", "type": "password"}]})).await;
    b.mock.calls.lock().unwrap().clear();
    let (status, reply) = submit(&b, &card, json!({"email": "", "password": ""}), false).await;
    assert_eq!(status, 200, "{reply}");
    let said = words(&call.await.unwrap());
    assert!(said.contains("with every field empty, so nothing was typed into the page") && !said.contains("filled into the page"), "{said}");
    assert_eq!(reply["notice"]["state"], "fill_failed");
    assert_eq!(reply["notice"]["note"], login::EMPTY_NOTE);
    assert!(acts(&b).is_empty() && b.mock.typed().is_empty());
}

#[tokio::test]
async fn the_prompt_names_the_card_and_the_hand_over_only_when_they_are_offered() {
    let b = login_bed("login-prompt");
    awake(&b, &b.ann.0).await;
    let r = run(&b, &b.ann.1, None);
    let said = tools::prompt(&r).await;
    assert!(said.contains(login::NAME) && said.contains(tools::ASK_PERSON), "{said}");
    set(&b, &b.ann.0, tools::ROUTINES_KEY, "on");
    let routine = run(&b, &b.ann.1, Some("rt_1"));
    let offered_now = offered(&routine).await;
    assert!(!offered_now.iter().any(|n| n == login::NAME || n == tools::ASK_PERSON), "{offered_now:?}");
    let said = tools::prompt(&routine).await;
    assert!(!said.is_empty() && !said.contains(login::NAME) && !said.contains(tools::ASK_PERSON), "{said}");
    assert!(said.contains("Nobody can answer a card"), "{said}");
}

#[tokio::test]
async fn nothing_reaches_the_box_before_a_person_answers_the_card() {
    let b = login_bed("login-before-submit");
    awake(&b, &b.ann.0).await;
    b.mock.calls.lock().unwrap().clear();
    let (card, call) = asked(&b, card_input()).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let touched: Vec<String> = b.mock.calls().into_iter().filter(|c| ["browser", "active_tab", "capabilities", "input", "screenshot", "open", "run"].iter().any(|w| c.starts_with(w))).collect();
    assert!(touched.is_empty(), "the box was used before a person answered: {touched:?}");
    assert!(b.mock.typed().is_empty());
    assert_eq!(card["form"]["domain"], SITE, "the card names the site");
    assert_eq!(card["form"]["submit"], true, "and says Return is pressed");
    login::host_dismiss(&b.ctx, &json!({"chat": card["chat"], "id": card["id"], "mode": "dismissed"})).await;
    call.await.unwrap();
    assert!(b.mock.calls().iter().all(|c| !c.starts_with("browser") && !c.starts_with("active_tab")), "nor after a dismiss");
}

#[test]
fn a_backslash_ends_the_host_as_a_browser_reads_it() {
    assert_eq!(login::site_of("https://evil.test\\@shop.example/").as_deref(), Some("evil.test"));
    assert!(!login::on_site("https://evil.test\\@shop.example/login", SITE));
    assert_eq!(login::sanitize(&json!({"title": "t", "domain": "https://evil.test\\@shop.example/", "fields": [{"id": "a", "label": "Email"}]})).unwrap().site, "evil.test", "the card names the host the browser would go to");
}
