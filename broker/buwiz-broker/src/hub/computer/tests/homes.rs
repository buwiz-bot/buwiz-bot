//! Homes: a person's keys, checked before they're kept, never shown or logged; the Mac's default
//! and a Bot's own choice, for its next computer. Against a fake cloud provider; nothing here
//! reaches a platform.

use std::sync::Mutex;

use async_trait::async_trait;
use buwiz_computer::cloud;
use buwiz_computer::{Machine, Reach, Secret, Shared, Sleep, Spec, Woke};

use super::*;
use crate::hub::computer::homes::{Factory, Made};

const GOOD: &str = "e2b_good_key_0123456789wxyz";
const OTHER: &str = "e2b_other_account_key_9999";

/// What the fake platform knows: the keys it takes, and whether it can see the computers made so far.
#[derive(Default)]
struct Platform {
    /// the settings each provider was made with, in order: the live one first
    made: Mutex<Vec<Shared<cloud::Settings>>>,
    created: Mutex<Vec<String>>,
    /// the first computer made is one every key can see
    first_shared: std::sync::atomic::AtomicBool,
    /// every computer is gone, whatever the key
    all_gone: std::sync::atomic::AtomicBool,
}

struct FakeCloud {
    kind: ProviderKind,
    settings: Shared<cloud::Settings>,
    platform: Arc<Platform>,
}

impl FakeCloud {
    fn key(&self) -> Option<String> {
        self.settings.get().key.map(|k| k.expose().to_owned())
    }
}

#[async_trait]
impl Provider for FakeCloud {
    fn kind(&self) -> ProviderKind {
        self.kind
    }

    async fn check(&self) -> buwiz_computer::Result<()> {
        self.key().map(drop).ok_or(Problem::Refused { code: "no_key", message: "no key".into() })
    }

    async fn verify(&self) -> buwiz_computer::Result<()> {
        match self.key().as_deref() {
            Some(GOOD) | Some(OTHER) => Ok(()),
            _ => Err(Problem::Refused { code: "bad_key", message: "E2B didn't take this API key.".into() }),
        }
    }

    async fn create(&self, spec: &Spec) -> buwiz_computer::Result<Machine> {
        self.check().await?;
        let first = self.platform.created.lock().unwrap().is_empty();
        self.platform.created.lock().unwrap().push(spec.id.clone());
        let shared = first && self.platform.first_shared.load(Ordering::SeqCst);
        Ok(Machine {
            provider: self.kind,
            reach: Reach::Sandbox { id: format!("sbx-{}{}", if shared { "shared-" } else { "" }, spec.id) },
            image: self.settings.get().image,
            token: Secret::new("tok-fake"),
            viewer_password: Secret::new("pw-fake"),
            dir: spec.dir.clone(),
            link: None,
            seen: None,
        })
    }

    /// The computers made with the good key's account are only seen with it.
    async fn power(&self, machine: &Machine) -> buwiz_computer::Result<Power> {
        let shared = matches!(&machine.reach, Reach::Sandbox { id } if id.contains("shared-"));
        Ok(if self.platform.all_gone.load(Ordering::SeqCst) || (self.key().as_deref() == Some(OTHER) && !shared) { Power::Absent } else { Power::Asleep })
    }

    async fn wake(&self, _machine: &Machine) -> buwiz_computer::Result<Woke> {
        Ok(Woke::Started)
    }

    async fn sleep(&self, _machine: &Machine, _depth: Sleep) -> buwiz_computer::Result<()> {
        Ok(())
    }

    async fn destroy(&self, _machine: &Machine) -> buwiz_computer::Result<()> {
        Ok(())
    }
}

fn factory(platform: Arc<Platform>) -> Factory {
    Arc::new(move |made: Made| -> Arc<dyn Provider> {
        match made {
            Made::Cloud(kind, settings) => {
                platform.made.lock().unwrap().push(settings.clone());
                Arc::new(FakeCloud { kind, settings, platform: platform.clone() })
            }
            // its keys are what the AWS check reads; this one takes any
            Made::Ec2(..) => Arc::new(FakeCloud { kind: ProviderKind::Ec2, settings: Shared::new(cloud::Settings::new(Some(Secret::new(GOOD)), None)), platform: platform.clone() }),
        }
    })
}

fn homes_bed(label: &str) -> (Bed, Arc<Platform>) {
    let platform = Arc::new(Platform::default());
    let p = platform.clone();
    let b = bed_with(label, move |mock| Computers::new(mock).with_factory(factory(p)));
    (b, platform)
}

fn values(pairs: &[(&str, &str)]) -> serde_json::Map<String, Value> {
    pairs.iter().map(|(k, v)| ((*k).to_owned(), json!(v))).collect()
}

fn provider_of<'a>(list: &'a Value, kind: &str) -> &'a Value {
    list["providers"].as_array().unwrap().iter().find(|p| p["kind"] == kind).unwrap()
}

fn field<'a>(provider: &'a Value, key: &str) -> &'a Value {
    provider["fields"].as_array().unwrap().iter().find(|f| f["key"] == key).unwrap()
}

/// What the broker's data dir holds, every file of it, as text.
fn everything_on_disk(ctx: &HubCtx) -> String {
    fn walk(dir: &std::path::Path, out: &mut String) {
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else if let Ok(b) = std::fs::read(&p) {
                out.push_str(&format!("\n== {}\n{}", p.display(), String::from_utf8_lossy(&b)));
            }
        }
    }
    let mut out = String::new();
    walk(&ctx.data, &mut out);
    out
}

async fn save_e2b(b: &Bed, key: &str) -> Result<Value, Problem> {
    b.computers.save_provider(&b.ctx, ProviderKind::E2b, &values(&[("api_key", key), ("template", "buwiz-computer")]), &[]).await
}

#[tokio::test]
async fn every_home_is_listed_and_docker_is_the_default_until_a_person_picks() {
    let (b, _) = homes_bed("list");
    let list = b.computers.providers_json(&b.ctx);
    let named: Vec<(&str, &str)> = list["providers"].as_array().unwrap().iter().map(|p| (p["kind"].as_str().unwrap(), p["label"].as_str().unwrap())).collect();
    assert_eq!(named, buwiz_protocol::homes::HOMES, "the app shows buwiz_protocol's list before the broker answers");
    assert!(provider_of(&list, "boat")["notes"].as_array().unwrap().iter().any(|n| n == "EU regions only today."), "boat.dev says it's EU only");
    assert_eq!(list["default"]["kind"], "docker");
    assert_eq!(provider_of(&list, "remote")["canDefault"], false);
    assert_eq!(provider_of(&list, "docker")["configured"], true);
    assert!(provider_of(&list, "ec2")["policy"].as_str().unwrap().contains("buwiz:managed"), "the Copy button has the limited policy");
    let launch = provider_of(&list, "ec2")["launch"].as_str().unwrap();
    assert!(launch.starts_with("https://console.aws.amazon.com/cloudformation/home?region=") && launch.contains("templateURL=https%3A%2F%2Fbuwiz-computer-templates.s3"), "{launch}");
}

#[tokio::test]
async fn a_key_that_fails_its_check_is_not_kept_anywhere() {
    let (b, _) = homes_bed("bad-key");
    let p = save_e2b(&b, "e2b_wrong_key_000000000000").await.unwrap_err();
    assert_eq!(p.code(), "bad_key");
    assert!(!everything_on_disk(&b.ctx).contains("e2b_wrong_key"), "a refused key isn't written");
    let list = b.computers.providers_json(&b.ctx);
    assert_eq!(field(provider_of(&list, "e2b"), "api_key")["set"], false);
}

#[tokio::test]
async fn a_good_key_goes_to_the_keystore_and_shows_only_as_set_and_its_last_4() {
    let (b, platform) = homes_bed("good-key");
    let shown = save_e2b(&b, GOOD).await.unwrap();
    let key = field(&shown, "api_key");
    assert_eq!((key["set"].clone(), key["last4"].clone()), (json!(true), json!("wxyz")));
    assert!(key.get("value").is_none());
    assert_eq!(field(&shown, "template")["value"], "buwiz-computer");
    assert!(!b.computers.providers_json(&b.ctx).to_string().contains(GOOD), "no route answers with the key");
    // in the keystore's file, and in no row of app.db
    let keys = keys(&b.ctx).unwrap().get_provider_cred("e2b").unwrap().unwrap();
    assert!(keys.contains(GOOD));
    let row: String = b.ctx.db.with(|c| c.query_row("SELECT config FROM computer_providers WHERE kind='e2b'", [], |r| r.get(0))).unwrap();
    assert!(!row.contains(GOOD), "{row}");
    // the provider that makes computers reads it at once
    let live = b.computers.homes.provider(&b.ctx, ProviderKind::E2b);
    live.check().await.unwrap();
    assert_eq!(platform.made.lock().unwrap().last().unwrap().get().key.unwrap().expose(), GOOD);
}

#[tokio::test]
async fn a_blank_secret_keeps_the_saved_one_and_a_new_one_reaches_the_live_provider() {
    let (b, platform) = homes_bed("blank");
    save_e2b(&b, GOOD).await.unwrap();
    let live = b.computers.homes.provider(&b.ctx, ProviderKind::E2b);
    b.computers.save_provider(&b.ctx, ProviderKind::E2b, &values(&[("api_key", ""), ("template", "tpl-2")]), &[]).await.unwrap();
    // made in order: the first save's check, the live provider, the second save's check
    let settings = platform.made.lock().unwrap()[1].get();
    assert_eq!((settings.key.unwrap().expose(), settings.image.as_deref()), (GOOD, Some("tpl-2")));
    // the same provider, so no connection was dropped for it
    assert!(Arc::ptr_eq(&live, &b.computers.homes.provider(&b.ctx, ProviderKind::E2b)));
}

#[tokio::test]
async fn the_model_never_reads_a_saved_key() {
    let (b, _) = homes_bed("scrub");
    save_e2b(&b, GOOD).await.unwrap();
    let machine = b.computers.homes.provider(&b.ctx, ProviderKind::E2b).create(&Spec { id: "cmp_00".into(), dir: b.ctx.data.join("x") }).await.unwrap();
    assert_eq!(machine.scrub(&format!("key {GOOD} here")), "key [hidden] here");
}

#[tokio::test]
async fn a_bots_choice_and_the_default_decide_where_its_next_computer_is_made() {
    let (b, platform) = homes_bed("where");
    let (ann, bea) = (b.ann.0.clone(), b.bea.0.clone());
    // nothing set up: E2B can't be picked
    assert_eq!(b.computers.set_default(&b.ctx, ProviderKind::E2b).unwrap_err().code(), "not_configured");
    assert_eq!(b.computers.set_bot_home(&b.ctx, &ann, Some(ProviderKind::Remote)).unwrap_err().code(), "cant_default");
    save_e2b(&b, GOOD).await.unwrap();
    b.computers.set_bot_home(&b.ctx, &ann, Some(ProviderKind::E2b)).unwrap();
    assert_eq!(b.computers.status(&b.ctx, &ann).await["home"], json!({"kind": "e2b", "label": "E2B", "source": "bot"}));
    assert_eq!(b.computers.status(&b.ctx, &bea).await["home"]["source"], "default");
    b.computers.click(&b.ctx, &ann).await.unwrap();
    b.computers.click(&b.ctx, &bea).await.unwrap();
    let kind_of = |bot: &str| Computers::kind_of(&b.ctx, &seated(&b.ctx, bot).unwrap().computer).unwrap().unwrap();
    assert_eq!((kind_of(&ann), kind_of(&bea)), (ProviderKind::E2b, ProviderKind::Docker));
    assert_eq!(platform.created.lock().unwrap().len(), 1);
    // a new default is for computers made from now on: Bea keeps hers
    b.computers.set_default(&b.ctx, ProviderKind::E2b).unwrap();
    assert_eq!(kind_of(&bea), ProviderKind::Docker);
    assert_eq!(b.computers.providers_json(&b.ctx)["default"], json!({"kind": "e2b", "label": "E2B", "source": "saved"}));
    // following the default again
    b.computers.set_bot_home(&b.ctx, &ann, None).unwrap();
    assert_eq!(b.computers.status(&b.ctx, &ann).await["home"]["source"], "default");
}

#[tokio::test]
async fn keys_with_computers_still_on_them_arent_removed_or_swapped_for_another_accounts() {
    let (b, _) = homes_bed("in-use");
    save_e2b(&b, GOOD).await.unwrap();
    b.computers.set_bot_home(&b.ctx, &b.ann.0, Some(ProviderKind::E2b)).unwrap();
    b.computers.click(&b.ctx, &b.ann.0).await.unwrap();
    for _ in 0..400 {
        if b.computers.status(&b.ctx, &b.ann.0).await["state"] != "waking" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let p = b.computers.remove_provider(&b.ctx, ProviderKind::E2b).await.unwrap_err();
    assert_eq!(p.code(), "in_use");
    assert!(p.sentence().contains("Ann") && p.sentence().contains("delete the Bot"), "{}", p.sentence());
    // a key that works, and can't see Ann's computer: not kept
    assert_eq!(save_e2b(&b, OTHER).await.unwrap_err().code(), "other_account");
    assert!(keys(&b.ctx).unwrap().get_provider_cred("e2b").unwrap().unwrap().contains(GOOD));
    // with the computer gone, the keys go
    b.computers.detach(&b.ctx, &b.ann.0).await.unwrap();
    // retired, it's still there until the sweep has its provider delete it: its keys are still needed
    assert_eq!(b.computers.remove_provider(&b.ctx, ProviderKind::E2b).await.unwrap_err().code(), "in_use");
    b.computers.sleep_the_idle(&b.ctx).await;
    b.computers.remove_provider(&b.ctx, ProviderKind::E2b).await.unwrap();
    assert_eq!(keys(&b.ctx).unwrap().get_provider_cred("e2b").unwrap(), None);
}

#[tokio::test]
async fn the_routes_answer_only_the_apps_host_token() {
    let (b, _) = homes_bed("routes");
    for (method, path) in [("GET", "/v1/host/computer/providers"), ("PUT", "/v1/host/computer/providers/e2b"), ("POST", "/v1/host/computer/providers/e2b/check"), ("PUT", "/v1/host/computer/providers/default")] {
        let req = HubRequest { method: method.parse().unwrap(), url: path.to_owned(), headers: hyper::HeaderMap::new(), body: Bytes::from_static(b"{}"), params: vec!["e2b".into()] };
        let res = match method {
            "GET" => providers(b.ctx.clone(), req).await,
            "POST" => check_provider(b.ctx.clone(), req).await,
            _ if path.ends_with("default") => set_default(b.ctx.clone(), req).await,
            _ => save_provider(b.ctx.clone(), req).await,
        };
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED, "{method} {path}");
    }
}

#[tokio::test]
async fn a_model_can_ask_for_a_setup_card_once_and_never_sees_a_key() {
    let (b, _) = homes_bed("setup-card");
    let r = run(&b, &b.ann.1, None);
    let said = words(&call(&r, tools::SETUP, json!({"reason": "to open the bank's site"})).await);
    assert!(said.contains("card is in the chat"), "{said}");
    assert!(words(&call(&r, tools::SETUP, json!({"reason": "again"})).await).contains("already in the chat"));
    let notice = b.ctx.store.read(|s| s.thread(&b.ann.1).unwrap().turns.iter().rev().find_map(|t| t.notice.clone())).unwrap();
    assert_eq!((notice["kind"].as_str(), notice["reason"].as_str()), (Some("computer_setup"), Some("to open the bank's site")));
    // a routine has nobody to ask
    let routine = run(&b, &b.ann.1, Some("rt_1"));
    assert!(words(&call(&routine, tools::SETUP, json!({"reason": "x"})).await).contains("routine can't"));
}

#[tokio::test]
async fn removing_a_homes_keys_takes_it_off_as_the_default_and_as_a_bots_choice() {
    let (b, _) = homes_bed("remove-choices");
    save_e2b(&b, GOOD).await.unwrap();
    b.computers.set_default(&b.ctx, ProviderKind::E2b).unwrap();
    b.computers.set_bot_home(&b.ctx, &b.ann.0, Some(ProviderKind::E2b)).unwrap();
    b.computers.remove_provider(&b.ctx, ProviderKind::E2b).await.unwrap();
    assert_eq!(b.computers.providers_json(&b.ctx)["default"]["kind"], "docker");
    assert_eq!(b.computers.status(&b.ctx, &b.ann.0).await["home"], json!({"kind": "docker", "label": "Docker on this Mac", "source": "default"}));
}

#[tokio::test]
async fn a_saved_aws_key_is_kept_by_blank_boxes_and_let_go_only_when_asked() {
    let (b, _) = homes_bed("aws-profile");
    let save = |pairs: &'static [(&'static str, &'static str)], clear: &'static [&'static str]| {
        let (b, v, c) = (&b, values(pairs), clear.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>());
        async move { b.computers.save_provider(&b.ctx, ProviderKind::Ec2, &v, &c).await }
    };
    save(&[("access_key_id", "test-access-key-id"), ("secret_access_key", "test-secret-value-long"), ("region", "ap-southeast-1")], &[]).await.unwrap();
    // a profile named with the boxes blank: a blank box keeps the saved key, as the form says
    save(&[("access_key_id", ""), ("secret_access_key", ""), ("profile", "work"), ("region", "eu-west-1")], &[]).await.unwrap();
    assert!(keys(&b.ctx).unwrap().get_provider_cred("ec2").unwrap().is_some());
    // Use the profile instead: the keys are cleared on purpose
    let shown = save(&[("profile", "work"), ("region", "eu-west-1")], &["access_key_id", "secret_access_key"]).await.unwrap();
    assert_eq!(keys(&b.ctx).unwrap().get_provider_cred("ec2").unwrap(), None);
    assert_eq!((field(&shown, "secret_access_key")["set"].clone(), field(&shown, "profile")["value"].clone()), (json!(false), json!("work")));
}

#[test]
fn a_bots_choice_must_be_named_and_only_null_means_the_default() {
    assert_eq!(kind_in(&json!({"kind": null})).unwrap(), None);
    assert_eq!(kind_in(&json!({"kind": "e2b"})).unwrap(), Some(ProviderKind::E2b));
    assert!(kind_in(&json!({})).is_err() && kind_in(&Value::Null).is_err() && kind_in(&json!({"kind": "nope"})).is_err());
}

/// Two Bots at their own E2B computers.
async fn two_on_e2b(b: &Bed) {
    save_e2b(b, GOOD).await.unwrap();
    for bot in [&b.ann.0, &b.bea.0] {
        b.computers.set_bot_home(&b.ctx, bot, Some(ProviderKind::E2b)).unwrap();
        b.computers.click(&b.ctx, bot).await.unwrap();
    }
}

#[tokio::test]
async fn a_new_key_must_see_every_computer_not_only_the_first() {
    let (b, platform) = homes_bed("sees-every");
    platform.first_shared.store(true, Ordering::SeqCst);
    two_on_e2b(&b).await;
    // the other account's key sees Ann's (the first) but not Bea's
    assert_eq!(save_e2b(&b, OTHER).await.unwrap_err().code(), "other_account");
}

#[tokio::test]
async fn a_computer_neither_key_can_see_is_already_gone_and_doesnt_count() {
    let (b, platform) = homes_bed("gone-for-both");
    two_on_e2b(&b).await;
    platform.all_gone.store(true, Ordering::SeqCst);
    save_e2b(&b, OTHER).await.unwrap();
}

#[tokio::test]
async fn a_keys_file_that_cant_be_read_is_an_error_not_no_keys() {
    let (b, _) = homes_bed("unreadable-file");
    save_e2b(&b, GOOD).await.unwrap();
    // its file swapped for something that can't be read as one
    let file = b.ctx.data.join("keys/provider-creds/e2b~default.json");
    std::fs::remove_file(&file).unwrap();
    std::fs::create_dir_all(&file).unwrap();
    assert!(keys(&b.ctx).unwrap().get_provider_cred("e2b").is_err());
    // and a save doesn't go on to act on "there are none"
    assert!(b.computers.save_provider(&b.ctx, ProviderKind::E2b, &values(&[("template", "tpl-2")]), &[]).await.is_err());
    assert!(file.is_dir(), "nothing was removed");
}

#[tokio::test]
async fn a_retired_computer_of_a_kind_this_build_doesnt_know_keeps_its_row() {
    let (b, _) = homes_bed("unknown-kind");
    let id = "cmp_00ff";
    b.ctx.db.with(|c| c.execute("INSERT INTO computers(id, provider, created_at, gone_at) VALUES(?, 'quantum', 1, 1)", [id])).unwrap();
    b.computers.sleep_the_idle(&b.ctx).await;
    let left: i64 = b.ctx.db.with(|c| c.query_row("SELECT COUNT(*) FROM computers WHERE id=?", [id], |r| r.get(0))).unwrap();
    assert_eq!(left, 1, "nothing here could delete it, so its record stays for a build that can");
}

#[test]
fn an_aws_keys_debug_says_where_they_are_from_and_never_the_key() {
    use crate::hub::computer::homes::AwsKeys;
    let saved = AwsKeys::Saved(buwiz_computer::ec2::sigv4::Credentials { access_key: "AKID-shown-nowhere".into(), secret: Secret::new("s"), token: None });
    assert!(!format!("{saved:?}").contains("AKID-shown-nowhere"));
}

#[tokio::test]
async fn a_named_aws_profile_that_gives_no_keys_isnt_ready_or_a_default() {
    let (b, _) = homes_bed("aws-no-profile");
    let shown = b.computers.save_provider(&b.ctx, ProviderKind::Ec2, &values(&[("profile", "buwiz-test-no-such-profile"), ("region", "eu-west-1")]), &[]).await.unwrap();
    assert_eq!(shown["configured"], false);
    assert_eq!(b.computers.set_default(&b.ctx, ProviderKind::Ec2).unwrap_err().code(), "not_configured");
}

#[tokio::test]
async fn keys_that_cant_be_read_make_a_home_refuse_and_never_sign_with_this_macs_own() {
    let (b, _) = homes_bed("unread-refuses");
    // the keys file is there and can't be read, before anything asked for the provider
    std::fs::create_dir_all(b.ctx.data.join("keys/provider-creds/ec2~default.json")).unwrap();
    let p = b.computers.homes.provider(&b.ctx, ProviderKind::Ec2);
    let said = p.verify().await.unwrap_err();
    assert_eq!(said.code(), "keychain", "{}", said.sentence());
    // nothing was kept: once the keys can be read, the provider is made with them
    assert!(b.computers.providers_json(&b.ctx)["providers"].as_array().unwrap().iter().any(|h| h["kind"] == "ec2" && h["configured"] == false));
}

#[tokio::test]
async fn a_computer_being_made_or_removed_holds_off_new_keys() {
    let (b, _) = homes_bed("being-removed");
    save_e2b(&b, GOOD).await.unwrap();
    // a failed create's leftover: a row on its way out, with no record to ask about
    b.ctx.db.with(|c| c.execute("INSERT INTO computers(id, provider, created_at, gone_at) VALUES('cmp_0abc', 'e2b', 1, 1)", [])).unwrap();
    assert_eq!(save_e2b(&b, OTHER).await.unwrap_err().code(), "in_use");
}

#[tokio::test]
async fn a_later_turn_doesnt_post_a_second_setup_card() {
    let (b, _) = homes_bed("setup-twice");
    let first = run(&b, &b.ann.1, None);
    assert!(words(&call(&first, tools::SETUP, json!({"reason": "a browser"})).await).contains("card is in the chat"));
    let later = run(&b, &b.ann.1, None);
    assert!(words(&call(&later, tools::SETUP, json!({"reason": "again"})).await).contains("already in the chat"));
}
