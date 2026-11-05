//! A provider that runs nothing, for the broker's tests: a computer is a word in memory, and
//! every call is written down.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use async_trait::async_trait;

use crate::{CommandOutput, Input, Machine, Power, Problem, Provider, ProviderKind, Reach, Result, Screen, Screenshot, Secret, Sleep, Spec, Viewer};

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
    /// `wake` waits here until released, when set
    pub hold: AtomicBool,
    /// what `commands_stay_on_their_screen` says
    pub separate: AtomicBool,
    /// the Bots with a screen, in the order they first got one
    pub screens: Mutex<Vec<String>>,
}

impl Default for MockProvider {
    fn default() -> Self {
        Self { power: Mutex::new(Power::Absent), calls: Mutex::new(Vec::new()), problem: Mutex::new(None), stdout: Mutex::new(String::new()), hold: AtomicBool::new(false), separate: AtomicBool::new(false), screens: Mutex::new(Vec::new()) }
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
        self.note(format!("create {}", spec.id));
        self.set_power(Power::Asleep);
        Ok(Machine {
            provider: ProviderKind::Docker,
            reach: Reach::Container { name: format!("mock-{}", spec.id) },
            image: Some("mock:1".to_owned()),
            token: Secret::new(TOKEN),
            viewer_password: Secret::new(PASSWORD),
            dir: spec.dir.clone(),
        })
    }

    async fn power(&self, _: &Machine) -> Result<Power> {
        Ok(self.power.lock().unwrap_or_else(|e| e.into_inner()).clone())
    }

    async fn wake(&self, _: &Machine) -> Result<()> {
        self.check().await?;
        self.note("wake".to_owned());
        while self.hold.load(Ordering::SeqCst) {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        self.set_power(Power::Awake);
        Ok(())
    }

    async fn sleep(&self, _: &Machine, depth: Sleep) -> Result<()> {
        self.note(format!("sleep {depth:?}"));
        self.set_power(Power::Asleep);
        Ok(())
    }

    async fn destroy(&self, _: &Machine) -> Result<()> {
        self.note("destroy".to_owned());
        self.set_power(Power::Absent);
        Ok(())
    }

    async fn screen_up(&self, _: &Machine, bot: &str) -> Result<Screen> {
        self.awake()?;
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
        Ok(())
    }

    async fn commands_stay_on_their_screen(&self, _: &Machine) -> bool {
        self.separate.load(Ordering::SeqCst)
    }

    async fn viewer(&self, machine: &Machine, screen: &Screen) -> Result<Option<Viewer>> {
        self.awake()?;
        Ok(Some(Viewer { addr: format!("127.0.0.1:{}", screen.viewer_port), password: machine.viewer_password.clone() }))
    }

    async fn screenshot(&self, _: &Machine, screen: &Screen) -> Result<Screenshot> {
        self.awake()?;
        self.note(format!("screenshot {}", screen.display));
        Ok(Screenshot { png: b"\x89PNG-mock".to_vec(), width: 1280, height: 800 })
    }

    async fn input(&self, _: &Machine, screen: &Screen, input: &Input) -> Result<()> {
        self.awake()?;
        self.note(format!("input {} {input:?}", screen.display));
        Ok(())
    }

    async fn run(&self, _: &Machine, screen: &Screen, command: &str, _: u64) -> Result<CommandOutput> {
        self.awake()?;
        self.note(format!("run {} {command}", screen.display));
        Ok(CommandOutput { exit_code: Some(0), stdout: self.stdout.lock().unwrap_or_else(|e| e.into_inner()).clone(), ..CommandOutput::default() })
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
        Ok(7)
    }

    async fn record_stop(&self, _: &Machine, screen: &Screen, id: u64) -> Result<String> {
        self.awake()?;
        Ok(format!("{}/recordings/{id}.mp4", screen.folder))
    }
}
