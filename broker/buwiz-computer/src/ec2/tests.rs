//! The provider against a fake EC2 (an HTTP server here that keeps instances and moves them along
//! as EC2 does), a fake `ssh` that logs what it's asked and reads what it's fed, and a fake box on
//! the ports the master forwards. Nothing leaves this Mac.

use super::*;
use base64::Engine;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const TOKEN: &str = "tok-0123456789abcdef";
const VIEWER: &str = "pw-fedcba9876543210";
const PIN_A: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAGvgQ/uwCuNoi8TtSUfRbtFwCChFjTrqFTN6bC3Iq7L";
const PIN_B: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIFMUQzIoUweaWFmCD2hUs96kUjz4ovO6pk0e+3uh7tou";
const TEMPLATE_DATA: &str = "#cloud-config\npackages: [docker.io]\n";

#[derive(Clone, Debug)]
struct FakeInstance {
    id: String,
    state: String,
    hibernation: bool,
    hibernated: bool,
    tags: Vec<(String, String)>,
}

#[derive(Default)]
struct Fake {
    instances: Vec<FakeInstance>,
    calls: Vec<(String, HashMap<String, String>)>,
    auth: Vec<String>,
    console: String,
    refuse_hibernate: Option<String>,
    /// what a dry-run launch is refused with, when not `DryRunOperation`
    deny_launch: Option<String>,
    /// a dry run that launches anyway
    launch_on_dry: bool,
}

type Shared = Arc<Mutex<Fake>>;

fn decode(v: &str) -> String {
    let bytes = v.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            out.push(u8::from_str_radix(&v[i + 1..i + 3], 16).unwrap());
            i += 3;
        } else {
            out.push(if bytes[i] == b'+' { b' ' } else { bytes[i] });
            i += 1;
        }
    }
    String::from_utf8(out).unwrap()
}

fn instance_xml(i: &FakeInstance) -> String {
    let tags: String = i.tags.iter().map(|(k, v)| format!("<item><key>{k}</key><value>{v}</value></item>")).collect();
    let ip = if i.state == "running" { "<ipAddress>203.0.113.7</ipAddress>" } else { "" };
    format!("<item><instanceId>{}</instanceId><instanceState><code>0</code><name>{}</name></instanceState>{ip}<tagSet>{tags}</tagSet><hibernationOptions><configured>{}</configured></hibernationOptions></item>", i.id, i.state, i.hibernation)
}

fn refused(code: &str) -> (u16, String) {
    (400, format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?><Response><Errors><Error><Code>{code}</Code><Message>no</Message></Error></Errors><RequestID>r</RequestID></Response>"))
}

fn answer(fake: &Shared, action: &str, q: &HashMap<String, String>) -> (u16, String) {
    let mut f = fake.lock().unwrap();
    let ok = |body: String| (200, format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?><{action}Response xmlns=\"http://ec2.amazonaws.com/doc/2016-11-15/\"><requestId>r</requestId>{body}</{action}Response>"));
    let ids: Vec<String> = (1..10).filter_map(|n| q.get(&format!("InstanceId.{n}")).cloned()).collect();
    match action {
        "DescribeInstances" => {
            let wanted: Vec<FakeInstance> = if ids.is_empty() {
                let has = |k: &str, v: &str| q.values().any(|x| x == k) && q.values().any(|x| x == v);
                let id = q.get("Filter.2.Value.1").cloned().unwrap_or_default();
                assert!(has("tag:buwiz:managed", "true") && q.get("Filter.2.Name").map(String::as_str) == Some("tag:buwiz:computer-id"), "a sweep filters by both tags: {q:?}");
                f.instances.iter().filter(|i| i.tags.contains(&("buwiz:managed".into(), "true".into())) && i.tags.contains(&("buwiz:computer-id".into(), id.clone())) && i.state != "terminated").cloned().collect()
            } else {
                let found: Vec<FakeInstance> = f.instances.iter().filter(|i| ids.contains(&i.id)).cloned().collect();
                if found.is_empty() {
                    return refused("InvalidInstanceID.NotFound");
                }
                found
            };
            // as EC2 does: each look finds it a step further along
            for i in f.instances.iter_mut().filter(|i| wanted.iter().any(|w| w.id == i.id)) {
                i.state = match i.state.as_str() {
                    "pending" => "running",
                    "stopping" => "stopped",
                    "shutting-down" => "terminated",
                    s => s,
                }
                .to_owned();
            }
            let items: String = wanted.iter().map(instance_xml).collect();
            ok(format!("<reservationSet><item><reservationId>r-1</reservationId><instancesSet>{items}</instancesSet></item></reservationSet>"))
        }
        "RunInstances" if q.get("DryRun").map(String::as_str) == Some("true") && !f.launch_on_dry => {
            refused(f.deny_launch.as_deref().unwrap_or("DryRunOperation"))
        }
        "RunInstances" => {
            let id = format!("i-0abc{:012}", f.instances.len() + 1);
            let tags = (1..10).filter_map(|n| Some((q.get(&format!("TagSpecification.1.Tag.{n}.Key"))?.clone(), q.get(&format!("TagSpecification.1.Tag.{n}.Value"))?.clone()))).collect();
            let made = FakeInstance { id, state: "pending".into(), hibernation: true, hibernated: false, tags };
            f.instances.push(made.clone());
            ok(format!("<reservationId>r-1</reservationId><instancesSet>{}</instancesSet>", instance_xml(&made)))
        }
        "StartInstances" => {
            for i in f.instances.iter_mut().filter(|i| ids.contains(&i.id) && i.state == "stopped") {
                i.state = "pending".into();
            }
            ok("<instancesSet/>".into())
        }
        "StopInstances" => {
            let hibernate = q.get("Hibernate").map(String::as_str) == Some("true");
            if hibernate {
                if let Some(code) = f.refuse_hibernate.clone() {
                    return refused(&code);
                }
            }
            for i in f.instances.iter_mut().filter(|i| ids.contains(&i.id)) {
                i.state = "stopping".into();
                i.hibernated = hibernate;
            }
            ok("<instancesSet/>".into())
        }
        "TerminateInstances" => {
            for i in f.instances.iter_mut().filter(|i| ids.contains(&i.id)) {
                i.state = "shutting-down".into();
            }
            ok("<instancesSet/>".into())
        }
        "GetConsoleOutput" => ok(format!("<instanceId>x</instanceId><output>{}</output>", base64::engine::general_purpose::STANDARD.encode(&f.console))),
        "DescribeLaunchTemplates" => ok("<launchTemplates><item><launchTemplateId>lt-1</launchTemplateId></item></launchTemplates>".into()),
        "DescribeLaunchTemplateVersions" => ok(format!("<launchTemplateVersionSet><item><launchTemplateData><userData>{}</userData></launchTemplateData></item></launchTemplateVersionSet>", base64::engine::general_purpose::STANDARD.encode(TEMPLATE_DATA))),
        other => panic!("the broker may not call {other}"),
    }
}

async fn fake_ec2(fake: Shared) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else { return };
            let fake = fake.clone();
            tokio::spawn(async move {
                let mut got = Vec::new();
                let mut chunk = [0u8; 8192];
                let (head, body) = loop {
                    let n = socket.read(&mut chunk).await.unwrap();
                    if n == 0 {
                        return;
                    }
                    got.extend_from_slice(&chunk[..n]);
                    let text = String::from_utf8_lossy(&got).into_owned();
                    if let Some(at) = text.find("\r\n\r\n") {
                        let head = text[..at].to_owned();
                        let length: usize = head.lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse().unwrap())).unwrap_or(0);
                        if got.len() >= at + 4 + length {
                            break (head, String::from_utf8_lossy(&got[at + 4..at + 4 + length]).into_owned());
                        }
                    }
                };
                let q: HashMap<String, String> = body.split('&').filter_map(|p| p.split_once('=')).map(|(k, v)| (decode(k), decode(v))).collect();
                let action = q.get("Action").cloned().unwrap_or_default();
                {
                    let mut f = fake.lock().unwrap();
                    f.auth.push(head.lines().find_map(|l| l.strip_prefix("authorization: ")).unwrap_or_default().to_owned());
                    f.calls.push((action.clone(), q.clone()));
                }
                let (status, xml) = answer(&fake, &action, &q);
                let reply = format!("HTTP/1.1 {status} X\r\ncontent-type: text/xml\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{xml}", xml.len());
                let _ = socket.write_all(reply.as_bytes()).await;
            });
        }
    });
    format!("http://{addr}")
}

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("buwiz-ec2-{name}-{}", random_hex(4).unwrap()));
    std::fs::create_dir_all(d.join("cmp_ab12")).unwrap();
    d
}

fn script(path: &Path, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path.to_owned()
}

/// An `ssh` that logs its arguments and keeps what it's fed. Its master either stays up, or says
/// the host key is wrong.
fn fake_ssh(d: &Path, master_up: bool) -> PathBuf {
    let master = if master_up { format!(": > c; printf '%s\\n' \"$*\" > \"{d}/master\"; exec /bin/sleep 600", d = d.display()) } else { "echo 'Host key verification failed.' >&2; exit 255".to_owned() };
    let body = format!(
        r#"d="{d}"
printf '%s\n' "$*" >> "$d/ssh-calls"
case "$*" in
*"-O check"*) [ -e c ] && exit 0; exit 1;;
*"-O exit"*) /bin/rm -f c; exit 0;;
*"-M -N"*) {master};;
esac
/bin/cat > "$d/stdin-$$"
case "$*" in
*buwiz:prepare*) /bin/cat "$d/box-state";;
*buwiz:start*) : > "$d/box-up"; echo started;;
esac
exit 0"#,
        d = d.display()
    );
    script(&d.join("fake-ssh"), &body)
}

/// A box on every port the master forwards: up once `box-up` is there, taking only `TOKEN`.
fn fake_box(d: PathBuf) {
    tokio::spawn(async move {
        let mut seen = String::new();
        loop {
            let master = loop {
                match std::fs::read_to_string(d.join("master")) {
                    Ok(text) if text != seen => break text,
                    _ => tokio::time::sleep(Duration::from_millis(10)).await,
                }
            };
            seen = master.clone();
            serve_forwards(&d, &master).await;
        }
    });
}

async fn serve_forwards(d: &Path, master: &str) {
    {
        let words: Vec<&str> = master.split_whitespace().collect();
        for pair in words.windows(2).filter(|w| w[0] == "-L") {
            let here: u16 = pair[1].split(':').nth(1).unwrap().parse().unwrap();
            let listener = TcpListener::bind(("127.0.0.1", here)).await.unwrap();
            let d = d.to_owned();
            tokio::spawn(async move {
                loop {
                    let Ok((mut socket, _)) = listener.accept().await else { return };
                    let up = d.join("box-up").exists();
                    tokio::spawn(async move {
                        let mut buf = vec![0u8; 16384];
                        let n = socket.read(&mut buf).await.unwrap_or(0);
                        let head = String::from_utf8_lossy(&buf[..n]).to_ascii_lowercase();
                        let status = if !up { "503 Down" } else if head.contains(&format!("authorization: bearer {TOKEN}")) { "200 OK" } else { "401 No" };
                        let _ = socket.write_all(format!("HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{{}}").as_bytes()).await;
                    });
                }
            });
        }
    }
}

fn quick() -> Patience {
    let s = Duration::from_secs;
    Patience { state: s(5), console: s(2), login: s(1), setup: s(10), answer: s(5), poll: Duration::from_millis(10) }
}

async fn provider(d: &Path, fake: &Shared, master_up: bool) -> Ec2Provider {
    let endpoint = fake_ec2(fake.clone()).await;
    let keys = Arc::new(sigv4::Credentials { access_key: "AKIDTEST".into(), secret: Secret::new("not-a-real-secret"), token: None });
    Ec2Provider::new(Config::default(), keys).with_endpoint(&endpoint).with_programs(fake_ssh(d, master_up), PathBuf::from(KEYGEN)).with_patience(quick())
}

fn machine(d: &Path, id: &str) -> Machine {
    Machine { provider: ProviderKind::Ec2, reach: Reach::Instance { id: id.into(), region: DEFAULT_REGION.into() }, image: Some(DEFAULT_IMAGE.into()), token: Secret::new(TOKEN), viewer_password: Secret::new(VIEWER), dir: d.join("cmp_ab12"), link: None, egress: None, seen: None }
}

fn ours(id: &str, computer: &str, state: &str) -> FakeInstance {
    FakeInstance { id: id.into(), state: state.into(), hibernation: true, hibernated: false, tags: api::tags(computer).to_vec() }
}

fn actions(fake: &Shared) -> Vec<String> {
    fake.lock().unwrap().calls.iter().map(|(a, _)| a.clone()).collect()
}

fn stops(fake: &Shared) -> Vec<String> {
    fake.lock().unwrap().calls.iter().filter(|(a, _)| a == "StopInstances").map(|(_, q)| q["Hibernate"].clone()).collect()
}

fn fed(d: &Path) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(d).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().starts_with("stdin-")).map(|e| std::fs::read_to_string(e.path()).unwrap()).filter(|s| !s.is_empty()).collect();
    out.sort();
    out
}

fn ssh_calls(d: &Path) -> String {
    std::fs::read_to_string(d.join("ssh-calls")).unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread")]
async fn create_launches_from_the_template_tagged_with_only_a_public_key_in_its_user_data() {
    let (d, fake) = (tmp("create"), Shared::default());
    let p = provider(&d, &fake, true).await;
    let spec = Spec { id: "cmp_ab12".into(), dir: d.join("cmp_ab12"), network: Default::default() };
    let m = p.create(&spec).await.unwrap();
    assert_eq!(m.provider, ProviderKind::Ec2);
    let Reach::Instance { id, region } = &m.reach else { panic!("{:?}", m.reach) };
    assert!(id.starts_with("i-") && region == DEFAULT_REGION);
    assert_ne!(m.token, m.viewer_password);
    assert_eq!(actions(&fake), ["DescribeLaunchTemplates", "DescribeLaunchTemplateVersions", "RunInstances"], "nothing but what the broker's policy allows");
    let f = fake.lock().unwrap();
    let run = &f.calls[2].1;
    assert_eq!(run["LaunchTemplate.LaunchTemplateName"], "buwiz-computer");
    for (n, kind) in [(1, "instance"), (2, "volume")] {
        assert_eq!(run[&format!("TagSpecification.{n}.ResourceType")], kind);
        let tags: Vec<(String, String)> = (1..4).map(|t| (run[&format!("TagSpecification.{n}.Tag.{t}.Key")].clone(), run[&format!("TagSpecification.{n}.Tag.{t}.Value")].clone())).collect();
        assert!(tags.contains(&("buwiz:managed".into(), "true".into())) && tags.contains(&("buwiz:computer-id".into(), "cmp_ab12".into())) && tags.contains(&("Name".into(), "buwiz-computer-cmp_ab12".into())), "{tags:?}");
    }
    let data = String::from_utf8(base64::engine::general_purpose::STANDARD.decode(&run["UserData"]).unwrap()).unwrap();
    let public = std::fs::read_to_string(d.join("cmp_ab12/ec2/key.pub")).unwrap();
    assert!(data.contains("packages: [docker.io]"), "the template's own user data stays: {data}");
    assert!(data.contains(&format!("  - {}", public.trim())), "{data}");
    for secret in [m.token.expose(), m.viewer_password.expose()] {
        assert!(!data.contains(secret) && !format!("{:?}", f.calls).contains(secret), "a secret went to AWS");
    }
    assert!(!data.contains("PRIVATE KEY"));
    assert!(f.auth.iter().all(|a| a.starts_with("AWS4-HMAC-SHA256 Credential=AKIDTEST/") && a.contains("/ap-southeast-1/ec2/aws4_request, SignedHeaders=content-type;host;x-amz-date, Signature=")), "{:?}", f.auth);
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::fs::metadata(d.join("cmp_ab12/ec2/key")).unwrap().permissions().mode() & 0o777, 0o600);
}

#[tokio::test(flavor = "multi_thread")]
async fn power_follows_ec2s_states() {
    let (d, fake) = (tmp("power"), Shared::default());
    fake.lock().unwrap().instances.push(ours("i-1", "cmp_ab12", "pending"));
    let p = provider(&d, &fake, true).await;
    let m = machine(&d, "i-1");
    assert_eq!(p.power(&m).await.unwrap(), Power::Waking);
    assert_eq!(p.power(&m).await.unwrap(), Power::Awake);
    fake.lock().unwrap().instances[0].state = "stopping".into();
    assert_eq!(p.power(&m).await.unwrap(), Power::Asleep);
    assert_eq!(p.power(&m).await.unwrap(), Power::Asleep, "stopped");
    fake.lock().unwrap().instances[0].state = "shutting-down".into();
    assert_eq!(p.power(&m).await.unwrap(), Power::Absent);
    assert_eq!(p.power(&machine(&d, "i-gone")).await.unwrap(), Power::Absent);
    assert_eq!(power_of("rebooting").word(), "error");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_first_wake_pins_the_consoles_key_and_only_then_sends_the_secrets_on_stdin() {
    let (d, fake) = (tmp("wake"), Shared::default());
    {
        let mut f = fake.lock().unwrap();
        f.instances.push(ours("i-1", "cmp_ab12", "pending"));
        f.console = console::tests::console(PIN_A);
    }
    std::fs::write(d.join("box-state"), "absent\n").unwrap();
    let p = provider(&d, &fake, true).await;
    let m = machine(&d, "i-1");
    keygen(Path::new(KEYGEN), &m.dir.join(FOLDER), "cmp_ab12").await.unwrap();
    fake_box(d.clone());
    assert_eq!(p.wake(&m).await.unwrap(), Woke::Started);
    assert_eq!(std::fs::read_to_string(m.dir.join("ec2/pin")).unwrap().trim(), PIN_A);
    assert_eq!(std::fs::read_to_string(m.dir.join("ssh/known_hosts")).unwrap(), format!("computer {PIN_A}\n"), "ssh trusts the console's key and no other");
    assert!(!actions(&fake).contains(&"StartInstances".to_owned()), "it was on its way up already");
    let calls = ssh_calls(&d);
    let at = |w: &str| calls.find(w).unwrap_or_else(|| panic!("{w}: {calls}"));
    assert!(at("-M -N") < at("buwiz:prepare") && at("buwiz:prepare") < at("buwiz:start") && at("buwiz:start") < at("buwiz:clear"), "{calls}");
    assert!(calls.contains("ubuntu") && calls.contains("203.0.113.7") && calls.contains("StrictHostKeyChecking=yes"), "{calls}");
    assert!(!calls.contains(TOKEN) && !calls.contains(VIEWER), "a secret on a command line");
    assert_eq!(fed(&d), [format!("{TOKEN}\n{VIEWER}\n")], "the secrets went once, to the start, on its stdin");
    assert!(!m.dir.join("ssh/key").exists(), "the key isn't left staged");
    p.guard(&m).await.unwrap();
    let screen = Screen { display: ":2".into(), viewer_port: 6902, folder: "agents/b_1".into(), profile: None };
    let viewer = p.viewer(&m, &screen).await.unwrap().unwrap();
    assert!(viewer.addr.starts_with("127.0.0.1:") && viewer.far.is_some());
    let port: u16 = viewer.addr.rsplit(':').next().unwrap().parse().unwrap();
    assert!((crate::PORT_FIRST..=crate::DOCKER_PORT_LAST).contains(&port), "in the range the shell is kept off");
}

#[tokio::test(flavor = "multi_thread")]
async fn sleep_hibernates_and_the_wake_after_finds_the_box_as_it_was() {
    let (d, fake) = (tmp("hibernate"), Shared::default());
    {
        let mut f = fake.lock().unwrap();
        f.instances.push(ours("i-1", "cmp_ab12", "running"));
        f.console = console::tests::console(PIN_A);
    }
    std::fs::write(d.join("box-state"), "running\n").unwrap();
    std::fs::write(d.join("box-up"), "").unwrap();
    let p = provider(&d, &fake, true).await;
    let m = machine(&d, "i-1");
    keygen(Path::new(KEYGEN), &m.dir.join(FOLDER), "cmp_ab12").await.unwrap();
    fake_box(d.clone());
    assert_eq!(p.wake(&m).await.unwrap(), Woke::Resumed, "a running box that takes the token is used as it is");
    p.sleep(&m, Sleep::Light).await.unwrap();
    assert_eq!(stops(&fake), ["true"]);
    assert!(fake.lock().unwrap().instances[0].hibernated);
    assert_eq!(p.take_sleep_note(&m), None, "a hibernation has nothing to tell");
    assert_eq!(p.power(&m).await.unwrap(), Power::Asleep, "stopping");
    assert_eq!(p.power(&m).await.unwrap(), Power::Asleep, "stopped");
    assert!(matches!(p.screens(&m).await, Err(Problem::Asleep)), "the link went with the sleep");
    p.sleep(&m, Sleep::Deep).await.unwrap();
    assert_eq!(stops(&fake), ["true"], "deeper than stopped is nothing to do");
    assert_eq!(p.wake(&m).await.unwrap(), Woke::Resumed);
    assert!(actions(&fake).contains(&"StartInstances".to_owned()));
    assert!(!ssh_calls(&d).contains("buwiz:start"), "nothing was started again");
    assert!(fed(&d).is_empty(), "no secret had to go");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_box_that_refuses_the_records_token_is_started_again_with_it() {
    let (d, fake) = (tmp("refused"), Shared::default());
    {
        let mut f = fake.lock().unwrap();
        f.instances.push(ours("i-1", "cmp_ab12", "running"));
        f.console = console::tests::console(PIN_A);
    }
    std::fs::write(d.join("box-state"), "running\n").unwrap();
    let p = provider(&d, &fake, true).await.with_patience(Patience { answer: Duration::from_secs(5), ..quick() });
    let m = machine(&d, "i-1");
    keygen(Path::new(KEYGEN), &m.dir.join(FOLDER), "cmp_ab12").await.unwrap();
    fake_box(d.clone());
    // not up for the first look: as a recovered record's old box, which won't answer
    assert_eq!(p.wake(&m).await.unwrap(), Woke::Started);
    let calls = ssh_calls(&d);
    assert!(calls.find("buwiz:stop").unwrap() < calls.find("buwiz:start").unwrap(), "{calls}");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_instance_that_cant_hibernate_is_stopped_and_deep_sleep_is_a_stop() {
    let (d, fake) = (tmp("fallback"), Shared::default());
    {
        let mut f = fake.lock().unwrap();
        f.instances.push(ours("i-1", "cmp_ab12", "running"));
        f.refuse_hibernate = Some("UnsupportedHibernationConfiguration".into());
    }
    let p = provider(&d, &fake, true).await;
    let m = machine(&d, "i-1");
    p.sleep(&m, Sleep::Light).await.unwrap();
    assert_eq!(stops(&fake), ["true", "false"]);
    let note = p.take_sleep_note(&m).expect("a hibernation that became a stop is told");
    assert!(note.contains("couldn't hibernate (AWS said UnsupportedHibernationConfiguration)") && note.contains("won't be there when it wakes"), "{note}");
    assert_eq!(p.take_sleep_note(&m), None, "taken once");
    {
        let mut f = fake.lock().unwrap();
        f.instances[0] = FakeInstance { hibernation: false, ..ours("i-1", "cmp_ab12", "running") };
        f.calls.clear();
    }
    p.sleep(&m, Sleep::Light).await.unwrap();
    assert_eq!(stops(&fake), ["false"], "one launched without hibernation isn't asked to");
    assert!(p.take_sleep_note(&m).is_some_and(|n| n.contains("wasn't made able to hibernate")));
    {
        let mut f = fake.lock().unwrap();
        f.instances[0] = ours("i-1", "cmp_ab12", "running");
        f.refuse_hibernate = Some("UnauthorizedOperation".into());
        f.calls.clear();
    }
    assert!(p.sleep(&m, Sleep::Light).await.unwrap_err().sentence().contains("may not"), "a policy's no isn't papered over");
    assert_eq!(p.take_sleep_note(&m), None, "a sleep that failed says so itself");
    fake.lock().unwrap().calls.clear();
    p.sleep(&m, Sleep::Deep).await.unwrap();
    assert_eq!(stops(&fake), ["false"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_host_key_that_isnt_the_pinned_one_gets_no_secret() {
    let (d, fake) = (tmp("mismatch"), Shared::default());
    {
        let mut f = fake.lock().unwrap();
        f.instances.push(ours("i-1", "cmp_ab12", "running"));
        f.console = console::tests::console(PIN_A);
    }
    let p = provider(&d, &fake, false).await;
    let m = machine(&d, "i-1");
    keygen(Path::new(KEYGEN), &m.dir.join(FOLDER), "cmp_ab12").await.unwrap();
    // pinned at the first boot: the machine at this address now shows another
    std::fs::write(m.dir.join("ec2/pin"), format!("{PIN_B}\n")).unwrap();
    let said = p.wake(&m).await.unwrap_err().sentence();
    assert!(said.contains("isn't the one you trusted"), "{said}");
    assert!(!actions(&fake).contains(&"GetConsoleOutput".to_owned()), "a pin is never read again");
    let calls = ssh_calls(&d);
    assert!(!calls.contains("buwiz:prepare") && !calls.contains("buwiz:start"), "{calls}");
    assert!(fed(&d).is_empty(), "nothing was fed to the far end");
    assert_eq!(std::fs::read_to_string(m.dir.join("ssh/known_hosts")).unwrap(), format!("computer {PIN_B}\n"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_console_that_doesnt_hold_together_means_no_login_at_all() {
    let (d, fake) = (tmp("doubt"), Shared::default());
    {
        let mut f = fake.lock().unwrap();
        f.instances.push(ours("i-1", "cmp_ab12", "running"));
        f.console = format!("{}{}", console::tests::console(PIN_A), console::tests::console(PIN_B));
    }
    let p = provider(&d, &fake, true).await;
    let m = machine(&d, "i-1");
    let said = p.wake(&m).await.unwrap_err().sentence();
    assert!(said.contains("doesn't say for sure"), "{said}");
    assert!(ssh_calls(&d).is_empty(), "ssh never ran");
    assert!(!m.dir.join("ec2/pin").exists());
    // and one that hasn't printed it yet is waited for, then given up on
    fake.lock().unwrap().console = "booting...\n".into();
    assert!(p.wake(&m).await.unwrap_err().sentence().contains("didn't print its SSH key"));
    assert!(ssh_calls(&d).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn destroy_terminates_and_sweep_and_recover_go_by_both_tags_alone() {
    let (d, fake) = (tmp("sweep"), Shared::default());
    {
        let mut f = fake.lock().unwrap();
        f.instances.push(ours("i-1", "cmp_ab12", "running"));
        f.instances.push(ours("i-2", "cmp_ab12", "stopped"));
        f.instances.push(ours("i-3", "cmp_other", "running"));
        // someone else's, with the id tag and not the managed one, and one with no tags at all
        f.instances.push(FakeInstance { tags: vec![("buwiz:computer-id".into(), "cmp_ab12".into())], ..ours("i-4", "x", "running") });
        f.instances.push(FakeInstance { tags: Vec::new(), ..ours("i-02615fd28737cd067", "x", "running") });
    }
    let p = provider(&d, &fake, true).await;
    let spec = Spec { id: "cmp_ab12".into(), dir: d.join("cmp_ab12"), network: Default::default() };
    assert_eq!(p.recover(&spec, None).await.unwrap(), None, "no key left, no way in");
    keygen(Path::new(KEYGEN), &spec.dir.join(FOLDER), "cmp_ab12").await.unwrap();
    let found = p.recover(&spec, None).await.unwrap().unwrap();
    assert!(matches!(&found.reach, Reach::Instance { id, .. } if id == "i-1" || id == "i-2"));
    assert_ne!(found.token.expose(), TOKEN, "new secrets");
    // a `last` that isn't ours is never touched
    p.sweep("cmp_ab12", Some(&Reach::Instance { id: "i-4".into(), region: DEFAULT_REGION.into() })).await.unwrap();
    let terminated: Vec<String> = fake.lock().unwrap().calls.iter().filter(|(a, _)| a == "TerminateInstances").flat_map(|(_, q)| q.iter().filter(|(k, _)| k.starts_with("InstanceId")).map(|(_, v)| v.clone()).collect::<Vec<_>>()).collect();
    let mut terminated = terminated;
    terminated.sort();
    assert_eq!(terminated, ["i-1", "i-2"]);
    p.destroy(&machine(&d, "i-3")).await.unwrap();
    assert_eq!(fake.lock().unwrap().instances[2].state, "shutting-down");
    p.destroy(&machine(&d, "i-gone")).await.unwrap();
    let f = fake.lock().unwrap();
    assert_eq!(f.instances[3].state, "running");
    assert_eq!(f.instances[4].state, "running");
}

/// The signer against AWS itself, read-only: `cargo test -p buwiz-computer --lib ec2::tests::real -- --ignored`
/// with the `buwiz-broker` profile on this Mac. It describes, and changes nothing.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "calls AWS, read-only"]
async fn real_aws_takes_the_signature_and_finds_no_computer_by_a_made_up_id() {
    let (config, keys) = from_env();
    let api = Api::new(&config.region, keys).unwrap();
    assert!(api.tagged("cmp_does_not_exist").await.unwrap().is_empty());
    assert_eq!(api.describe("i-00000000000000000").await.unwrap(), None);
}

#[tokio::test]
async fn a_check_launches_nothing_and_says_when_the_keys_may_not() {
    let d = tmp("verify");
    let fake: Shared = Arc::default();
    let p = provider(&d, &fake, true).await;
    p.verify().await.unwrap();
    assert!(fake.lock().unwrap().instances.is_empty(), "a dry run makes nothing");
    fake.lock().unwrap().deny_launch = Some("UnauthorizedOperation".into());
    let said = p.verify().await.unwrap_err();
    assert_eq!(said.code(), "not_allowed");
    assert!(said.sentence().contains("buwiz:managed=true"), "{}", said.sentence());
    fake.lock().unwrap().deny_launch = Some("AuthFailure".into());
    assert_eq!(p.verify().await.unwrap_err().code(), "bad_key");
}

#[tokio::test]
async fn a_check_that_launched_ends_what_it_launched_and_doesnt_pass() {
    let d = tmp("verify-launched");
    let fake: Shared = Arc::default();
    fake.lock().unwrap().launch_on_dry = true;
    let p = provider(&d, &fake, true).await;
    let said = p.verify().await.unwrap_err();
    assert!(said.sentence().contains("terminated at once"), "{}", said.sentence());
    let f = fake.lock().unwrap();
    assert!(f.instances.iter().all(|i| i.state == "shutting-down"), "nothing it launched is left running");
}
