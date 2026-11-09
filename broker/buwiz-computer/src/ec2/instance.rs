//! What runs on the instance, over the SSH connection, as root: the scripts, and the container
//! they make. Nothing here is a secret: the token and the viewer password reach [`START`] on its
//! stdin, never its argv, and go to a root-only tmpfs folder the box reads them from.
//!
//! The box is the same image, hardened as on a Mac (`docker.rs`) and as the image's
//! `docker-compose.yml`: every capability dropped but the eight its root needs, no new privileges,
//! limits on pids and memory, a 1 GiB `/dev/shm`, IPv6 off, ports below 6906 kept for its services,
//! and its ports published on the instance's loopback alone, where only the SSH connection reaches
//! them. Its files are two named volumes on the root EBS volume, which hibernation keeps too.
//!
//! **The wall.** On a Mac the wall keeps a box off the Mac. Here it keeps the box off what is the
//! instance's and the VPC's, from the host's own netfilter, which nothing in the box can touch:
//! the box's bridge may not reach a private, link-local or reserved address (the VPC, its peers,
//! and the instance metadata service with its credentials, which IMDSv2's hop limit of one closes
//! too), and may open nothing on the instance itself. DNS goes to Amazon's resolver at
//! 169.254.169.253, the one address let through, and only on port 53. It goes up before every start,
//! and the box only starts once it is.

use crate::docker::{BOX_CAPS, EXEC_PORT, HOST_PORT, USERS_LABEL, USERS_VERSION};
use crate::remote::ssh::quoted;

pub const NAME: &str = "buwiz-computer";
const NET: &str = "buwiz-computer-net";
const BRIDGE: &str = "buwiz0";
const SECRETS: &str = "/run/buwiz-computer";
const TOKEN_FILE: &str = "token";
const VIEWER_FILE: &str = "viewer";
const AMAZON_DNS: &str = "169.254.169.253";
/// Below this a port takes a capability, inside the box (`docker.rs`).
const FIRST_OPEN_PORT: u16 = 6906;
/// Of the instance's 8 GiB: the rest is the host's, Docker's and the page cache's.
const MEMORY: &str = "6g";
/// Not the open internet: the box may not reach these from its bridge.
const CLOSED: [&str; 9] = ["0.0.0.0/8", "10.0.0.0/8", "100.64.0.0/10", "169.254.0.0/16", "172.16.0.0/12", "192.0.0.0/24", "192.168.0.0/16", "198.18.0.0/15", "240.0.0.0/4"];

/// What each script is called, first in its text: a log of the instance's `sudo` shows which ran.
pub const PREPARE_MARK: &str = "# buwiz:prepare";
pub const START_MARK: &str = "# buwiz:start";
pub const CLEAR_MARK: &str = "# buwiz:clear";
pub const STOP_MARK: &str = "# buwiz:stop";
pub const INSIDE_MARK: &str = "# buwiz:inside";

/// The line `ssh` runs: `$1` under root's `/bin/sh`, the rest its arguments. `sudo -n`, so a login
/// that would ask for a password fails instead of waiting.
pub const AS_ROOT: &str = r#"s=$1; shift; exec sudo -n /bin/sh -c "$s" sh "$@""#;

/// Waits out the first boot's cloud-init, then prints the box's state: `running`, `exited`,
/// `created`, … or `absent`. No Docker is `nodocker`.
pub fn prepare() -> String {
    format!(
        r#"{PREPARE_MARK}
if command -v cloud-init >/dev/null 2>&1; then cloud-init status --wait >/dev/null 2>&1 || true; fi
command -v docker >/dev/null 2>&1 || {{ echo nodocker; exit 0; }}
systemctl is-active --quiet docker || systemctl start docker >/dev/null 2>&1 || {{ echo nodocker; exit 0; }}
docker inspect -f '{{{{.State.Status}}}}' {NAME} 2>/dev/null || echo absent"#
    )
}

/// The `docker create` words for the box, from `image`, for computer `id`.
pub fn create_args(image: &str, id: &str) -> Vec<String> {
    let mut args: Vec<String> = ["docker", "create", "--name", NAME, "--network", NET, "--dns", AMAZON_DNS, "--label", "app.buwiz.computer=1", "--label"].map(str::to_owned).to_vec();
    args.push(format!("app.buwiz.computer.id={id}"));
    for a in ["--user", "0", "--cap-drop", "ALL", "--security-opt", "no-new-privileges", "--pids-limit", "2048", "--memory", MEMORY, "--shm-size", "1g", "--sysctl", "net.ipv6.conf.all.disable_ipv6=1", "--sysctl"] {
        args.push(a.to_owned());
    }
    args.push(format!("net.ipv4.ip_unprivileged_port_start={FIRST_OPEN_PORT}"));
    for cap in BOX_CAPS {
        args.push("--cap-add".to_owned());
        args.push(cap.to_owned());
    }
    for port in [EXEC_PORT, HOST_PORT] {
        args.push("-p".to_owned());
        args.push(format!("127.0.0.1:{port}:{port}"));
    }
    args.push("-p".to_owned());
    args.push("127.0.0.1:6901-6905:6901-6905".to_owned());
    for (source, target) in [("buwiz-computer-workspace", "/workspace"), ("buwiz-computer-home", "/home/box")] {
        args.push("--mount".to_owned());
        args.push(format!("type=volume,source={source},target={target}"));
    }
    args.push("--mount".to_owned());
    args.push(format!("type=bind,source={SECRETS},target={SECRETS},readonly"));
    for env in [format!("BOX_TOKEN_FILE={SECRETS}/{TOKEN_FILE}"), format!("BOX_VIEWER_PASSWORD_FILE={SECRETS}/{VIEWER_FILE}"), "BOX_DESKTOP=1".to_owned(), "BOX_DESKTOP_REQUIRED=1".to_owned(), "BOX_CHROME=0".to_owned()] {
        args.push("-e".to_owned());
        args.push(env);
    }
    args.push(image.to_owned());
    args
}

/// The wall, then the image, the container and the secrets, then the start. Reads the token and the
/// viewer password from stdin, a line each. Prints `started`; exits with its own number otherwise
/// ([`start_failure`]). The staged files go once the box answers ([`clear`]), and in five minutes
/// whatever happens, so a broker that went away mid-start leaves nothing behind.
pub fn start(image: &str, id: &str) -> String {
    let closed: Vec<String> = CLOSED.iter().map(|d| format!("iptables -A BUWIZ -d {d} -j DROP || exit 31")).collect();
    let create = create_args(image, id).iter().map(|w| quoted(w)).collect::<Vec<_>>().join(" ");
    let image_q = quoted(image);
    format!(
        r#"{START_MARK}
set -u
d={SECRETS}
docker network inspect {NET} >/dev/null 2>&1 || docker network create --driver bridge --opt com.docker.network.bridge.name={BRIDGE} --opt com.docker.network.bridge.enable_icc=false --label app.buwiz.computer=1 {NET} >/dev/null || exit 30
iptables -N BUWIZ 2>/dev/null; iptables -F BUWIZ || exit 31
iptables -A BUWIZ -d {AMAZON_DNS} -p udp --dport 53 -j RETURN || exit 31
iptables -A BUWIZ -d {AMAZON_DNS} -p tcp --dport 53 -j RETURN || exit 31
{closed}
iptables -C DOCKER-USER -i {BRIDGE} -j BUWIZ 2>/dev/null || iptables -I DOCKER-USER -i {BRIDGE} -j BUWIZ || exit 31
iptables -C INPUT -i {BRIDGE} -m conntrack --ctstate NEW -j DROP 2>/dev/null || iptables -I INPUT -i {BRIDGE} -m conntrack --ctstate NEW -j DROP || exit 31
docker image inspect {image_q} >/dev/null 2>&1 || docker pull -q {image_q} >/dev/null 2>&1 || exit 40
[ "$(docker image inspect -f '{{{{index .Config.Labels "{USERS_LABEL}"}}}}' {image_q})" = {USERS_VERSION} ] || exit 41
umask 077
rm -rf -- "$d" && mkdir -m 700 -- "$d" || exit 50
cur=$(docker inspect -f '{{{{.Config.Image}}}}' {NAME} 2>/dev/null || true)
if [ -n "$cur" ] && [ "$cur" != {image_q} ]; then docker rm -f {NAME} >/dev/null || exit 42; cur=; fi
if [ -z "$cur" ]; then {create} >/dev/null || exit 43; fi
IFS= read -r t || exit 51
IFS= read -r v || exit 51
(set -C; printf '%s' "$t" > "$d/{TOKEN_FILE}") && (set -C; printf '%s' "$v" > "$d/{VIEWER_FILE}") || exit 52
t=; v=
setsid -f /bin/sh -c 'sleep 300; rm -f -- "$0/{TOKEN_FILE}" "$0/{VIEWER_FILE}"' "$d" </dev/null >/dev/null 2>&1
docker start {NAME} >/dev/null || {{ rm -f -- "$d/{TOKEN_FILE}" "$d/{VIEWER_FILE}"; exit 60; }}
echo started"#,
        closed = closed.join("\n")
    )
}

/// What [`start`]'s exit means, for a person.
pub fn start_failure(code: Option<i32>, image: &str) -> String {
    match code {
        Some(30 | 31) => "The wall around the box couldn't go up on the instance, so the box didn't start.".to_owned(),
        Some(40) => format!("The instance couldn't pull the computer's image ({image})."),
        Some(41) => format!("The computer's image ({image}) is from before Bots had users of their own: use a newer one."),
        Some(42 | 43) => "The box's container couldn't be made on the instance.".to_owned(),
        Some(50..=52) => "The box's secrets couldn't be put in place on the instance, so it didn't start.".to_owned(),
        Some(60) => "The box didn't start on the instance.".to_owned(),
        _ => "The instance didn't run the box's start.".to_owned(),
    }
}

/// The staged secrets, gone. Safe to repeat.
pub fn clear() -> String {
    format!("{CLEAR_MARK}\nrm -f -- {SECRETS}/{TOKEN_FILE} {SECRETS}/{VIEWER_FILE}")
}

/// The box stopped, with time to save its work, before a stop of the instance that isn't a
/// hibernation.
pub fn stop() -> String {
    format!("{STOP_MARK}\ndocker stop -t 5 {NAME} >/dev/null 2>&1 || true")
}

/// `python` run as root inside the box, from outside it, with `args`: `docker.rs`'s checks, the same
/// way it runs them on a Mac.
pub fn inside() -> String {
    format!("{INSIDE_MARK}\nexec docker exec --user 0 --privileged {NAME} /usr/bin/python3 -I -S -c \"$@\"")
}

/// The instance's user data: the launch template's own, and a part that lets this computer's
/// key log in as the image's default user. No secret: a public key.
pub fn user_data(template: Option<&str>, public_key: &str, boundary: &str) -> String {
    let mut out = format!("Content-Type: multipart/mixed; boundary=\"{boundary}\"\nMIME-Version: 1.0\n\n");
    if let Some(own) = template.filter(|t| !t.trim().is_empty()) {
        // text/plain: cloud-init reads the part's kind from its first line
        out.push_str(&format!("--{boundary}\nContent-Type: text/plain; charset=\"utf-8\"\nMIME-Version: 1.0\n\n{}\n", own.trim_end()));
    }
    out.push_str(&format!("--{boundary}\nContent-Type: text/cloud-config; charset=\"us-ascii\"\nMIME-Version: 1.0\n\n#cloud-config\nssh_authorized_keys:\n  - {}\n", public_key.trim()));
    out.push_str(&format!("--{boundary}--\n"));
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn the_box_is_hardened_as_on_a_mac_and_publishes_on_loopback_alone() {
        let args = create_args("ghcr.io/buwiz-bot/computer:latest", "cmp_1").join(" ");
        assert!(args.contains("--cap-drop ALL") && args.contains("no-new-privileges") && args.contains("--pids-limit 2048"), "{args}");
        for cap in BOX_CAPS {
            assert!(args.contains(&format!("--cap-add {cap}")), "{args}");
        }
        for cap in ["NET_RAW", "NET_ADMIN", "SYS_ADMIN", "SYS_PTRACE", "MKNOD", "SETPCAP"] {
            assert!(!args.contains(cap), "{cap}: {args}");
        }
        assert!(args.contains("-p 127.0.0.1:1337:1337") && args.contains("-p 127.0.0.1:1340:1340") && args.contains("-p 127.0.0.1:6901-6905:6901-6905"), "{args}");
        assert!(!args.contains("0.0.0.0") && !args.contains("8790"), "{args}");
        assert!(args.contains("ip_unprivileged_port_start=6906") && args.contains("disable_ipv6=1"), "{args}");
        assert!(args.contains("target=/run/buwiz-computer,readonly") && args.contains("BOX_TOKEN_FILE=/run/buwiz-computer/token"), "{args}");
        assert!(!args.contains("BOX_TOKEN=") && !args.contains("BOX_VIEWER_PASSWORD="), "{args}");
        assert!(args.contains("--network buwiz-computer-net") && args.contains("--dns 169.254.169.253"), "{args}");
    }

    #[test]
    fn the_wall_goes_up_before_the_box_starts_and_secrets_come_on_stdin() {
        let s = start("img:1", "cmp_1");
        let at = |w: &str| s.find(w).unwrap_or_else(|| panic!("{w}"));
        assert!(at("iptables -A BUWIZ -d 169.254.0.0/16 -j DROP") < at("docker start"));
        assert!(at("DOCKER-USER -i buwiz0 -j BUWIZ") < at("docker start"));
        assert!(at("RETURN") < at("-d 169.254.0.0/16"), "DNS is let through before the link-local drop");
        assert!(at("read -r t") < at("docker start") && at("setsid -f") < at("docker start"));
        assert!(!s.contains("BOX_TOKEN="), "no value on a command line");
    }

    #[test]
    fn user_data_keeps_the_templates_and_adds_only_a_public_key() {
        let data = user_data(Some("#cloud-config\npackages: [docker.io]\n"), "ssh-ed25519 AAAA buwiz-computer-cmp_1", "b0und");
        assert!(data.starts_with("Content-Type: multipart/mixed; boundary=\"b0und\""));
        assert!(data.contains("packages: [docker.io]") && data.contains("ssh_authorized_keys:\n  - ssh-ed25519 AAAA buwiz-computer-cmp_1"));
        assert!(data.trim_end().ends_with("--b0und--"));
        assert!(!user_data(None, "ssh-ed25519 AAAA", "b").contains("text/plain"));
    }
}
