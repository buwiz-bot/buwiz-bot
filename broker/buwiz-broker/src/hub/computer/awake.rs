//! Keeping the Mac awake while a job runs, and only then: a held `caffeinate -s -w <this pid>`.
//!
//! `-s` is a system sleep assertion, which the system honours on AC power alone. On battery it
//! holds nothing: the child stays and the Mac sleeps as it would have, and back on mains the same
//! assertion counts again. So "on mains only" isn't a check here, and needs none. It keeps the
//! system up and lets the display sleep. `-w` ends it with this broker however that goes, a
//! kill included, so nothing outlives what asked for it.
//!
//! Nothing here reads the power source or changes a setting of the Mac's. A system that has no
//! `caffeinate` has no program here, and a job runs on with the machine's own sleep settings.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;

const CAFFEINATE: &str = "/usr/bin/caffeinate";

pub struct KeepAwake {
    /// no program, and it holds nothing: other systems, and every test that doesn't name one
    program: Option<PathBuf>,
    held: Mutex<Option<Child>>,
}

impl KeepAwake {
    pub fn new() -> Self {
        Self { program: cfg!(all(target_os = "macos", not(test))).then(|| PathBuf::from(CAFFEINATE)), held: Mutex::new(None) }
    }

    /// With this program in `caffeinate`'s place (tests).
    #[cfg(test)]
    pub fn with(program: PathBuf) -> Self {
        Self { program: Some(program), held: Mutex::new(None) }
    }

    /// Held when `wanted`, let go when not. One that died by itself is started again.
    pub fn set(&self, wanted: bool) {
        let Some(program) = &self.program else { return };
        let mut held = self.held.lock().unwrap_or_else(|e| e.into_inner());
        if held.as_mut().is_some_and(|c| !matches!(c.try_wait(), Ok(None))) {
            *held = None;
        }
        match (wanted, held.take()) {
            (true, None) => match Command::new(program).args(["-s", "-w", &std::process::id().to_string()]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() {
                Ok(child) => *held = Some(child),
                Err(e) => eprintln!("buwiz-broker: keeping the Mac awake: {e}"),
            },
            (true, Some(child)) => *held = Some(child),
            (false, Some(mut child)) => {
                let _ = child.kill();
                let _ = child.wait();
            }
            (false, None) => {}
        }
    }
}

impl Drop for KeepAwake {
    fn drop(&mut self) {
        self.set(false);
    }
}
