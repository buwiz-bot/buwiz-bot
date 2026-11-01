//! No computer here: Docker is not installed, or installed and not running. Every call fails
//! with [`BoxError::Unavailable`] carrying the reason, so the pane shows "Docker is not
//! running" instead of a stack of inspect failures — and nothing panics.

use async_trait::async_trait;

use crate::docker::DockerComputer;
use crate::{
    BoxError, BoxResult, CommandOutput, Computer, CreateBox, EgressTunnel, ImageStatus, Screen,
    Screenshot, StartedCommand,
};

/// Why there is no computer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unavailability {
    /// No `docker` on PATH.
    NotInstalled,
    /// `docker` is there; the daemon did not answer.
    NotRunning(String),
}

impl Unavailability {
    /// The sentence the pane shows.
    pub fn message(&self) -> String {
        match self {
            Self::NotInstalled => "Docker is not installed: no `docker` on PATH".to_string(),
            Self::NotRunning(detail) if detail.is_empty() => "Docker is not running".to_string(),
            Self::NotRunning(detail) => format!("Docker is not running: {detail}"),
        }
    }

    pub fn error(&self) -> BoxError {
        BoxError::Unavailable(self.message())
    }
}

/// A [`Computer`] that is not there. Holds the reason so every answer says it.
#[derive(Debug, Clone)]
pub struct NoComputer {
    pub reason: Unavailability,
}

impl NoComputer {
    pub fn new(reason: Unavailability) -> Self {
        Self { reason }
    }

    fn refuse<T>(&self) -> BoxResult<T> {
        Err(self.reason.error())
    }
}

#[async_trait]
impl Computer for NoComputer {
    async fn create(&self, _: CreateBox) -> BoxResult<String> {
        self.refuse()
    }
    async fn run(&self, _: &str, _: &str, _: Option<u64>) -> BoxResult<CommandOutput> {
        self.refuse()
    }
    async fn start(&self, _: &str, _: &str) -> BoxResult<StartedCommand> {
        self.refuse()
    }
    async fn watch(&self, _: &str, _: &str) -> BoxResult<StartedCommand> {
        self.refuse()
    }
    async fn read_file(&self, _: &str, _: &str) -> BoxResult<String> {
        self.refuse()
    }
    async fn write_file_bytes(&self, _: &str, _: &str, _: &[u8]) -> BoxResult<()> {
        self.refuse()
    }
    async fn expose_port(&self, _: &str, _: u16) -> BoxResult<String> {
        self.refuse()
    }
    async fn stop(&self, _: &str) -> BoxResult<()> {
        self.refuse()
    }
    async fn resume(&self, _: &str) -> BoxResult<()> {
        self.refuse()
    }
    async fn destroy(&self, _: &str) -> BoxResult<()> {
        self.refuse()
    }
    async fn discard(&self, _: &str) -> BoxResult<()> {
        self.refuse()
    }
    async fn state(&self, _: &str) -> BoxResult<String> {
        self.refuse()
    }
    async fn has_screen(&self, _: &str) -> BoxResult<bool> {
        self.refuse()
    }
    async fn screen_url(&self, _: &str, _: &Screen) -> BoxResult<String> {
        self.refuse()
    }
    async fn screenshot(&self, _: &str, _: &Screen) -> BoxResult<Screenshot> {
        self.refuse()
    }
    async fn image_status(&self, _: Option<&str>) -> BoxResult<ImageStatus> {
        self.refuse()
    }
    async fn pull_latest(&self) -> BoxResult<ImageStatus> {
        self.refuse()
    }
    async fn recreate(&self, _: &str, _: CreateBox) -> BoxResult<String> {
        self.refuse()
    }
    async fn egress_intent(&self, _: &str) -> BoxResult<Option<bool>> {
        self.refuse()
    }
    async fn egress_probe(&self, _: &str) -> (Option<EgressTunnel>, bool) {
        (None, false)
    }
    async fn generation(&self, _: &str) -> u64 {
        0
    }
}

/// The local Docker computer if Docker answers right now, else a [`NoComputer`] that says
/// why. The hub calls this once at startup (and again when the pane asks to retry); a daemon
/// that goes away later surfaces as the same [`BoxError::Unavailable`] from each call.
pub async fn local_docker() -> Box<dyn Computer> {
    let docker = DockerComputer::from_env();
    match docker.availability().await {
        Ok(()) => Box::new(docker),
        Err(reason) => Box::new(NoComputer::new(reason)),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn every_call_says_why_there_is_no_computer() {
        let none = NoComputer::new(Unavailability::NotRunning(
            "Cannot connect to the Docker daemon".into(),
        ));
        let expected = BoxError::Unavailable(
            "Docker is not running: Cannot connect to the Docker daemon".into(),
        );
        assert_eq!(
            none.create(CreateBox::default()).await,
            Err(expected.clone())
        );
        assert_eq!(none.run("b", "echo hi", None).await, Err(expected.clone()));
        assert_eq!(none.state("b").await, Err(expected.clone()));
        assert_eq!(
            none.screenshot("b", &Screen::Shared).await,
            Err(expected.clone())
        );
        assert_eq!(none.pull_latest().await, Err(expected.clone()));
        assert_eq!(none.egress_probe("b").await, (None, false));
        assert_eq!(expected.code(), "provider_unreachable");
        assert_eq!(
            Unavailability::NotInstalled.message(),
            "Docker is not installed: no `docker` on PATH"
        );
        assert_eq!(
            Unavailability::NotRunning(String::new()).message(),
            "Docker is not running"
        );
    }
}
