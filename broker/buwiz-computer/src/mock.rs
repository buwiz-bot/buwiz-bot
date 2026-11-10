//! A provider that runs nothing, for the broker's tests: a computer is a word in memory, and
//! every call is written down.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use async_trait::async_trait;

use crate::{CommandOutput, Input, Machine, Power, Problem, Provider, ProviderKind, Reach, Result, Screen, Screenshot, Secret, Sleep, Spec, Viewer, Woke};

/// Its computer's token and viewer password, for tests that look for them where they mustn't be.
pub const TOKEN: &str = "mock-token-0123456789abcdef";
pub const PASSWORD: &str = "mock-viewer-password-456";

pub struct MockProvider {
    pub power: Mutex<Power>,
    pub calls: Mutex<Vec<String>>,
    /// `check` and `create` fail with this
    pub problem: Mutex<Option<Problem>>,
    /// `run` answers with this on stdout
    pub stdout: Mutex<String>,
    /// what `run` answers instead for a command holding the first word; `Err` fails it
    pub replies: Mutex<Vec<(String, Result<String>)>>,
    /// `run` exits 1 for a command holding this word
    pub fails: Mutex<Option<String>>,
    /// `run` waits here until released, for a command holding this word
    pub hold_run: Mutex<Option<String>>,
    /// `wake` waits here until released, when set
    pub hold: AtomicBool,
    /// asleep lightly: a wake resumes it
    pub paused: AtomicBool,
    /// `create` leaves a thing in the computer's folder that stops its record being saved
    pub poison_save: AtomicBool,
    /// `create` takes this long, for tests of two at once
    pub create_ms: std::sync::atomic::AtomicU64,
    /// `create` fails with this, part way
    pub create_problem: Mutex<Option<Problem>>,
    /// `sleep` never returns while set
    pub hang_sleep: AtomicBool,
    /// `destroy` never returns while set: a hung Docker
    pub hang_destroy: AtomicBool,
    /// what `recover` finds
    pub recoverable: Mutex<Option<Reach>>,
    /// `look` fails with this
    pub look_problem: Mutex<Option<Problem>>,
    /// `screen_up` fails with this
    pub screen_problem: Mutex<Option<Problem>>,
    /// `guard` fails with this
    pub unsafe_now: Mutex<Option<Problem>>,
    /// what `resolve` answers for a path, instead of the path itself
    pub links: Mutex<std::collections::HashMap<String, String>>,
    /// the Bots with a screen, in the order they first got one
    pub screens: Mutex<Vec<String>>,
    /// where `viewer` says the screen is, instead of its port
    pub viewer_addr: Mutex<Option<String>>,
    /// `record_stop` fails with this
    pub record_problem: Mutex<Option<Problem>>,
    /// `sleep` fails with this
    pub sleep_problem: Mutex<Option<Problem>>,
    /// `record_start` waits here until released, when set
    pub hold_record: AtomicBool,
    /// what `clear_strays` found
    pub strays: Mutex<Vec<String>>,
    /// `clear_strays` fails with this
    pub stray_problem: Mutex<Option<Problem>>,
    /// what `users_apart` answers; left alone, no
    pub apart: Mutex<Option<Result<bool>>>,
    /// what a light sleep leaves for `take_sleep_note`
    pub sleep_note: Mutex<Option<String>>,
    /// what the last sleep left, until taken
    noted: Mutex<Option<String>>,
    /// what `leftovers` finds
    pub leftovers: Mutex<Vec<crate::Leftover>>,
    /// `leftovers` waits while this is set, as a Docker that hangs does
    pub hang_leftovers: AtomicBool,
    /// `leftovers` can't say while this is set, as a provider with none to tell of
    pub leftovers_unknown: AtomicBool,
    /// what `cant_keep_lost` says: a kind whose lost computers can't be kept
    pub cant_keep: Mutex<Option<String>>,
}

impl Default for MockProvider {
    fn default() -> Self {
        Self { power: Mutex::new(Power::Absent), calls: Mutex::new(Vec::new()), problem: Mutex::new(None), stdout: Mutex::new(String::new()), replies: Mutex::default(), fails: Mutex::new(None), hold_run: Mutex::new(None), hold: AtomicBool::new(false), paused: AtomicBool::new(false), poison_save: AtomicBool::new(false), create_ms: std::sync::atomic::AtomicU64::new(0), create_problem: Mutex::new(None), hang_sleep: AtomicBool::new(false), hang_destroy: AtomicBool::new(false), recoverable: Mutex::new(None), look_problem: Mutex::new(None), screen_problem: Mutex::new(None), unsafe_now: Mutex::new(None), links: Mutex::default(), screens: Mutex::new(Vec::new()), viewer_addr: Mutex::new(None), record_problem: Mutex::new(None), sleep_problem: Mutex::new(None), hold_record: AtomicBool::new(false), strays: Mutex::default(), stray_problem: Mutex::new(None), apart: Mutex::new(None), sleep_note: Mutex::new(None), noted: Mutex::new(None), leftovers: Mutex::default(), hang_leftovers: AtomicBool::new(false), leftovers_unknown: AtomicBool::new(false), cant_keep: Mutex::new(None) }
    }
}

impl MockProvider {
    fn note(&self, call: String) {
        self.calls.lock().unwrap_or_else(|e| e.into_inner()).push(call);
    }

    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn set_power(&self, power: Power) {
        *self.power.lock().unwrap_or_else(|e| e.into_inner()) = power;
    }

    fn awake(&self) -> Result<()> {
        match *self.power.lock().unwrap_or_else(|e| e.into_inner()) {
            Power::Awake => Ok(()),
            _ => Err(Problem::Asleep),
        }
    }
}

/// A real 2x2 PNG.
fn tiny_png() -> Vec<u8> {
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, 2, 2);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    if let Ok(mut writer) = encoder.write_header() {
        let _ = writer.write_image_data(&[0u8; 12]);
        let _ = writer.finish();
    }
    out
}

#[async_trait]
impl Provider for MockProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Docker
    }

    async fn check(&self) -> Result<()> {
        match self.problem.lock().unwrap_or_else(|e| e.into_inner()).clone() {
            Some(p) => Err(p),
            None => Ok(()),
        }
    }

    async fn create(&self, spec: &Spec) -> Result<Machine> {
        self.check().await?;
        if let Some(p) = self.create_problem.lock().unwrap_or_else(|e| e.into_inner()).clone() {
            return Err(p);
        }
        tokio::time::sleep(std::time::Duration::from_millis(self.create_ms.load(Ordering::SeqCst))).await;
        self.note(format!("create {}", spec.id));
        self.set_power(Power::Asleep);
        if self.poison_save.load(Ordering::SeqCst) {
            std::fs::create_dir_all(spec.dir.join("machine.json.new")).map_err(|e| Problem::Failed(e.to_string()))?;
        }
        Ok(Machine {
            provider: ProviderKind::Docker,
            reach: Reach::Container { name: format!("mock-{}", spec.id) },
            image: Some("mock:1".to_owned()),
            token: Secret::new(TOKEN),
            viewer_password: Secret::new(PASSWORD),
            dir: spec.dir.clone(),
            link: None,
            seen: None,
        })
    }

    async fn look(&self, machine: &Machine) -> Result<(Power, Machine)> {
        if let Some(p) = self.look_problem.lock().unwrap_or_else(|e| e.into_inner()).clone() {
            return Err(p);
        }
        Ok((self.power(machine).await?, machine.clone()))
    }

    async fn power(&self, _: &Machine) -> Result<Power> {
        Ok(self.power.lock().unwrap_or_else(|e| e.into_inner()).clone())
    }

    async fn wake(&self, _: &Machine) -> Result<Woke> {
        self.check().await?;
        if *self.power.lock().unwrap_or_else(|e| e.into_inner()) == Power::Absent {
            self.note("rebuild".to_owned());
        }
        self.note("wake".to_owned());
        while self.hold.load(Ordering::SeqCst) {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        self.set_power(Power::Awake);
        Ok(if self.paused.swap(false, Ordering::SeqCst) { Woke::Resumed } else { Woke::Started })
    }

    async fn sleep(&self, _: &Machine, depth: Sleep) -> Result<()> {
        while self.hang_sleep.load(Ordering::SeqCst) {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        if let Some(p) = self.sleep_problem.lock().unwrap_or_else(|e| e.into_inner()).clone() {
            return Err(p);
        }
        self.note(format!("sleep {depth:?}"));
        self.paused.store(depth == Sleep::Light, Ordering::SeqCst);
        self.set_power(Power::Asleep);
        let left = if depth == Sleep::Light { self.sleep_note.lock().unwrap_or_else(|e| e.into_inner()).clone() } else { None };
        *self.noted.lock().unwrap_or_else(|e| e.into_inner()) = left;
        Ok(())
    }

    fn take_sleep_note(&self, _: &Machine) -> Option<String> {
        self.noted.lock().unwrap_or_else(|e| e.into_inner()).take()
    }

    async fn leftovers(&self) -> Result<Option<Vec<crate::Leftover>>> {
        while self.hang_leftovers.load(Ordering::SeqCst) {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        if self.leftovers_unknown.load(Ordering::SeqCst) {
            return Ok(None);
        }
        Ok(Some(self.leftovers.lock().unwrap_or_else(|e| e.into_inner()).clone()))
    }

    async fn destroy(&self, _: &Machine) -> Result<()> {
        self.check().await?;
        while self.hang_destroy.load(Ordering::SeqCst) {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        self.note("destroy".to_owned());
        self.set_power(Power::Absent);
        Ok(())
    }

    async fn screen_up(&self, _: &Machine, bot: &str) -> Result<Screen> {
        self.awake()?;
        if let Some(p) = self.screen_problem.lock().unwrap_or_else(|e| e.into_inner()).clone() {
            return Err(p);
        }
        self.note(format!("screen_up {bot}"));
        let mut screens = self.screens.lock().unwrap_or_else(|e| e.into_inner());
        let at = screens.iter().position(|b| b == bot).unwrap_or_else(|| {
            screens.push(bot.to_owned());
            screens.len() - 1
        });
        let n = at + 2;
        Ok(Screen { display: format!(":{n}"), viewer_port: 6900 + n as u16, folder: format!("agents/{bot}"), profile: None })
    }

    async fn screen_down(&self, _: &Machine, bot: &str) -> Result<()> {
        self.note(format!("screen_down {bot}"));
        self.screens.lock().unwrap_or_else(|e| e.into_inner()).retain(|b| b != bot);
        Ok(())
    }

    async fn screens(&self, _: &Machine) -> Result<Vec<String>> {
        Ok(self.screens.lock().unwrap_or_else(|e| e.into_inner()).clone())
    }

    async fn recover(&self, spec: &Spec, last: Option<&Reach>) -> Result<Option<Machine>> {
        self.note(format!("recover {} {last:?}", spec.id));
        let found = self.recoverable.lock().unwrap_or_else(|e| e.into_inner()).clone();
        Ok(found.map(|reach| Machine {
            provider: ProviderKind::Docker,
            reach,
            image: Some("mock:1".to_owned()),
            token: Secret::new(TOKEN),
            viewer_password: Secret::new(PASSWORD),
            dir: spec.dir.clone(),
            link: None,
            seen: None,
        }))
    }

    fn cant_keep_lost(&self) -> Option<String> {
        self.cant_keep.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    async fn sweep(&self, id: &str, _: Option<&Reach>) -> Result<()> {
        self.note(format!("sweep {id}"));
        Ok(())
    }

    async fn guard(&self, _: &Machine) -> Result<()> {
        self.note("guard".to_owned());
        match self.unsafe_now.lock().unwrap_or_else(|e| e.into_inner()).clone() {
            Some(p) => {
                self.set_power(Power::Asleep);
                Err(p)
            }
            None => Ok(()),
        }
    }

    async fn resolve(&self, _: &Machine, path: &str) -> Result<String> {
        let links = self.links.lock().unwrap_or_else(|e| e.into_inner());
        Ok(links.get(path).cloned().unwrap_or_else(|| format!("/workspace/{path}")))
    }

    async fn viewer(&self, machine: &Machine, screen: &Screen) -> Result<Option<Viewer>> {
        self.awake()?;
        let addr = self.viewer_addr.lock().unwrap_or_else(|e| e.into_inner()).clone();
        Ok(Some(Viewer { addr: addr.unwrap_or_else(|| format!("127.0.0.1:{}", screen.viewer_port)), password: machine.viewer_password.clone(), far: None }))
    }

    async fn screenshot(&self, _: &Machine, screen: &Screen) -> Result<Screenshot> {
        self.awake()?;
        self.note(format!("screenshot {}", screen.display));
        Ok(Screenshot { png: tiny_png(), width: 2, height: 2 })
    }

    async fn input(&self, _: &Machine, screen: &Screen, input: &Input) -> Result<()> {
        self.awake()?;
        self.note(format!("input {} {input:?}", screen.display));
        Ok(())
    }

    async fn run(&self, _: &Machine, screen: &Screen, command: &str, _: u64) -> Result<CommandOutput> {
        self.awake()?;
        self.note(format!("run {} {command}", screen.display));
        while self.hold_run.lock().unwrap_or_else(|e| e.into_inner()).as_ref().is_some_and(|word| command.contains(word.as_str())) {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        let reply = self.replies.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|(word, _)| command.contains(word.as_str())).map(|(_, said)| said.clone());
        let stdout = match reply {
            Some(said) => said?,
            None => self.stdout.lock().unwrap_or_else(|e| e.into_inner()).clone(),
        };
        let failed = self.fails.lock().unwrap_or_else(|e| e.into_inner()).as_ref().is_some_and(|word| command.contains(word.as_str()));
        Ok(CommandOutput { exit_code: Some(i32::from(failed)), stdout, ..CommandOutput::default() })
    }

    async fn open_url(&self, _: &Machine, screen: &Screen, url: &str) -> Result<()> {
        self.awake()?;
        self.note(format!("open {} {url}", screen.display));
        Ok(())
    }

    async fn read_file(&self, _: &Machine, path: &str) -> Result<Vec<u8>> {
        self.awake()?;
        self.note(format!("read {path}"));
        Ok(self.stdout.lock().unwrap_or_else(|e| e.into_inner()).clone().into_bytes())
    }

    async fn write_file(&self, _: &Machine, path: &str, content: &[u8]) -> Result<()> {
        self.awake()?;
        self.note(format!("write {path} {}", content.len()));
        Ok(())
    }

    async fn record_start(&self, _: &Machine, _: &Screen) -> Result<u64> {
        self.awake()?;
        self.note("record_start".to_owned());
        while self.hold_record.load(Ordering::SeqCst) {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        Ok(7)
    }

    async fn users_apart(&self, _: &Machine, _: &[String]) -> Result<bool> {
        self.note("users_apart".to_owned());
        match self.apart.lock().unwrap_or_else(|e| e.into_inner()).clone() {
            Some(answer) => answer,
            None => Ok(false),
        }
    }

    async fn clear_strays(&self, _: &Machine, _: &Screen) -> Result<Vec<String>> {
        if let Some(p) = self.stray_problem.lock().unwrap_or_else(|e| e.into_inner()).clone() {
            return Err(p);
        }
        self.note("clear_strays".to_owned());
        Ok(std::mem::take(&mut *self.strays.lock().unwrap_or_else(|e| e.into_inner())))
    }

    async fn record_stop(&self, _: &Machine, screen: &Screen, id: u64) -> Result<String> {
        self.awake()?;
        if let Some(p) = self.record_problem.lock().unwrap_or_else(|e| e.into_inner()).clone() {
            return Err(p);
        }
        self.note(format!("record_stop {id}"));
        Ok(format!("{}/recordings/{id}.mp4", screen.folder))
    }
}
