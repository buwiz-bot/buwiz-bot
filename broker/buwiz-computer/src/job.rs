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
//! A job is its process group, not `run.sh`: that ends with its command, and what the command left
//! in the background is still the job, until a stop takes the whole group.
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
/// for a `cancel` mark: a [`cancel`] either finds that pid or leaves a mark this finds. `boot` goes
/// first, so a pid always has one.
pub fn script(id: &str, command: &str) -> String {
    let (dir, command) = (format!("{FOLDER}/{id}"), quoted(command));
    format!("#!/bin/sh\nps -o lstart= -p 1 > {dir}/boot 2>/dev/null\necho $$ > {dir}/pid\n[ -e {dir}/cancel ] && exit 0\n/bin/sh -c {command} > {dir}/out 2>&1 < /dev/null\ncode=$?\necho \"$code\" > {dir}/exit.new && mv {dir}/exit.new {dir}/exit\n")
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

/// Whether `$p` is still this job's group, on the boot of the computer it started on: after a
/// restart the same number can be anyone's, and a `boot` that doesn't match says so. A job from
/// before `boot` has none, and counts.
fn ours(id: &str) -> String {
    format!("{{ b=$(cat {FOLDER}/{id}/boot 2>/dev/null); [ -z \"$b\" ] || [ \"$b\" = \"$(ps -o lstart= -p 1 2>/dev/null)\" ]; }}")
}

/// Whether group `$p` has a live member. Its leader may be long gone: `run.sh` ends with its
/// command, and what that left running goes on. One that ended and wasn't reaped doesn't count.
const GROUP: &str = r#"ps -A -o pgid= -o stat= 2>/dev/null | awk -v p="$p" '$1 == p && $2 !~ /^Z/ { f = 1 } END { exit !f }'"#;

/// One line for [`read_look`]. A job is over when its group is: a command that left something
/// running in the background hasn't ended, whatever `exit` says, so its limit and its stop still
/// reach what's left. Its code is read once that has gone too.
pub fn look(id: &str) -> String {
    let dir = format!("{FOLDER}/{id}");
    let ours = ours(id);
    format!("p=$(cat {dir}/pid 2>/dev/null)\nif [ -n \"$p\" ] && {ours} && {GROUP}; then echo running\nelif [ -s {dir}/exit ]; then echo \"exit $(cat {dir}/exit)\"\nelse echo gone\nfi")
}

/// How a job stands, from what [`look`] printed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Seen {
    Running,
    /// it ended by itself, with this code, and nothing of it is left
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

/// Ends the job's whole group, its leader there or not: about 2 seconds for the group to finish,
/// then for good. A zero exit code says the group has no member left.
pub fn stop(id: &str) -> String {
    let dir = format!("{FOLDER}/{id}");
    let ours = ours(id);
    format!(
        "p=$(cat {dir}/pid 2>/dev/null)\n\
         case \"$p\" in ''|*[!0-9]*) exit 0;; esac\n\
         {ours} || exit 0\n\
         {GROUP} || exit 0\n\
         kill -TERM \"-$p\" 2>/dev/null\n\
         i=0; while [ \"$i\" -lt 20 ] && {GROUP}; do sleep 0.1; i=$((i+1)); done\n\
         kill -KILL \"-$p\" 2>/dev/null\n\
         i=0; while [ \"$i\" -lt 20 ] && {GROUP}; do sleep 0.1; i=$((i+1)); done\n\
         {GROUP} && exit 1\n\
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

    /// A command that left something behind: `run.sh` wrote its code and went, and that's no end.
    #[test]
    fn what_a_job_left_in_the_background_is_still_the_job() {
        let dir = folder("left", "sleep 300 & echo $! > child; echo started");
        started(&dir);
        let began = Instant::now();
        while !dir.join(FOLDER).join(ID).join("exit").exists() {
            assert!(began.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(seen(&dir), Some(Seen::Running), "its child runs on");
        let child = std::fs::read_to_string(dir.join("child")).unwrap();
        assert_eq!(sh(&dir, &cancel(ID)).status.code(), Some(0));
        assert!(!sh(&dir, &format!("ps -o pid= -p {}", child.trim())).status.success(), "a cancel reaches it");
        assert_eq!(seen(&dir), Some(Seen::Exited(0)), "and with nothing left, its code stands");
    }

    /// Leaving the group: a child that can't be signalled keeps a stop from saying it worked.
    #[test]
    fn a_stop_says_so_when_a_member_outlives_it() {
        let dir = folder("outlive", "sleep 300");
        started(&dir);
        until(&dir, Seen::Running);
        // `kill` that can't kill: the group goes on, so the answer can't be that it stopped
        let line = stop(ID).replace("kill -KILL", "true").replace("kill -TERM", "true").replace("-lt 20", "-lt 1");
        assert_eq!(sh(&dir, &line).status.code(), Some(1));
        assert_eq!(sh(&dir, &stop(ID)).status.code(), Some(0));
    }

    /// TERM ends `run.sh` at once: the grace is for the group, or a child gets none.
    #[test]
    fn a_child_gets_its_grace_after_the_leader_has_gone() {
        let dir = folder("grace", "sh -c 'trap \"sleep 1; echo graceful > g.txt; exit 0\" TERM; while :; do sleep 0.1; done' & wait");
        started(&dir);
        until(&dir, Seen::Running);
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(sh(&dir, &stop(ID)).status.code(), Some(0));
        assert!(dir.join("g.txt").exists(), "killed before it could finish");
    }

    /// After a restart its number can be anyone's: a look or a stop leaves that alone.
    #[test]
    fn a_group_from_another_boot_isnt_the_job() {
        let dir = folder("boot", "sleep 300");
        started(&dir);
        until(&dir, Seen::Running);
        let boot = dir.join(FOLDER).join(ID).join("boot");
        let real = std::fs::read(&boot).unwrap();
        assert!(!real.is_empty());
        std::fs::write(&boot, "Thu Jan  1 00:00:00 1970").unwrap();
        assert_eq!(seen(&dir), Some(Seen::Gone));
        assert_eq!(sh(&dir, &stop(ID)).status.code(), Some(0));
        std::fs::write(&boot, real).unwrap();
        assert_eq!(seen(&dir), Some(Seen::Running), "it wasn't touched");
        assert_eq!(sh(&dir, &stop(ID)).status.code(), Some(0));
    }

    /// A job that was running when this came in has no `boot`: it's still found, and stopped.
    #[test]
    fn a_job_from_before_boot_still_counts() {
        let dir = folder("noboot", "sleep 300");
        started(&dir);
        until(&dir, Seen::Running);
        std::fs::remove_file(dir.join(FOLDER).join(ID).join("boot")).unwrap();
        assert_eq!(seen(&dir), Some(Seen::Running));
        assert_eq!(sh(&dir, &stop(ID)).status.code(), Some(0));
        assert_eq!(seen(&dir), Some(Seen::Gone));
    }

    #[test]
    fn a_pid_with_no_group_isnt_the_job() {
        let dir = folder("pid", "true");
        std::fs::write(dir.join(FOLDER).join(ID).join("pid"), "999999").unwrap();
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
