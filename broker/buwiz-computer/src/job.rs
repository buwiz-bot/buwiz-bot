//! A job: a command that goes on after the call that started it, as the words a computer is sent.
//!
//! It's plain shell through [`crate::Provider::run`], in the Bot's own folder, so a box and an SSH
//! host do it the same way and no image changes. `.jobs/<id>/` holds `run.sh`, which the broker
//! writes, and the files it leaves: `pid`, `out` and `exit`. The job is a session of its own, so
//! nothing that ends the command that started it ends the job, and its files are on the computer,
//! so a broker that restarts finds it where it was.
//!
//! `box-exec`'s own `detach` won't do: it's capped at 10 minutes, and it keeps its state in memory.
//!
//! What's in those files is the job's own to write. A model can lie in them about its own job, and
//! everything here runs as that Bot's user, so a false `pid` reaches nothing that user couldn't.

/// The folder under a Bot's own that keeps its jobs.
pub const FOLDER: &str = ".jobs";

/// `run.sh`'s path in the Bot's folder.
pub fn script_path(id: &str) -> String {
    format!("{FOLDER}/{id}/run.sh")
}

fn quoted(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// What `run.sh` holds. The command gets a shell of its own, so nothing it says can reach the lines
/// that record how it ended. `exit` appears whole or not at all. Its pid is written before it looks
/// for a `cancel` mark: a [`cancel`] either finds that pid or leaves a mark this finds.
pub fn script(id: &str, command: &str) -> String {
    let (dir, command) = (format!("{FOLDER}/{id}"), quoted(command));
    format!("#!/bin/sh\necho $$ > {dir}/pid\n[ -e {dir}/cancel ] && exit 0\n/bin/sh -c {command} > {dir}/out 2>&1 < /dev/null\ncode=$?\necho \"$code\" > {dir}/exit.new && mv {dir}/exit.new {dir}/exit\n")
}

/// Starts `run.sh` and answers with its pid once it has written one. A host with no `setsid`, as a
/// Mac is, gets a process group of its own from job control instead.
pub fn start(id: &str) -> String {
    let (dir, run) = (format!("{FOLDER}/{id}"), script_path(id));
    format!(
        "if command -v setsid >/dev/null 2>&1; then setsid nohup /bin/sh {run} </dev/null >/dev/null 2>&1 &\n\
         else set -m; nohup /bin/sh {run} </dev/null >/dev/null 2>&1 &\n\
         fi\n\
         i=0; while [ ! -s {dir}/pid ] && [ \"$i\" -lt 50 ]; do sleep 0.1; i=$((i+1)); done\n\
         cat {dir}/pid"
    )
}

/// Whether `$p` is still this job: a pid alone can be another process's after a restart. A
/// finished one that nobody reaped has lost its arguments, so it doesn't count either.
fn alive(id: &str) -> String {
    format!("ps -o args= -p \"$p\" 2>/dev/null | grep -qF {}", script_path(id))
}

/// One line for [`read_look`].
pub fn look(id: &str) -> String {
    let dir = format!("{FOLDER}/{id}");
    let alive = alive(id);
    format!("p=$(cat {dir}/pid 2>/dev/null)\nif [ -s {dir}/exit ]; then echo \"exit $(cat {dir}/exit)\"\nelif [ -n \"$p\" ] && {alive}; then echo running\nelse echo gone\nfi")
}

/// How a job stands, from what [`look`] printed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Seen {
    Running,
    /// it ended by itself, with this code
    Exited(i32),
    /// it isn't there and said nothing: a restart of the computer, or a kill from inside it
    Gone,
}

/// `None` for anything else: not an answer to go on.
pub fn read_look(stdout: &str) -> Option<Seen> {
    let line = stdout.trim();
    match line {
        "running" => Some(Seen::Running),
        "gone" => Some(Seen::Gone),
        _ => line.strip_prefix("exit ").map(|code| code.trim().parse().map_or(Seen::Gone, Seen::Exited)),
    }
}

/// Ends the job's whole group: a moment to finish, then for good.
pub fn stop(id: &str) -> String {
    let dir = format!("{FOLDER}/{id}");
    let alive = alive(id);
    format!(
        "p=$(cat {dir}/pid 2>/dev/null)\n\
         case \"$p\" in ''|*[!0-9]*) exit 0;; esac\n\
         {alive} || exit 0\n\
         kill -TERM \"-$p\" 2>/dev/null\n\
         i=0; while [ \"$i\" -lt 20 ] && {alive}; do sleep 0.1; i=$((i+1)); done\n\
         kill -KILL \"-$p\" 2>/dev/null\n\
         exit 0"
    )
}

/// Ends the job for good, started or not: a `run.sh` that hasn't run yet finds the mark and does
/// nothing. For a start nobody knows the end of, and a job its Bot left. A zero exit code is
/// the only word that the mark is there.
pub fn cancel(id: &str) -> String {
    let dir = format!("{FOLDER}/{id}");
    format!("mkdir -p {dir} && : > {dir}/cancel || exit 1\n{}", stop(id))
}

/// The last `bytes` of what it printed.
pub fn tail(id: &str, bytes: usize) -> String {
    format!("tail -c {bytes} {FOLDER}/{id}/out 2>/dev/null")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::process::{Command, Output};
    use std::time::{Duration, Instant};

    use super::*;

    const ID: &str = "j_test";

    /// The way a provider runs a line: `/bin/sh -c`, in the Bot's folder.
    fn sh(dir: &Path, line: &str) -> Output {
        Command::new("/bin/sh").args(["-c", line]).current_dir(dir).output().unwrap()
    }

    fn folder(label: &str, command: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("buwiz-job-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(FOLDER).join(ID)).unwrap();
        std::fs::write(dir.join(script_path(ID)), script(ID, command)).unwrap();
        dir
    }

    fn seen(dir: &Path) -> Option<Seen> {
        read_look(&String::from_utf8_lossy(&sh(dir, &look(ID)).stdout))
    }

    fn until(dir: &Path, want: Seen) {
        let began = Instant::now();
        while seen(dir) != Some(want) {
            assert!(began.elapsed() < Duration::from_secs(10), "it never got to {want:?}: {:?}", seen(dir));
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn started(dir: &Path) -> i32 {
        let out = sh(dir, &start(ID));
        assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).trim().parse().unwrap()
    }

    /// The real words under this machine's own shell: a Mac has no `setsid`, Linux does.
    #[test]
    fn a_job_outlives_its_command_and_says_how_it_ended() {
        let dir = folder("ends", "sleep 1; echo \"it's done\"; echo oops >&2; exit 3");
        started(&dir);
        assert_eq!(seen(&dir), Some(Seen::Running), "the command that started it has already gone");
        until(&dir, Seen::Exited(3));
        assert_eq!(String::from_utf8_lossy(&sh(&dir, &tail(ID, 100)).stdout), "it's done\noops\n");
    }

    #[test]
    fn a_stop_ends_what_the_job_started_too() {
        let dir = folder("stop", "sleep 300 & echo $! > child; wait");
        started(&dir);
        until(&dir, Seen::Running);
        let began = Instant::now();
        while !dir.join("child").exists() {
            assert!(began.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(20));
        }
        let child = std::fs::read_to_string(dir.join("child")).unwrap();
        assert_eq!(sh(&dir, &stop(ID)).status.code(), Some(0));
        assert_eq!(seen(&dir), Some(Seen::Gone), "killed, it leaves no code");
        assert!(!sh(&dir, &format!("ps -o pid= -p {}", child.trim())).status.success(), "its child went with it");
    }

    /// A start that timed out can still run: what comes after the mark runs nothing.
    #[test]
    fn a_cancelled_job_that_starts_late_runs_nothing() {
        let dir = folder("late", "echo ran > ran.txt");
        assert_eq!(sh(&dir, &cancel(ID)).status.code(), Some(0));
        started(&dir);
        std::thread::sleep(Duration::from_millis(300));
        assert!(!dir.join("ran.txt").exists());
        assert_eq!(seen(&dir), Some(Seen::Gone));
    }

    #[test]
    fn a_cancel_ends_a_job_already_going() {
        let dir = folder("cancel", "sleep 300");
        started(&dir);
        until(&dir, Seen::Running);
        assert_eq!(sh(&dir, &cancel(ID)).status.code(), Some(0));
        assert_eq!(seen(&dir), Some(Seen::Gone));
    }

    #[test]
    fn a_cancel_that_cant_leave_its_mark_says_so() {
        let dir = folder("nomark", "true");
        std::fs::remove_dir_all(dir.join(FOLDER)).unwrap();
        std::fs::write(dir.join(FOLDER), "a file where the folder goes").unwrap();
        assert_ne!(sh(&dir, &cancel(ID)).status.code(), Some(0));
    }

    #[test]
    fn a_pid_that_is_another_process_now_isnt_the_job() {
        let dir = folder("pid", "true");
        std::fs::write(dir.join(FOLDER).join(ID).join("pid"), std::process::id().to_string()).unwrap();
        assert_eq!(seen(&dir), Some(Seen::Gone));
        assert_eq!(sh(&dir, &stop(ID)).status.code(), Some(0));
    }

    #[test]
    fn nothing_but_an_answer_counts_as_one() {
        assert_eq!(read_look("running\n"), Some(Seen::Running));
        assert_eq!(read_look("exit 0\n"), Some(Seen::Exited(0)));
        assert_eq!(read_look("exit hello\n"), Some(Seen::Gone), "a garbled code is no code");
        assert_eq!(read_look(""), None);
        assert_eq!(read_look("sh: ps: not found"), None);
    }
}
