//! The local Docker provider: two containers per computer, driven through the `docker` CLI.
//!
//! - **The wall comes first.** On Docker Desktop a container reaches the Mac's loopback through
//!   the host gateway, broker included. A box's home persists, so a model can leave something in
//!   its autostart to run at the next start. So the box never owns its network: a small holder
//!   container does (`<name>-wall`, a `sleep`), and the box joins it. The holder starts, the wall
//!   goes up in its network as root from outside ([`WALL`]), and only then does the box start.
//!   No process of the box ever runs without the wall, and the box keeps no capability to take
//!   it down. If it can't go up, the box doesn't start.
//! - **Files outlive containers.** A box or holder that went missing is made again on the same
//!   named volumes. Those are `/workspace` and `/home/box`, all that a rebuild keeps: the image
//!   declares no `VOLUME`, and whatever was written anywhere else goes with the box. Volumes are
//!   only ever removed by [`Provider::destroy`] and [`Provider::sweep`], which are for a computer
//!   a person agreed to delete.
//! - **Ports** are published by the holder, on loopback only, from a range the Bot's shell is
//!   kept off ([`crate::PORT_FIRST`]). They are read for each request.
//! - **Secrets** never go on an argument list, where any process's `ps` reads them: the container
//!   gets file paths, in a folder it mounts read-only. They are put there before every start
//!   (the box exits without them, so a plain `docker restart` fails safe) and taken away once it
//!   answers, or the start is abandoned ([`Staged`]). The image would unlink them as it reads them
//!   and says it couldn't: a mount the box can write is a place to plant things in a Mac folder,
//!   so this side does the removing.
//! - **Users.** A box starts as root, with only the capabilities it takes to set itself up and
//!   start each part as its own user ([`BOX_CAPS`]). `box-keeper` alone stays root. Commands run as
//!   a user with no way to a screen, a browser profile or a service. A box from an older image
//!   hasn't got that, and is made again on the same volumes at its next wake, with its home set
//!   aside: commands could write there. What says which is a box Docker's label for the image
//!   ([`USERS_LABEL`]), and whether Bots on one computer are really apart is read from outside
//!   ([`Provider::users_apart`]).
//! - **Sleep** is `docker pause`: it comes back at once with everything open. Deep sleep is
//!   `docker stop`: the memory is free, and the files stay on the volumes. The holder stays up
//!   through both, a `sleep` and little else.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::Value;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::guest::BoxGuest;
use crate::{
    random_hex, CommandOutput, Egress, Input, Leftover, Machine, Network, Power, Problem, Provider, ProviderKind, Reach, Result, Screen, Screenshot, Secret, Seen, Settled, Sleep, Spec, Viewer, WaitFor, Woke, DOCKER_PORT_LAST, EGRESS_PORT, PORT_FIRST,
};

/// The image's tag on this Mac (the computer repo's compose file builds it).
pub const DEFAULT_IMAGE: &str = "buwiz-computer:local";
pub const DEFAULT_PREFIX: &str = "buwiz-computer";
pub const IMAGE_ENV: &str = "BUWIZ_COMPUTER_IMAGE";
pub const PREFIX_ENV: &str = "BUWIZ_COMPUTER_PREFIX";

pub(crate) const EXEC_PORT: u16 = 1337;
pub(crate) const HOST_PORT: u16 = 1340;
/// The screens' viewers, `6900 + slot`: the box's own, then the Bots'.
pub(crate) const VIEWER_PORTS: [u16; 5] = [6901, 6902, 6903, 6904, 6905];
const LABEL: &str = "app.buwiz.computer";
/// The label a computer's containers and volumes carry, with its id: `{LABEL}.id`.
pub const ID_LABEL: &str = "app.buwiz.computer.id";
/// Where the container sees the folder its secrets are put in for a start.
const SECRET_MOUNT: &str = "/run/buwiz-computer";
const TOKEN_FILE: &str = "token";
const VIEWER_FILE: &str = "viewer";
/// The tunnel's bearer, for a computer made with the tunnel: staged and taken away with the others.
const EGRESS_FILE: &str = "egress";
/// The browsing rules the box starts with. Not a secret: the hub keeps it there, the latest set,
/// so every start reads the rules before anything of the box runs ([`stage_rules`]).
const RULES_FILE: &str = "rules.json";
/// How many random bytes the tunnel's bearer is.
const BEARER_BYTES: usize = 32;
/// How long `docker` itself gets; a hung daemon counts as down.
const DOCKER_PATIENCE: Duration = Duration::from_secs(60);
/// How long a start gets to answer.
const WAKE_PATIENCE: Duration = Duration::from_secs(120);
/// On the image, and so on each container made from it: this image runs commands as users of their
/// own. A new value is for an image whose containers need making again.
pub(crate) const USERS_LABEL: &str = "app.buwiz.box.users";
pub(crate) const USERS_VERSION: &str = "1";
/// On an image whose browser can go through its own proxy, under the computer's rules. A computer is
/// made with the proxy only on such an image: on an older one it's made as before, and says so.
pub(crate) const EGRESS_LABEL: &str = "app.buwiz.box.egress";
/// What the box's root needs, and nothing else. A box doesn't start with fewer.
pub(crate) const BOX_CAPS: [&str; 8] = ["CHOWN", "DAC_OVERRIDE", "FOWNER", "FSETID", "KILL", "NET_BIND_SERVICE", "SETGID", "SETUID"];
/// Below this a port takes a capability, in the network the holder owns, so no command can listen
/// on a service's port, free or not, or send from one. It's the first above the viewers.
const FIRST_OPEN_PORT: u16 = 6906;
const STOP_SECONDS: &str = "5";
const AT_ONCE: &str = "0";
const IFCONFIG: &str = "/sbin/ifconfig";
const PORT_TRIES: usize = 4;
/// How long `guard` trusts the addresses it read: it runs on every request, and a Mac changes
/// networks rarely. A wake reads them anyway.
const ADDRESS_LIFE: Duration = Duration::from_secs(60);
/// `ifconfig` failures in a row a running box rides out on the addresses it has: the wall it has
/// stays up meanwhile, so one bad read is no reason to kill its work.
const ADDRESS_TRIES: usize = 3;

/// Where Docker Desktop puts its CLI: an app started from Finder has a bare `PATH`.
const DOCKER_DIRS: [&str; 4] = ["/usr/local/bin", "/opt/homebrew/bin", "/Applications/Docker.app/Contents/Resources/bin", "~/.docker/bin"];

/// The wall, over netlink, with nothing but python3 in the image. Its arguments are this Mac's
/// own addresses. Safe to repeat.
///
/// - Blackhole routes for every range that isn't the open internet: the private ranges (which
///   hold the host gateway, `192.168.65.x`, the way to the Mac's loopback), link-local, CGNAT, the
///   benchmark range some VPNs use, the reserved block.
/// - Blackhole routes for each of this Mac's addresses, wherever they are.
/// - Any route the box has with a next hop of its own, beyond the default.
/// - The box's own subnets, bridge gateway included, by rule: a route would stop the replies on
///   published ports too, which come from that gateway. So the rules first let through what goes
///   to the gateway alone from those ports, then drop the rest, peers on the network included. A
///   default route keeps working: its next hop is resolved on the link and never looked up as a
///   destination.
/// - A computer made with the tunnel (`--tunnel` first among the arguments) lets through, the same
///   way, what its WebSocket (8790) sends to the gateway alone. 8790 is above the box's port floor:
///   the box's proxy holds it from before any command can run, and the box stops if that ends.
/// - IPv6 is off (`create_args`); a box where it is on doesn't pass.
const WALL: &str = r#"
import socket, struct, sys
s = socket.socket(socket.AF_NETLINK, socket.SOCK_RAW, 0)
def ask(kind, flags, body):
    s.send(struct.pack('=IHHII', 16 + len(body), kind, 0x1 | 0x4 | flags, 1, 0) + body)
    return struct.unpack('=i', s.recv(4096)[16:20])[0]
def attr(kind, data):
    return struct.pack('=HH', 4 + len(data), kind) + data + b'\0' * (-len(data) % 4)
def blackhole(dst, plen):
    err = ask(24, 0x400 | 0x100, struct.pack('=BBBBBBBBI', 2, plen, 0, 0, 254, 4, 0, 6, 0) + attr(1, socket.inet_aton(dst)))
    if err:
        raise OSError(-err, dst)
def rule(priority, action, dst=None, plen=0, ports=None):
    body = struct.pack('=BBBBBBBBI', 2, plen, 0, 0, 254 if action == 1 else 0, 0, 0, action, 0) + attr(6, struct.pack('=I', priority))
    if dst:
        body += attr(1, socket.inet_aton(dst))
    if ports:
        body += attr(22, b'\x06') + attr(23, struct.pack('=HH', *ports))
    err = ask(32, 0x400 | 0x200, body)
    if err and err != -17:
        raise OSError(-err, 'rule')
for dst, plen in [('0.0.0.0', 8), ('10.0.0.0', 8), ('100.64.0.0', 10), ('169.254.0.0', 16), ('172.16.0.0', 12), ('192.0.0.0', 24), ('192.168.0.0', 16), ('198.18.0.0', 15), ('240.0.0.0', 4)]:
    blackhole(dst, plen)
tunnel = sys.argv[1:2] == ['--tunnel']
for address in sys.argv[2 if tunnel else 1:]:
    blackhole(address, 32)
ip = lambda h: socket.inet_ntoa(struct.pack('<I', int(h, 16)))
routes = [line.split()[:8] for line in open('/proc/net/route').read().splitlines()[1:]]
bridge = [ip(r[2]) for r in routes if r[0] != '*' and r[1] == '00000000' and r[7] == '00000000']
if len(bridge) != 1:
    raise OSError('no bridge gateway')
rule(100, 1, dst=bridge[0], plen=32, ports=(1337, 1337))
rule(101, 1, dst=bridge[0], plen=32, ports=(1340, 1340))
rule(102, 1, dst=bridge[0], plen=32, ports=(6901, 6905))
if tunnel:
    rule(103, 1, dst=bridge[0], plen=32, ports=(8790, 8790))
priority = 200
for iface, dst, gateway, flags, _, _, _, mask in routes:
    plen = bin(int(mask, 16)).count('1')
    if iface == '*' or dst == '00000000':
        continue
    if gateway != '00000000':
        blackhole(ip(dst), plen)
    else:
        rule(priority, 6, dst=ip(dst), plen=plen)
        priority += 1
try:
    if open('/proc/sys/net/ipv6/conf/all/disable_ipv6').read().strip() != '1':
        raise OSError('IPv6 is on')
except FileNotFoundError:
    pass
"#;

/// Ends what's left on one Bot's desktop before a person takes it over, as root from outside, with
/// nothing but python3 in the image. Prints what it ended, as a JSON list. Safe to repeat.
///
/// A command can't reach a desktop, so this isn't for commands. It's for what the model got running
/// on the screen itself, by clicks: that's the desktop's user. So this looks at that user alone, and
/// spares the desktop and browser, by the path of the file each runs, which that user can't write
/// to. Starting an allowed program with a library of its own doesn't help: anything mapped to run
/// from outside the system's folders makes it a stray. `voxl` runs as this user too, and counts only
/// as the root keeper's child: one that user started is watching the screen. Passes repeat until one
/// finds nothing, so a forking loop doesn't get away.
///
/// The browser's launcher chain stays, by the process tree, so an open browser lives through a
/// hand-over: Chromium dies with `box-chrome-pipe` (`PR_SET_PDEATHSIG`), and `box-chromium`'s shell
/// loop starts it again when `box-exec` lets go of its pipe. A `box-chrome-pipe` stays only while
/// it's the parent of the system's Chromium itself (`/usr/lib/chromium/chromium`, not just anything
/// the desktop may run), and only as `box-chromium` starts it: its arguments name `chromium`, to be
/// found on a `PATH` of the system's folders alone. The shell stays only while it's the parent of
/// such a launcher or of that Chromium, and runs the system's `dash` on `box-chromium` (its first
/// argument, and a file it holds open), on that same `PATH`. Both run root's files, and
/// neither can start anything again but the system's Chromium. A launcher whose Chromium went, and a
/// shell that runs anything else, go like any stray.
///
/// The name is bytes, and a user's process is one if any of its ids says so. A process goes on while
/// any thread lives, and a leader that exited shows `Z` with its threads running, so a process only
/// counts as gone when every thread has. `/proc/<pid>/exe` needs `CAP_SYS_PTRACE` for another user,
/// so this needs `--privileged`: `docker exec` can't add just that. A file that can't be read makes
/// a stray, and so does a process of that user that can't be read at all. One that exits while it's
/// read, or an fd it closes, is nothing to end and says nothing of anything else. The Bot's number
/// comes from a file only root writes, and without it nothing is handed over.
///
/// It's no wall: a page in the browser is a page, and the list of what the desktop runs is kept by
/// hand. It ends what was left running on that screen, which is what's there to be left.
#[doc(hidden)]
pub const CLEAR: &str = r#"
import fnmatch, json, os, signal, stat, sys, time
ALLOWED = ['/usr/local/bin/voxl', '/usr/bin/Xvfb', '/usr/bin/dbus-launch', '/usr/bin/dbus-daemon', '/usr/bin/xfce4-session', '/usr/bin/xfwm4', '/usr/bin/xfsettingsd', '/usr/bin/xfce4-panel', '/usr/bin/Thunar', '/usr/bin/thunar', '/usr/bin/xfdesktop', '/usr/lib/*/xfce4/*', '/usr/lib/*/tumbler-1/*', '/usr/lib/chromium/*', '/usr/local/bin/box-active-tab']
LIBRARIES = ('/usr/', '/lib/', '/opt/box/')
VOXL, KEEPER = '/usr/local/bin/voxl', '/usr/local/bin/box-keeper'
# the box image's own (buwiz-bot/computer, docker/Dockerfile, on Debian): nothing here installs
# them. Its /bin/sh is dash, whose exe reads /usr/bin/dash where /bin is merged into /usr, as in
# bookworm, and /bin/dash where it isn't
LAUNCHER, SCRIPT = '/usr/local/bin/box-chrome-pipe', '/usr/local/bin/box-chromium'
# the browser itself, of the '/usr/lib/chromium/*' ALLOWED keeps: /usr/bin/chromium execs it
CHROMIUM = '/usr/lib/chromium/chromium'
SHELLS = ('/usr/bin/dash', '/bin/dash')
SYSTEM_PATH = ('/usr/local/sbin', '/usr/local/bin', '/usr/sbin', '/usr/bin', '/sbin', '/bin')
path = '/home/box/.box/bots/' + sys.argv[1]
info = os.lstat(path)
if info.st_uid != 0 or not stat.S_ISREG(info.st_mode):
    raise SystemExit('this Bot has no user')
USER = 20001 + 2 * int(open(path).read().strip())
def alive(pid):
    for task in os.listdir('/proc/%s/task' % pid):
        stat_line = open('/proc/%s/task/%s/stat' % (pid, task), 'rb').read()
        if stat_line[stat_line.rindex(b')') + 2:].split()[0] != b'Z':
            return True
    return False
def foreign(pid):
    for line in open('/proc/%s/maps' % pid, 'rb'):
        fields = line.split(None, 5)
        if len(fields) == 6 and b'x' in fields[1]:
            name = fields[5].strip().decode('utf-8', 'replace')
            if name.startswith('/') and not name.startswith(LIBRARIES):
                return True
    return False
def keepers_own(pid, status):
    parent = status[status.index(b'\nPPid:') + 6:].split()[0].decode()
    return os.readlink('/proc/%s/exe' % parent) == KEEPER and os.stat('/proc/%s' % parent).st_uid == 0
def gone(pid):
    # exited since it was listed: nothing to end, and no word on anything else
    try:
        return not alive(pid)
    except FileNotFoundError:
        return True
def words(pid, what):
    return open('/proc/%s/%s' % (pid, what), 'rb').read().split(b'\0')[:-1]
def system_path(pid):
    paths = [w[5:] for w in words(pid, 'environ') if w.startswith(b'PATH=')]
    return len(paths) == 1 and all(p.decode('utf-8', 'replace') in SYSTEM_PATH for p in paths[0].split(b':'))
def chain(pid, exe):
    # which part of the browser's launcher chain this is, by what the kernel shows: '' for none
    if exe == LAUNCHER:
        argv = words(pid, 'cmdline')
        return 'launcher' if len(argv) > 3 and argv[2] == b'--' and argv[3] == b'chromium' and system_path(pid) else ''
    if exe in SHELLS:
        argv = words(pid, 'cmdline')
        held = False
        for f in os.listdir('/proc/%s/fd' % pid):
            try:
                # every one is read: one there that won't say makes it a stray, whatever came first
                held = os.readlink('/proc/%s/fd/%s' % (pid, f)) == SCRIPT or held
            except FileNotFoundError:
                # closed since the folder was read: it held nothing
                continue
        return 'loop' if len(argv) > 1 and argv[1] == SCRIPT.encode() and held and system_path(pid) else ''
    return ''
def table():
    out = {}
    for d in os.listdir('/proc'):
        if not d.isdigit():
            continue
        try:
            stat_line = open('/proc/%s/stat' % d, 'rb').read()
            status = open('/proc/%s/status' % d, 'rb').read()
            ids = [int(n) for n in status[status.index(b'\nUid:') + 5:].split(b'\n')[0].split()]
            if USER not in ids or not alive(d):
                continue
            name = stat_line[stat_line.index(b'(') + 1:stat_line.rindex(b')')].decode('utf-8', 'replace')
            parent = int(status[status.index(b'\nPPid:') + 6:].split()[0])
            try:
                exe = os.readlink('/proc/%s/exe' % d)
                strange = foreign(d) or (exe == VOXL and not keepers_own(d, status))
                part = '' if strange else chain(d, exe)
            except FileNotFoundError:
                # a process that went while it was read is no stray; one still there that won't say is
                if gone(d):
                    continue
                exe, strange, part = '', True, ''
            except OSError:
                exe, strange, part = '', True, ''
            out[int(d)] = (exe, name, strange, parent, part)
        except (FileNotFoundError, ProcessLookupError):
            continue
        except (OSError, ValueError, IndexError):
            # alive, and not saying what it is: the folder's owner is the one thing left to go on
            try:
                if os.stat('/proc/%s' % d).st_uid == USER:
                    out[int(d)] = ('', 'unreadable', True, 0, '')
            except OSError:
                pass
    return out
ended = []
for _ in range(20):
    procs = table()
    kept = {p for p, (exe, _, strange, _, _) in procs.items() if not strange and any(fnmatch.fnmatch(exe, a) for a in ALLOWED)}
    # the browser's launcher by the Chromium it started, and its loop by that launcher or Chromium:
    # a child that's only allowed, xfce4-panel say, keeps neither
    browser = {p for p in kept if procs[p][0] == CHROMIUM}
    launchers = {p for p, v in procs.items() if v[4] == 'launcher' and any(c[3] == p and q in browser for q, c in procs.items())}
    loops = {p for p, v in procs.items() if v[4] == 'loop' and any(c[3] == p and q in browser | launchers for q, c in procs.items())}
    kept |= launchers | loops
    strays = [p for p in procs if p not in kept]
    if not strays:
        print(json.dumps(sorted(set(ended))))
        raise SystemExit(0)
    for p in strays:
        try:
            os.kill(p, signal.SIGKILL)
            ended.append(procs[p][1])
        except ProcessLookupError:
            pass
    time.sleep(0.05)
raise SystemExit('programs kept coming')
"#;

/// Whether the Bots named are kept apart on this computer, read as root from outside: it prints
/// `apart`, or why not. Everything it reads is the kernel's, or a file only root writes, and it runs
/// the image's own python, which no command can write to, with nothing of the box's environment.
/// A process in the box can't answer for it.
///
/// - The port floor is up, in this network.
/// - Each Bot has its own two numbers, a folder only its users can open, and a desktop home only its
///   desktop user can.
/// - The services are `boxsvc`'s, and each voxl a desktop's.
/// - Every X listener in the box, by the kernel's list and not by program: each is held by the real
///   Xvfb as a desktop's user, with no `-ac` or `-shmem`, and turns away a client with no cookie,
///   which is also what shows `xhost +` or an empty cookie file.
/// - Each server's cookie and its folder are root's, for that desktop's group alone. The services'
///   cookie is the same for theirs. A desktop can't pass a cookie on, by mode or by group.
/// - No desktop has shared memory open.
/// - No process of a Bot's user holds a capability, but voxl its port's.
pub(crate) const APART: &str = r#"
import os, re, socket, stat, struct, sys
def no(why):
    print(why)
    raise SystemExit(0)
PORT_FLOOR = 6906
SVC = 990
if int(open('/proc/sys/net/ipv4/ip_unprivileged_port_start').read()) < PORT_FLOOR:
    no('the port floor is down')
seen = set()
desktop = lambda uid: uid == 1000 or (uid >= 20001 and uid % 2 == 1)
for bot in sys.argv[1:]:
    path = '/home/box/.box/bots/' + bot
    info = os.lstat(path)
    if info.st_uid != 0 or not stat.S_ISREG(info.st_mode):
        no('a Bot has no user')
    agent = 20000 + 2 * int(open(path).read().strip())
    if agent in seen:
        no('two Bots share a user')
    seen.add(agent)
    folder = os.lstat('/workspace/agents/' + bot)
    if not stat.S_ISDIR(folder.st_mode) or folder.st_uid != agent or folder.st_mode & 0o007:
        no('a Bot folder is open')
    home = os.lstat('/home/box/desktops/' + bot)
    if not stat.S_ISDIR(home.st_mode) or home.st_uid != agent + 1 or home.st_mode & 0o077:
        no('a desktop home is open')
def closed_folder(path):
    info = os.lstat(path)
    if not stat.S_ISDIR(info.st_mode) or info.st_uid != 0 or info.st_mode & 0o022:
        no('an X folder is open')
def cookie(path, group):
    info = os.lstat(path)
    if not stat.S_ISREG(info.st_mode) or info.st_size == 0 or info.st_uid != 0 or info.st_gid != group or info.st_mode & 0o777 != 0o640:
        no('an X cookie is open')
closed_folder('/run/box/x')
cookie('/run/box/x/svc.auth', SVC)
listeners = {}
for line in open('/proc/net/unix').read().splitlines()[1:]:
    fields = line.split()
    if len(fields) >= 8 and int(fields[3], 16) & 0x10000 and fields[7].lstrip('@').startswith('/tmp/.X11-unix/'):
        listeners[fields[6]] = fields[7]
held = {}
for d in os.listdir('/proc'):
    if not d.isdigit():
        continue
    try:
        status = open('/proc/%s/status' % d, 'rb').read().decode('utf-8', 'replace')
        exe = os.readlink('/proc/%s/exe' % d)
        args = open('/proc/%s/cmdline' % d, 'rb').read().split(b'\0')
        sockets = [os.readlink('/proc/%s/fd/%s' % (d, f)) for f in os.listdir('/proc/%s/fd' % d)]
    except FileNotFoundError:
        continue
    except PermissionError:
        no('a process cannot be read')
    fields = dict(line.split(':', 1) for line in status.splitlines() if ':' in line)
    ids = [int(n) for n in fields['Uid'].split()]
    for inode in re.findall(r'socket:\[(\d+)\]', ' '.join(sockets)):
        if inode in listeners:
            held[inode] = True
            if exe != '/usr/bin/Xvfb':
                no('something else is listening as an X server')
    if exe in ('/usr/local/bin/box-exec', '/usr/local/bin/box-host') and set(ids) != {SVC}:
        no('a service runs as another user')
    if exe == '/usr/local/bin/voxl' and not all(desktop(i) for i in ids):
        no('a viewer runs as another user')
    if exe == '/usr/bin/Xvfb':
        if not all(desktop(i) for i in ids) or len(set(ids)) != 1 or b'-ac' in args or b'-shmem' in args or b'-listen' in args or b'-auth' not in args:
            no('an X server is open')
        number = args[1].decode('utf-8', 'replace').lstrip(':')
        closed_folder('/run/box/x/' + number)
        path = args[args.index(b'-auth') + 1].decode('utf-8', 'replace')
        if path != '/run/box/x/%s/auth' % number:
            no('an X cookie is open')
        cookie(path, ids[0])
        run = os.lstat('/run/box/x/%s/run' % number)
        if run.st_uid != ids[0] or run.st_mode & 0o007:
            no('an X folder is open')
    allowed = 0x400 if exe == '/usr/local/bin/voxl' else 0
    if any(i >= 20000 for i in ids) and any(int(fields[c], 16) & ~allowed for c in ('CapEff', 'CapPrm', 'CapAmb')):
        no('a Bot user holds a capability')
if not listeners:
    no('no X server')
if set(listeners) - set(held):
    no('an X listener has no owner')
for name in listeners.values():
    s = socket.socket(socket.AF_UNIX)
    s.settimeout(2)
    try:
        s.connect('\0' + name[1:] if name.startswith('@') else name)
        s.sendall(struct.pack('<BxHHHHxx', 0x6c, 11, 0, 0, 0))
        answer = s.recv(1)
    except OSError:
        no('an X listener did not answer')
    if answer != b'\0':
        no('an X server lets anybody in')
    s.close()
lines = open('/proc/sysvipc/shm').read().splitlines()[1:]
for line in lines:
    fields = line.split()
    if int(fields[2], 8) & 0o077 and desktop(int(fields[7])):
        no('a desktop has shared memory open')
print('apart')
"#;

/// The local Docker computers.
pub struct DockerProvider {
    image: String,
    prefix: String,
    /// the program, when it isn't found the usual way (tests)
    program: Option<PathBuf>,
    /// where to read this Mac's addresses (tests)
    ifconfig: PathBuf,
    /// by computer: the start of its network and the addresses the wall went up with
    walled: Mutex<HashMap<String, (String, Vec<String>)>>,
    /// this Mac's addresses, and when they were read
    addresses: Mutex<Option<(Instant, Vec<String>)>>,
    address_life: Duration,
    /// failed reads in a row
    address_failures: std::sync::atomic::AtomicUsize,
}

impl DockerProvider {
    pub fn new(image: impl Into<String>, prefix: impl Into<String>) -> Self {
        Self { image: image.into(), prefix: prefix.into(), program: None, ifconfig: PathBuf::from(IFCONFIG), walled: Mutex::new(HashMap::new()), addresses: Mutex::new(None), address_life: ADDRESS_LIFE, address_failures: std::sync::atomic::AtomicUsize::new(0) }
    }

    /// From `BUWIZ_COMPUTER_IMAGE` and `BUWIZ_COMPUTER_PREFIX`.
    pub fn from_env() -> Self {
        let set = |k: &str, default: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty()).unwrap_or_else(|| default.to_owned());
        Self::new(set(IMAGE_ENV, DEFAULT_IMAGE), set(PREFIX_ENV, DEFAULT_PREFIX))
    }

    /// Runs `program` as `docker`.
    pub fn with_program(mut self, program: PathBuf) -> Self {
        self.program = Some(program);
        self
    }

    /// Reads this Mac's addresses with `program`.
    pub fn with_ifconfig(mut self, program: PathBuf) -> Self {
        self.ifconfig = program;
        self
    }

    /// Trusts addresses it read for this long.
    pub fn with_address_life(mut self, life: Duration) -> Self {
        self.address_life = life;
        self
    }

    /// This Mac's addresses: read now when `fresh`, else as last read while that's recent. A
    /// fresh read that fails is a failure: a wake doesn't go on without knowing. On the timer, a
    /// box with a wall carries on with what was known, and tries again at the next timer, until
    /// [`ADDRESS_TRIES`] reads in a row have failed.
    async fn addresses(&self, fresh: bool) -> Result<Vec<String>> {
        use std::sync::atomic::Ordering;
        if !fresh {
            let known = self.addresses.lock().unwrap_or_else(|e| e.into_inner());
            if let Some((_, macs)) = known.as_ref().filter(|(at, _)| at.elapsed() < self.address_life) {
                return Ok(macs.clone());
            }
        }
        match mac_addresses(&self.ifconfig).await {
            Ok(macs) => {
                self.address_failures.store(0, Ordering::SeqCst);
                *self.addresses.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), macs.clone()));
                Ok(macs)
            }
            Err(e) => {
                let failures = self.address_failures.fetch_add(1, Ordering::SeqCst) + 1;
                let mut known = self.addresses.lock().unwrap_or_else(|e| e.into_inner());
                match known.as_mut() {
                    Some((at, macs)) if !fresh && failures < ADDRESS_TRIES => {
                        eprintln!("buwiz-computer: this Mac's addresses can't be read ({failures} in a row); keeping the wall that's up");
                        *at = Instant::now();
                        Ok(macs.clone())
                    }
                    _ => Err(e),
                }
            }
        }
    }

    fn network(&self) -> String {
        format!("{}-net", self.prefix)
    }

    fn program(&self) -> PathBuf {
        self.program.clone().unwrap_or_else(find_docker)
    }

    async fn docker(&self, args: &[&str], stdin: Option<&[u8]>) -> Result<String> {
        let raw = run_docker(&self.program(), args, stdin, DOCKER_PATIENCE).await?;
        match raw.status {
            Some(0) => Ok(String::from_utf8_lossy(&raw.stdout).into_owned()),
            status => Err(classify(status, &String::from_utf8_lossy(&raw.stderr), &self.image)),
        }
    }

    async fn ensure_network(&self) -> Result<()> {
        let net = self.network();
        if self.docker(&["network", "inspect", "--format", "{{.Id}}", &net], None).await.is_ok() {
            return Ok(());
        }
        self.docker(&["network", "create", "--opt", "com.docker.network.bridge.enable_icc=false", "--label", &format!("{LABEL}=1"), &net], None).await.map(|_| ())
    }

    async fn inspect(&self, name: &str) -> Result<Inspected> {
        // one run for both. Docker answers for what it finds and exits 1 for the rest, so that
        // a missing holder can't be read as a missing computer: only the box decides that
        let raw = run_docker(&self.program(), &["inspect", "--type", "container", name, &holder(name)], None, DOCKER_PATIENCE).await?;
        match Inspected::parse(&String::from_utf8_lossy(&raw.stdout), name) {
            Ok(found) => Ok(found),
            Err(_) if raw.status != Some(0) => Err(classify(raw.status, &String::from_utf8_lossy(&raw.stderr), &self.image)),
            Err(e) => Err(e),
        }
    }

    /// What this request's [`Provider::look`] saw, else a read now.
    async fn seen(&self, machine: &Machine) -> Result<Inspected> {
        match &machine.seen {
            Some(seen) => Ok(seen.clone()),
            None => self.inspect(container(machine)?).await,
        }
    }

    /// The running computer's services, at the ports read for this request: nothing is kept
    /// between requests, because a start can pick new ports, and an old one can be another
    /// computer's by then.
    async fn guest(&self, machine: &Machine) -> Result<BoxGuest> {
        let found = self.seen(machine).await?;
        if found.status != "running" {
            return Err(Problem::Asleep);
        }
        connect(&found, machine)
    }

    /// `docker start`, answering. The secrets are there only for the start, and go however it
    /// ends, a dropped future included.
    async fn start(&self, machine: &Machine, name: &str) -> Result<()> {
        let staged = Staged::new(secret_dir(&machine.dir), machine)?;
        let up = self.start_staged(machine, name).await;
        drop(staged);
        up
    }

    async fn start_staged(&self, machine: &Machine, name: &str) -> Result<()> {
        self.docker(&["start", name], None).await?;
        let plain = connect(&self.inspect(name).await?, machine)?;
        let deadline = Instant::now() + WAKE_PATIENCE;
        while !plain.ready().await {
            if Instant::now() > deadline {
                return Err(Problem::Failed(format!("it didn't answer in {}s", WAKE_PATIENCE.as_secs())));
            }
            let status = self.inspect(name).await?.status;
            if status != "running" {
                return Err(Problem::Failed(format!("it stopped while starting ({status})")));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        Ok(())
    }

    /// Its network up and walled, for the Mac's addresses as they are, before anything of the box
    /// runs in it. `fresh` reads them now. Whatever is running there stops if that fails.
    async fn net_up(&self, name: &str, found: &Inspected, fresh: bool, tunnel: bool) -> Result<()> {
        let up = async {
            let macs = self.addresses(fresh).await?;
            let same = self.walled.lock().unwrap_or_else(|e| e.into_inner()).get(name).is_some_and(|(started, walled)| *started == found.started && *walled == macs);
            if found.net_running && same {
                return Ok(None);
            }
            if !found.net_running {
                self.start_holder(name).await?;
            }
            self.wall(name, &macs, tunnel).await.map(|started| Some((started, macs)))
        };
        match up.await {
            Ok(None) => Ok(()),
            Ok(Some(walled)) => {
                self.walled.lock().unwrap_or_else(|e| e.into_inner()).insert(name.to_owned(), walled);
                Ok(())
            }
            Err(e) => {
                self.walled.lock().unwrap_or_else(|e| e.into_inner()).remove(name);
                self.halt(name, found, AT_ONCE).await;
                Err(e)
            }
        }
    }

    /// Docker picks each port from the range as it starts, and can lose a race for one just freed.
    async fn start_holder(&self, name: &str) -> Result<()> {
        let net = holder(name);
        for _ in 1..PORT_TRIES {
            match self.docker(&["start", &net], None).await {
                Err(Problem::Failed(why)) if why.contains("ports are not available") => tokio::time::sleep(Duration::from_millis(300)).await,
                done => return done.map(|_| ()),
            }
        }
        self.docker(&["start", &net], None).await.map(|_| ())
    }

    /// The box down, whatever state it's in, in `seconds`: at once when it's no longer safe, else
    /// with time to save its work.
    async fn halt(&self, name: &str, found: &Inspected, seconds: &str) {
        if found.status == "paused" {
            let _ = self.docker(&["unpause", name], None).await;
        }
        if matches!(found.status.as_str(), "paused" | "running") {
            let _ = self.docker(&["stop", "-t", seconds, name], None).await;
        }
    }

    /// Both containers made again, on the same named volumes: the box's files stay. A box can't
    /// rejoin a new holder, because Docker keeps the old one's id, so the box goes too, and it
    /// goes without `-v`.
    ///
    /// Always on this provider's image, the record's may be from before users. With no image to
    /// make them from, nothing is taken down.
    async fn rebuild(&self, machine: &Machine, name: &str, found: Option<&Inspected>) -> Result<()> {
        self.check().await?;
        if let Some(found) = found {
            self.halt(name, found, STOP_SECONDS).await;
        }
        for gone in [name.to_owned(), holder(name)] {
            match self.docker(&["rm", "-f", &gone], None).await {
                Ok(_) | Err(Problem::Gone) => {}
                Err(e) => return Err(e),
            }
        }
        self.walled.lock().unwrap_or_else(|e| e.into_inner()).remove(name);
        self.ensure_network().await?;
        let start = secret_dir(&machine.dir);
        private_dir(&start)?;
        let id = machine.dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        // as it was made: a rebuild never adds or takes away the browser's proxy or the tunnel
        self.make_containers(&self.image, name, &id, &start, machine.egress.as_ref()).await
    }

    /// Whether either of `name`'s volumes is there: each holds files.
    async fn any_volume(&self, name: &str) -> Result<bool> {
        for volume in [format!("{name}-workspace"), format!("{name}-home")] {
            match self.docker(&["volume", "inspect", "--format", "{{.Name}}", &volume], None).await {
                Ok(_) => return Ok(true),
                Err(Problem::Gone) => {}
                Err(e) => return Err(e),
            }
        }
        Ok(false)
    }

    /// `name`'s two volumes, labelled with the computer they're for, so they can be told apart once
    /// nothing else of it is left. Docker hands back one that's already there as it is, label or
    /// none: one made before labels goes on working, and [`DockerProvider::remove`] finds it by name.
    async fn make_volumes(&self, name: &str, id: &str) -> Result<()> {
        let (ours, its) = (format!("{LABEL}=1"), format!("{LABEL}.id={id}"));
        for volume in [format!("{name}-workspace"), format!("{name}-home")] {
            match self.docker(&["volume", "create", "--label", &ours, "--label", &its, &volume], None).await {
                Ok(_) => {}
                Err(Problem::Failed(why)) if why.contains("already exists") => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    async fn make_containers(&self, image: &str, name: &str, id: &str, start: &Path, egress: Option<&Egress>) -> Result<()> {
        self.make_volumes(name, id).await?;
        let tunnel = egress.is_some_and(|e| e.tunnel);
        let net = holder_args(image, name, &self.network(), id, tunnel);
        self.docker(&net.iter().map(String::as_str).collect::<Vec<_>>(), None).await?;
        let args = create_args(image, name, id, start, egress);
        self.docker(&args.iter().map(String::as_str).collect::<Vec<_>>(), None).await.map(|_| ())
    }

    /// Puts the wall up in `name`'s network: the start of that network it's up on.
    async fn wall(&self, name: &str, macs: &[String], tunnel: bool) -> Result<String> {
        let net = holder(name);
        let mut args = vec!["exec", "--user", "0", "--privileged", &net, "python3", "-c", WALL];
        if tunnel {
            args.push("--tunnel");
        }
        args.extend(macs.iter().map(String::as_str));
        match self.docker(&args, None).await {
            Ok(_) => Ok(self.inspect(name).await?.started),
            Err(Problem::Failed(why)) => Err(Problem::Unwalled(why)),
            Err(e) => Err(e),
        }
    }
}

/// Whether Docker's timestamp `a` is before `b`. They trim trailing zeros, so the fraction can't be
/// compared as it stands.
fn older(a: &str, b: &str) -> bool {
    let parts = |t: &str| {
        let t = t.trim_end_matches('Z');
        let (secs, frac) = t.split_once('.').unwrap_or((t, ""));
        (secs.to_owned(), format!("{frac:0<9}"))
    };
    parts(a) < parts(b)
}

/// The container that owns `name`'s network.
fn holder(name: &str) -> String {
    format!("{name}-wall")
}

/// This Mac's own IPv4 addresses, loopback aside, as `ifconfig` lists them: a box must not reach
/// what listens on them, whatever kind of address they are. Not being able to tell is a failure:
/// the wall would go up with holes. A Mac with no network has none, and that's a fine answer.
async fn mac_addresses(ifconfig: &Path) -> Result<Vec<String>> {
    let failed = || Problem::Unwalled("this Mac's own addresses can't be read".to_owned());
    let out = Command::new(ifconfig).stdin(Stdio::null()).stderr(Stdio::null()).output().await.map_err(|_| failed())?;
    if !out.status.success() {
        return Err(failed());
    }
    Ok(parse_inets(&String::from_utf8_lossy(&out.stdout)))
}

fn parse_inets(ifconfig: &str) -> Vec<String> {
    let mut found: Vec<String> = ifconfig
        .lines()
        .filter_map(|l| l.trim_start().strip_prefix("inet ")?.split_whitespace().next())
        .filter(|a| !a.starts_with("127.") && a.parse::<std::net::Ipv4Addr>().is_ok())
        .map(str::to_owned)
        .collect();
    found.sort();
    found.dedup();
    found
}

fn connect(found: &Inspected, machine: &Machine) -> Result<BoxGuest> {
    let port = |p: u16| found.published(p).ok_or_else(|| Problem::Failed(format!("port {p} isn't published on loopback")));
    Ok(BoxGuest::connect(&format!("http://{}", port(EXEC_PORT)?), &format!("http://{}", port(HOST_PORT)?), machine.token.clone()))
}

/// The `docker create` arguments of the container that owns a computer's network: `sleep`, the
/// published ports, and IPv6 off. With the tunnel, its WebSocket's port too, on loopback.
pub(crate) fn holder_args(image: &str, name: &str, network: &str, id: &str, tunnel: bool) -> Vec<String> {
    let net = holder(name);
    let mut args: Vec<String> = ["create", "--name", &net, "--network", network, "--label"].map(str::to_owned).to_vec();
    args.push(format!("{LABEL}=1"));
    args.push("--label".to_owned());
    args.push(format!("{LABEL}.id={id}"));
    // a `sleep` needs nothing, and the wall's exec brings its own privileges
    for a in ["--user", "65534:65534", "--cap-drop", "ALL", "--memory", "64m", "--pids-limit", "64", "--security-opt", "no-new-privileges", "--sysctl", "net.ipv6.conf.all.disable_ipv6=1"] {
        args.push(a.to_owned());
    }
    args.push("--sysctl".to_owned());
    args.push(format!("net.ipv4.ip_unprivileged_port_start={FIRST_OPEN_PORT}"));
    for port in [EXEC_PORT, HOST_PORT].into_iter().chain(VIEWER_PORTS).chain(tunnel.then_some(EGRESS_PORT)) {
        args.push("-p".to_owned());
        args.push(format!("127.0.0.1:{PORT_FIRST}-{DOCKER_PORT_LAST}:{port}"));
    }
    for a in ["--entrypoint", "sleep", image, "infinity"] {
        args.push(a.to_owned());
    }
    args
}

/// The `docker create` arguments of a new computer: no secret among them, and no network of its
/// own. With `egress`, every screen's browser goes through the box's proxy under the rules staged
/// beside the secrets, and with the tunnel its bearer is a file there too.
pub(crate) fn create_args(image: &str, name: &str, id: &str, secrets: &Path, egress: Option<&Egress>) -> Vec<String> {
    let mut args: Vec<String> = ["create", "--name", name, "--network"].map(str::to_owned).to_vec();
    args.push(format!("container:{}", holder(name)));
    args.push("--label".to_owned());
    args.push(format!("{LABEL}=1"));
    args.push("--label".to_owned());
    args.push(format!("{LABEL}.id={id}"));
    for (source, target) in [(format!("{name}-workspace"), "/workspace"), (format!("{name}-home"), "/home/box")] {
        args.push("--mount".to_owned());
        args.push(format!("type=volume,source={source},target={target}"));
    }
    args.push("--mount".to_owned());
    args.push(format!("type=bind,source={},target={SECRET_MOUNT},readonly", secrets.display()));
    // Chromium needs more than Docker's 64 MiB; no setuid program may lift a user of the box
    for a in ["--user", "0", "--cap-drop", "ALL", "--shm-size", "1g", "--memory", "4g", "--pids-limit", "2048", "--security-opt", "no-new-privileges"] {
        args.push(a.to_owned());
    }
    for cap in BOX_CAPS {
        args.push("--cap-add".to_owned());
        args.push(cap.to_owned());
    }
    for env in [
        format!("BOX_TOKEN_FILE={SECRET_MOUNT}/{TOKEN_FILE}"),
        format!("BOX_VIEWER_PASSWORD_FILE={SECRET_MOUNT}/{VIEWER_FILE}"),
        "BOX_DESKTOP=1".to_owned(),
        "BOX_DESKTOP_REQUIRED=1".to_owned(),
        "BOX_CHROME=0".to_owned(),
    ] {
        args.push("-e".to_owned());
        args.push(env);
    }
    if let Some(egress) = egress {
        let mut env = vec!["BOX_EGRESS_PROXY=1".to_owned(), format!("BOX_EGRESS_RULES_FILE={SECRET_MOUNT}/{RULES_FILE}")];
        if egress.tunnel {
            env.push("BOX_EGRESS_TUNNEL=1".to_owned());
            env.push(format!("BOX_EGRESS_TUNNEL_BEARER_FILE={SECRET_MOUNT}/{EGRESS_FILE}"));
        }
        for e in env {
            args.push("-e".to_owned());
            args.push(e);
        }
    }
    args.push(image.to_owned());
    args
}

/// What a computer made for `network` is made with: the tunnel's bearer is new for each computer.
fn egress_for(network: Network) -> Result<Option<Egress>> {
    Ok(match network {
        Network::Plain => None,
        Network::Proxy => Some(Egress { tunnel: false, bearer: None }),
        Network::Tunnel => Some(Egress { tunnel: true, bearer: Some(Secret::fresh(BEARER_BYTES)?) }),
    })
}

fn tunnel_of(machine: &Machine) -> bool {
    machine.egress.as_ref().is_some_and(|e| e.tunnel)
}

fn container(machine: &Machine) -> Result<&str> {
    match &machine.reach {
        Reach::Container { name } => Ok(name),
        _ => Err(Problem::Failed("this computer isn't a container".to_owned())),
    }
}

fn secret_dir(dir: &Path) -> PathBuf {
    dir.join("start")
}

fn io_problem(e: std::io::Error) -> Problem {
    Problem::Failed(format!("its folder: {e}"))
}

/// A folder only this user reads, whatever the umask: `create_dir` modes are filtered by it, and
/// the folder may already be there.
fn private_dir(dir: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(dir).map_err(io_problem)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).map_err(io_problem)
}

/// The secrets, on disk for as long as this lives: dropped, however a start ends, they're gone.
struct Staged(PathBuf);

impl Staged {
    fn new(dir: PathBuf, machine: &Machine) -> Result<Self> {
        let staged = Self(dir);
        stage_secrets(&staged.0, machine)?;
        Ok(staged)
    }
}

impl Drop for Staged {
    fn drop(&mut self) {
        clear_secrets(&self.0);
    }
}

fn stage_secrets(dir: &Path, machine: &Machine) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    private_dir(dir)?;
    let bearer = machine.egress.as_ref().filter(|e| e.tunnel).and_then(|e| e.bearer.as_ref()).map(|b| (EGRESS_FILE, b));
    for (file, secret) in [(TOKEN_FILE, &machine.token), (VIEWER_FILE, &machine.viewer_password)].into_iter().chain(bearer) {
        let path = dir.join(file);
        // whatever is there goes, link or not, and this is a new file: a link is never followed
        match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(io_problem(e)),
            _ => {}
        }
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path).map_err(io_problem)?;
        f.write_all(secret.expose().as_bytes()).map_err(io_problem)?;
    }
    Ok(())
}

fn clear_secrets(dir: &Path) {
    for file in [TOKEN_FILE, VIEWER_FILE, EGRESS_FILE] {
        let _ = std::fs::remove_file(dir.join(file));
    }
}

/// Puts `rules` (a rules document) where a computer made with the proxy reads them at each start,
/// in place of what was there, whole or not at all. No file there is no rules: the box's browser
/// reaches no site until a set is put in force.
pub fn stage_rules(machine_dir: &Path, rules: &[u8]) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let dir = secret_dir(machine_dir);
    private_dir(&dir)?;
    let (path, next) = (dir.join(RULES_FILE), dir.join(format!("{RULES_FILE}.new")));
    let _ = std::fs::remove_file(&next);
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&next).map_err(io_problem)?;
    f.write_all(rules).map_err(io_problem)?;
    f.sync_all().map_err(io_problem)?;
    std::fs::rename(&next, &path).map_err(io_problem)
}

/// Takes the staged rules away: the box's browser then reaches no site after its next start.
pub fn unstage_rules(machine_dir: &Path) {
    let _ = std::fs::remove_file(secret_dir(machine_dir).join(RULES_FILE));
}

impl DockerProvider {
    /// Whether this provider's image says its browser can go through its own proxy. Not knowing is no.
    async fn has_proxy(&self) -> bool {
        let label = format!("{{{{index .Config.Labels \"{EGRESS_LABEL}\"}}}}");
        self.docker(&["image", "inspect", "--format", &label, &self.image], None).await.is_ok_and(|v| v.trim() == "1")
    }

    /// Whether `owner` is a box name [`Provider::create`] makes: the prefix and four random bytes.
    fn named_here(&self, owner: &str) -> bool {
        owner.strip_prefix(&self.prefix).and_then(|r| r.strip_prefix('-')).is_some_and(|hex| hex.len() == 8 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
    }

    /// The container and its volumes. Already gone is fine.
    async fn remove(&self, name: &str) -> Result<()> {
        self.walled.lock().unwrap_or_else(|e| e.into_inner()).remove(name);
        for container in [name.to_owned(), holder(name)] {
            match self.docker(&["rm", "-f", "-v", &container], None).await {
                Ok(_) | Err(Problem::Gone) => {}
                Err(e) => return Err(e),
            }
        }
        for volume in [format!("{name}-workspace"), format!("{name}-home")] {
            match self.docker(&["volume", "rm", &volume], None).await {
                Ok(_) | Err(Problem::Gone) => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
}

#[async_trait]
impl Provider for DockerProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Docker
    }

    async fn check(&self) -> Result<()> {
        self.docker(&["version", "--format", "{{.Server.Version}}"], None).await?;
        let label = format!("{{{{index .Config.Labels \"{USERS_LABEL}\"}}}}");
        let found = self.docker(&["image", "inspect", "--format", &label, &self.image], None).await.map_err(|e| match e {
            Problem::Gone => Problem::ImageMissing(self.image.clone()),
            other => other,
        })?;
        // an image from before users is as good as none: a computer mustn't start on it
        if found.trim() == USERS_VERSION {
            Ok(())
        } else {
            Err(Problem::ImageMissing(self.image.clone()))
        }
    }

    async fn create(&self, spec: &Spec) -> Result<Machine> {
        self.check().await?;
        self.ensure_network().await?;
        let name = format!("{}-{}", self.prefix, random_hex(4)?);
        let start = secret_dir(&spec.dir);
        private_dir(&start)?;
        let network = if spec.network != Network::Plain && !self.has_proxy().await { Network::Plain } else { spec.network };
        let egress = egress_for(network)?;
        if let Err(e) = self.make_containers(&self.image, &name, &spec.id, &start, egress.as_ref()).await {
            // nothing is on it yet; if this fails too, the hub sweeps by label
            let _ = self.remove(&name).await;
            return Err(e);
        }
        Ok(Machine {
            provider: ProviderKind::Docker,
            reach: Reach::Container { name },
            image: Some(self.image.clone()),
            token: Secret::fresh(24)?,
            viewer_password: Secret::fresh(24)?,
            dir: spec.dir.clone(),
            link: None,
            egress,
            seen: None,
        })
    }

    async fn power(&self, machine: &Machine) -> Result<Power> {
        match self.inspect(container(machine)?).await {
            Ok(found) => Ok(power_of(&found.status)),
            Err(Problem::Gone) => Ok(Power::Absent),
            Err(e) => Err(e),
        }
    }

    async fn look(&self, machine: &Machine) -> Result<(Power, Machine)> {
        match self.inspect(container(machine)?).await {
            Ok(found) => Ok((power_of(&found.status), Machine { seen: Some(found), ..machine.clone() })),
            Err(Problem::Gone) => Ok((Power::Absent, machine.clone())),
            Err(e) => Err(e),
        }
    }

    async fn wake(&self, machine: &Machine) -> Result<Woke> {
        let name = container(machine)?;
        let found = match self.inspect(name).await {
            Ok(found) if !found.net_missing && found.users_apart => found,
            Ok(found) => {
                self.rebuild(machine, name, Some(&found)).await?;
                self.inspect(name).await?
            }
            Err(Problem::Gone) => {
                self.rebuild(machine, name, None).await?;
                self.inspect(name).await?
            }
            Err(e) => return Err(e),
        };
        let up = matches!(found.status.as_str(), "paused" | "running");
        // the network went away under a running box, or came back since it started: only a fresh
        // start joins it
        let stale = up && (!found.net_running || older(&found.box_started, &found.started));
        if stale {
            self.halt(name, &found, STOP_SECONDS).await;
        }
        // first the wall, then anything of the box
        self.net_up(name, &found, true, tunnel_of(machine)).await?;
        match found.status.as_str() {
            "paused" if !stale => {
                self.docker(&["unpause", name], None).await?;
                Ok(Woke::Resumed)
            }
            "running" if !stale => Ok(Woke::Started),
            _ => self.start(machine, name).await.map(|()| Woke::Started),
        }
    }

    async fn sleep(&self, machine: &Machine, depth: Sleep) -> Result<()> {
        let name = container(machine)?;
        let status = self.inspect(name).await?.status;
        match (depth, status.as_str()) {
            (Sleep::Light, "running") => self.docker(&["pause", name], None).await.map(|_| ()),
            (Sleep::Light, _) => Ok(()),
            (Sleep::Deep, "paused") => {
                self.docker(&["unpause", name], None).await?;
                self.docker(&["stop", "-t", STOP_SECONDS, name], None).await.map(|_| ())
            }
            (Sleep::Deep, "running") => self.docker(&["stop", "-t", STOP_SECONDS, name], None).await.map(|_| ()),
            (Sleep::Deep, _) => Ok(()),
        }
    }

    async fn destroy(&self, machine: &Machine) -> Result<()> {
        self.remove(container(machine)?).await
    }

    async fn sweep(&self, id: &str, last: Option<&Reach>) -> Result<()> {
        let filter = format!("label={LABEL}.id={id}");
        let names = self.docker(&["ps", "-a", "--filter", &filter, "--format", "{{.Names}}"], None).await?;
        let mut boxes: Vec<&str> = names.lines().map(str::trim).filter(|n| !n.is_empty()).map(|n| n.strip_suffix("-wall").unwrap_or(n)).collect();
        if let Some(Reach::Container { name }) = last {
            boxes.push(name);
        }
        boxes.sort_unstable();
        boxes.dedup();
        for name in boxes {
            self.remove(name).await?;
        }
        Ok(())
    }

    /// Volumes of a computer, by its label or by this provider's naming, whose box container is
    /// gone. A box's own volumes are only ever its two, so its name says whose they were.
    async fn leftovers(&self) -> Result<Option<Vec<Leftover>>> {
        let format = format!("{{{{.Name}}}}\t{{{{.Label \"{LABEL}.id\"}}}}");
        // both at once: the hub waits on the pair for a few seconds at most
        let (ls, ps) = (["volume", "ls", "--format", format.as_str()], ["ps", "-a", "--format", "{{.Names}}"]);
        let (volumes, containers) = tokio::try_join!(self.docker(&ls, None), self.docker(&ps, None))?;
        let there: HashSet<&str> = containers.lines().map(str::trim).collect();
        let mut out: Vec<Leftover> = volumes
            .lines()
            .filter_map(|line| {
                let (name, id) = line.split_once('\t').unwrap_or((line, ""));
                let (name, id) = (name.trim(), id.trim());
                let owner = name.strip_suffix("-workspace").or_else(|| name.strip_suffix("-home"))?;
                (!there.contains(owner) && (!id.is_empty() || self.named_here(owner))).then(|| Leftover { name: name.to_owned(), id: (!id.is_empty()).then(|| id.to_owned()), owner: owner.to_owned() })
            })
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(Some(out))
    }

    async fn recover(&self, spec: &Spec, last: Option<&Reach>) -> Result<Option<Machine>> {
        let filter = format!("label={LABEL}.id={}", spec.id);
        let names = self.docker(&["ps", "-a", "--filter", &filter, "--format", "{{.Names}}"], None).await?;
        let found = names.lines().map(str::trim).find(|n| !n.is_empty()).map(|n| n.strip_suffix("-wall").unwrap_or(n).to_owned());
        let name = match (found, last) {
            (Some(name), _) => name,
            // no container left: its files may be
            (None, Some(Reach::Container { name })) => {
                if !self.any_volume(name).await? {
                    return Ok(None);
                }
                name.clone()
            }
            (None, _) => return Ok(None),
        };
        // as it was made, by what Docker says its containers were made with, never by what's wanted;
        // with only its files left, as a new computer would be on this image, which the wake makes
        // it again on: with the proxy only if the image has it. Not knowing is without: its rules
        // are refused then, never claimed
        let egress = match self.inspect(&name).await {
            Ok(found) => match found.egress {
                Some(tunnel) => egress_for(if tunnel { Network::Tunnel } else { Network::Proxy })?,
                None => None,
            },
            Err(Problem::Gone) if self.has_proxy().await => egress_for(spec.network)?,
            Err(_) => None,
        };
        Ok(Some(Machine {
            provider: ProviderKind::Docker,
            reach: Reach::Container { name },
            image: Some(self.image.clone()),
            token: Secret::fresh(24)?,
            viewer_password: Secret::fresh(24)?,
            dir: spec.dir.clone(),
            link: None,
            egress,
            seen: None,
        }))
    }

    async fn guard(&self, machine: &Machine) -> Result<()> {
        let name = container(machine)?;
        let found = self.seen(machine).await?;
        if !found.net_running || older(&found.box_started, &found.started) {
            self.halt(name, &found, AT_ONCE).await;
            return Err(Problem::Unwalled("its network changed under it".to_owned()));
        }
        // from before users, and nothing shipped ran one: a person's next click makes it again
        if !found.users_apart {
            self.halt(name, &found, STOP_SECONDS).await;
            return Err(Problem::Asleep);
        }
        self.net_up(name, &found, false, tunnel_of(machine)).await
    }

    async fn clear_strays(&self, machine: &Machine, screen: &Screen) -> Result<Vec<String>> {
        let name = container(machine)?;
        let bot = screen.bot().ok_or_else(|| Problem::Failed("this screen has no user of its own".to_owned()))?;
        let out = self.docker(&["exec", "--user", "0", "--privileged", name, "/usr/bin/python3", "-I", "-S", "-c", CLEAR, bot], None).await?;
        serde_json::from_str(out.trim()).map_err(|_| Problem::Failed("the programs left on it couldn't be told apart".to_owned()))
    }

    async fn users_apart(&self, machine: &Machine, bots: &[String]) -> Result<bool> {
        let name = container(machine)?;
        if !self.seen(machine).await?.users_apart {
            return Ok(false);
        }
        let mut args = vec!["exec", "--user", "0", "--privileged", name, "/usr/bin/python3", "-I", "-S", "-c", APART];
        args.extend(bots.iter().map(String::as_str));
        let out = self.docker(&args, None).await?;
        if out.trim() != "apart" {
            eprintln!("buwiz-computer: its Bots aren't apart: {}", out.trim());
        }
        Ok(out.trim() == "apart")
    }

    async fn screens(&self, machine: &Machine) -> Result<Vec<String>> {
        self.guest(machine).await?.screens().await
    }

    async fn resolve(&self, machine: &Machine, path: &str) -> Result<String> {
        self.guest(machine).await?.resolve(path).await
    }

    async fn screen_up(&self, machine: &Machine, bot: &str) -> Result<Screen> {
        self.guest(machine).await?.screen_up(bot).await
    }

    async fn screen_down(&self, machine: &Machine, bot: &str) -> Result<()> {
        self.guest(machine).await?.screen_down(bot).await
    }

    async fn viewer(&self, machine: &Machine, screen: &Screen) -> Result<Option<Viewer>> {
        let found = self.seen(machine).await?;
        Ok(found.published(screen.viewer_port).map(|addr| Viewer { addr, password: machine.viewer_password.clone(), far: None }))
    }

    async fn screenshot(&self, machine: &Machine, screen: &Screen) -> Result<Screenshot> {
        self.guest(machine).await?.screenshot(screen).await
    }

    async fn input(&self, machine: &Machine, screen: &Screen, input: &Input) -> Result<()> {
        self.guest(machine).await?.input(screen, input).await
    }

    async fn run(&self, machine: &Machine, screen: &Screen, command: &str, timeout_ms: u64) -> Result<CommandOutput> {
        self.guest(machine).await?.run(screen, command, timeout_ms.clamp(1000, 600_000)).await
    }

    async fn open_url(&self, machine: &Machine, screen: &Screen, url: &str) -> Result<()> {
        self.guest(machine).await?.open_url(screen, url).await
    }

    async fn browser(&self, machine: &Machine, screen: &Screen, op: crate::BrowserOp, body: &serde_json::Value) -> Result<crate::BrowserReply> {
        self.guest(machine).await?.browser(screen, op, body).await
    }

    async fn active_tab(&self, machine: &Machine, screen: &Screen) -> Result<crate::ActiveTab> {
        self.guest(machine).await?.active_tab(screen).await
    }

    async fn capabilities(&self, machine: &Machine) -> Result<serde_json::Value> {
        self.guest(machine).await?.capabilities().await
    }

    async fn egress(&self, machine: &Machine) -> Result<serde_json::Value> {
        self.guest(machine).await?.egress().await
    }

    async fn set_egress_rules(&self, machine: &Machine, rules: &serde_json::Value) -> Result<serde_json::Value> {
        self.guest(machine).await?.put_egress_rules(rules).await
    }

    async fn tunnel_url(&self, machine: &Machine) -> Result<Option<String>> {
        if !tunnel_of(machine) {
            return Ok(None);
        }
        let found = self.seen(machine).await?;
        if found.status != "running" {
            return Ok(None);
        }
        Ok(found.published(EGRESS_PORT).map(|addr| format!("ws://{addr}")))
    }

    async fn read_file(&self, machine: &Machine, path: &str) -> Result<Vec<u8>> {
        self.guest(machine).await?.read_file(path).await
    }

    async fn write_file(&self, machine: &Machine, path: &str, content: &[u8]) -> Result<()> {
        self.guest(machine).await?.write_file(path, content).await
    }

    async fn mark(&self, machine: &Machine, screen: &Screen) -> Option<u64> {
        self.guest(machine).await.ok()?.mark(screen).await
    }

    async fn settle(&self, machine: &Machine, screen: &Screen, wait: WaitFor) -> Result<Settled> {
        self.guest(machine).await?.settle(screen, wait).await
    }

    async fn record_start(&self, machine: &Machine, screen: &Screen) -> Result<u64> {
        self.guest(machine).await?.record_start(screen).await
    }

    async fn record_stop(&self, machine: &Machine, screen: &Screen, id: u64) -> Result<String> {
        self.guest(machine).await?.record_stop(screen, id).await
    }
}

/// `docker`'s state word as a [`Power`].
pub fn power_of(status: &str) -> Power {
    match status {
        "running" => Power::Awake,
        "paused" | "exited" | "created" | "stopped" => Power::Asleep,
        "restarting" => Power::Waking,
        other => Power::Broken(format!("Docker says it is {other}.")),
    }
}

/// What `docker inspect` said.
pub type Inspected = Seen;

impl Inspected {
    /// `name`'s entry decides whether the computer exists; its holder's may be missing.
    pub fn parse(json: &str, name: &str) -> Result<Self> {
        let v: Value = serde_json::from_str(json).map_err(|e| Problem::Failed(format!("docker inspect: {e}")))?;
        let entries = v.as_array().ok_or(Problem::Gone)?;
        let named = |n: &str| entries.iter().find(|e| e.get("Name").and_then(Value::as_str) == Some(n));
        let entry = named(&format!("/{name}")).ok_or(Problem::Gone)?;
        let null = Value::Null;
        let net = named(&format!("/{}", holder(name)));
        let text = |v: &Value, at: &str| v.pointer(at).and_then(Value::as_str).unwrap_or_default().to_owned();
        let status = text(entry, "/State/Status");
        let box_started = text(entry, "/State/StartedAt");
        let net_missing = net.is_none();
        let net = net.unwrap_or(&null);
        let started = text(net, "/State/StartedAt");
        let net_running = text(net, "/State/Status") == "running";
        let mut ports = HashMap::new();
        for (spec, bindings) in net.pointer("/NetworkSettings/Ports").and_then(Value::as_object).into_iter().flatten() {
            let Some(guest) = spec.strip_suffix("/tcp").and_then(|p| p.parse::<u16>().ok()) else { continue };
            // loopback, or nothing: a binding anywhere else isn't ours to use
            let found = bindings.as_array().into_iter().flatten().find(|b| b.get("HostIp").and_then(Value::as_str) == Some("127.0.0.1"));
            if let Some(port) = found.and_then(|b| b.get("HostPort")).and_then(Value::as_str).filter(|p| !p.is_empty()) {
                ports.insert(guest, format!("127.0.0.1:{port}"));
            }
        }
        let users = |v: &Value| v.pointer("/Config/Labels").and_then(|l| l.get(USERS_LABEL)).and_then(Value::as_str) == Some(USERS_VERSION);
        let users_apart = users(entry) && users(net);
        let env: Vec<&str> = entry.pointer("/Config/Env").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect();
        // the tunnel by both halves of it: the box's setting, and its holder's published port
        let binds = |v: &Value| v.pointer("/HostConfig/PortBindings").and_then(Value::as_object).is_some_and(|b| b.contains_key(&format!("{EGRESS_PORT}/tcp")));
        let egress = env.contains(&"BOX_EGRESS_PROXY=1").then(|| env.contains(&"BOX_EGRESS_TUNNEL=1") && binds(net));
        Ok(Self { status, started, net_running, net_missing, box_started, users_apart, ports, egress })
    }

    pub fn published(&self, guest_port: u16) -> Option<String> {
        self.ports.get(&guest_port).cloned()
    }
}

fn home_dir(dir: &str) -> PathBuf {
    match (dir.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ => PathBuf::from(dir),
    }
}

/// `PATH` with Docker's folders on it, worked out once: an app from Finder has a bare one, and
/// its credential helpers sit next to it.
fn docker_path() -> &'static std::ffi::OsStr {
    static P: OnceLock<std::ffi::OsString> = OnceLock::new();
    P.get_or_init(|| {
        let path = std::env::var_os("PATH").unwrap_or_default();
        let dirs: Vec<PathBuf> = std::env::split_paths(&path).chain(DOCKER_DIRS.map(home_dir)).collect();
        std::env::join_paths(dirs).unwrap_or(path)
    })
}

/// `docker`: on [`docker_path`].
fn find_docker() -> PathBuf {
    std::env::split_paths(docker_path()).map(|d| d.join("docker")).find(|p| p.is_file()).unwrap_or_else(|| PathBuf::from("docker"))
}

struct Raw {
    status: Option<i32>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

async fn run_docker(program: &Path, args: &[&str], stdin: Option<&[u8]>, patience: Duration) -> Result<Raw> {
    let mut command = Command::new(program);
    command.env("PATH", docker_path());
    command.args(args).stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() }).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    let mut child = command.spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            Problem::ProviderMissing("Docker".to_owned())
        } else {
            Problem::Failed(format!("docker wouldn't start: {e}"))
        }
    })?;
    if let (Some(bytes), Some(mut pipe)) = (stdin, child.stdin.take()) {
        pipe.write_all(bytes).await.map_err(|e| Problem::Failed(format!("docker's input: {e}")))?;
    }
    match tokio::time::timeout(patience, child.wait_with_output()).await {
        Ok(Ok(out)) => Ok(Raw { status: out.status.code(), stdout: out.stdout, stderr: out.stderr }),
        Ok(Err(e)) => Err(Problem::Failed(format!("docker: {e}"))),
        Err(_) => Err(Problem::ProviderDown("Docker".to_owned())),
    }
}

/// Docker's stderr as the problem a person can act on.
fn classify(status: Option<i32>, stderr: &str, image: &str) -> Problem {
    // Docker's case isn't steady: `No such container`, but `get x: no such volume`
    let lower = stderr.to_lowercase();
    let said = |words: &[&str]| words.iter().any(|w| lower.contains(w));
    if said(&["no such image", "unable to find image", "pull access denied"]) {
        Problem::ImageMissing(image.to_owned())
    } else if said(&["no such container", "no such object", "no such volume"]) {
        Problem::Gone
    } else if said(&["cannot connect to the docker daemon", "is the docker daemon running", "docker daemon is not running", "failed to connect to the docker api"]) {
        Problem::ProviderDown("Docker".to_owned())
    } else {
        let first = stderr.lines().next().unwrap_or_default().trim();
        Problem::Failed(if first.is_empty() { format!("docker exited with {status:?}") } else { first.to_owned() })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn the_id_label_is_the_one_made_things_carry() {
        assert_eq!(ID_LABEL, format!("{LABEL}.id"));
    }

    fn fake_docker(dir: &Path, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join("docker");
        std::fs::write(&path, format!("#!/bin/sh\necho \"$*\" >> \"{}/calls\"\n{body}\n", dir.display())).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("buwiz-computer-{name}-{}", random_hex(4).unwrap()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn machine(dir: &Path) -> Machine {
        Machine {
            provider: ProviderKind::Docker,
            reach: Reach::Container { name: "c2-x".into() },
            image: None,
            token: Secret::new("the-token"),
            viewer_password: Secret::new("the-password"),
            dir: dir.to_path_buf(),
            link: None,
            egress: None,
            seen: None,
        }
    }

    #[test]
    fn the_holder_publishes_on_loopback_in_the_range_and_the_box_only_joins_it() {
        let net = holder_args("img:1", "c2-x", "c2-net", "cmp_1", false).join(" ");
        assert!(net.contains("--cap-drop ALL") && !net.contains("--cap-add"), "{net}");
        assert!(net.contains("app.buwiz.computer.id=cmp_1"), "a sweep finds it by this: {net}");
        for port in [1337, 1340, 6901, 6902, 6905] {
            assert!(net.contains(&format!("-p 127.0.0.1:47000-47199:{port}")), "{net}");
        }
        assert!(!net.contains("0.0.0.0"), "{net}");
        assert!(net.contains("--name c2-x-wall") && net.contains("--entrypoint sleep img:1 infinity"), "{net}");
        assert!(net.contains("disable_ipv6=1") && !net.contains("BOX_"), "{net}");
        let args = create_args("img:1", "c2-x", "cmp_1", Path::new("/d/start"), None).join(" ");
        assert!(args.contains("--cap-drop ALL"), "{args}");
        for cap in BOX_CAPS {
            assert!(args.contains(&format!("--cap-add {cap}")), "{args}");
        }
        for cap in ["NET_RAW", "NET_ADMIN", "SYS_ADMIN", "SYS_PTRACE", "MKNOD", "SETPCAP", "SETFCAP"] {
            assert!(!args.contains(cap), "{cap}: {args}");
        }
        assert!(net.contains("net.ipv4.ip_unprivileged_port_start=6906"), "the port floor is the holder's: {net}");
        assert!(args.contains("--network container:c2-x-wall"), "{args}");
        assert!(!args.contains("-p "), "a box in another's network publishes nothing: {args}");
        assert!(args.contains("BOX_TOKEN_FILE=/run/buwiz-computer/token"), "{args}");
        assert!(args.contains("target=/run/buwiz-computer,readonly"), "the box can't plant anything there: {args}");
        assert!(args.contains("BOX_VIEWER_PASSWORD_FILE=/run/buwiz-computer/viewer"), "{args}");
        assert!(!args.contains("BOX_TOKEN=") && !args.contains("BOX_VIEWER_PASSWORD="), "{args}");
        assert!(args.contains("no-new-privileges") && args.contains("--pids-limit 2048"), "{args}");
    }

    #[test]
    fn a_computer_made_without_the_proxy_publishes_and_sets_nothing_of_it() {
        let net = holder_args("img:1", "c2-x", "c2-net", "cmp_1", false).join(" ");
        assert!(!net.contains("8790"), "{net}");
        let args = create_args("img:1", "c2-x", "cmp_1", Path::new("/d/start"), None).join(" ");
        assert!(!args.contains("BOX_EGRESS"), "{args}");
    }

    #[test]
    fn the_tunnel_publishes_its_port_on_loopback_and_its_bearer_is_a_file() {
        let net = holder_args("img:1", "c2-x", "c2-net", "cmp_1", true).join(" ");
        assert!(net.contains("-p 127.0.0.1:47000-47199:8790"), "{net}");
        assert!(!net.contains("8791") && !net.contains("8792"), "the proxy and its admin port are never published: {net}");
        let egress = Egress { tunnel: true, bearer: Some(Secret::new("bearer-in-a-file-only")) };
        let args = create_args("img:1", "c2-x", "cmp_1", Path::new("/d/start"), Some(&egress)).join(" ");
        for env in ["BOX_EGRESS_PROXY=1", "BOX_EGRESS_RULES_FILE=/run/buwiz-computer/rules.json", "BOX_EGRESS_TUNNEL=1", "BOX_EGRESS_TUNNEL_BEARER_FILE=/run/buwiz-computer/egress"] {
            assert!(args.contains(&format!("-e {env}")), "{env}: {args}");
        }
        assert!(!args.contains("bearer-in-a-file-only") && !args.contains("BOX_EGRESS_TUNNEL_BEARER="), "{args}");
        let proxy = create_args("img:1", "c2-x", "cmp_1", Path::new("/d/start"), Some(&Egress { tunnel: false, bearer: None })).join(" ");
        assert!(proxy.contains("-e BOX_EGRESS_PROXY=1") && !proxy.contains("BOX_EGRESS_TUNNEL"), "{proxy}");
    }

    #[tokio::test]
    async fn a_computer_is_made_with_the_proxy_only_on_an_image_that_has_it() {
        let made = |dir: &Path, label: &str| {
            let body = format!("case \"$1 $4\" in version*) echo 29;; image*app.buwiz.box.egress*) echo '{label}';; image*) echo 1;; esac");
            DockerProvider::new("img:1", "c2").with_program(fake_docker(dir, &body))
        };
        let dir = tmp("label-new");
        let m = made(&dir, "1").create(&Spec { id: "cmp_1".into(), dir: dir.clone(), network: Network::Tunnel }).await.unwrap();
        assert!(m.egress.is_some_and(|e| e.tunnel));
        assert!(calls(&dir).contains("BOX_EGRESS_PROXY=1"), "{}", calls(&dir));
        let dir = tmp("label-old");
        let m = made(&dir, "").create(&Spec { id: "cmp_1".into(), dir: dir.clone(), network: Network::Tunnel }).await.unwrap();
        assert_eq!(m.egress, None, "an image from before the proxy makes a computer as before");
        assert!(!calls(&dir).contains("BOX_EGRESS") && !calls(&dir).contains(":8790"), "{}", calls(&dir));
    }

    #[test]
    fn a_new_computers_bearer_is_32_random_bytes_and_each_its_own() {
        let a = egress_for(Network::Tunnel).unwrap().unwrap().bearer.unwrap();
        let b = egress_for(Network::Tunnel).unwrap().unwrap().bearer.unwrap();
        assert_eq!(a.expose().len(), 64, "32 bytes, as hex");
        assert_ne!(a, b);
        assert_eq!(egress_for(Network::Plain).unwrap(), None);
        assert_eq!(egress_for(Network::Proxy).unwrap(), Some(Egress { tunnel: false, bearer: None }));
    }

    /// `docker inspect` of a box made with `env`, its holder publishing `ports`.
    fn made(env: &str, ports: &str) -> String {
        format!(r#"[{{"Name":"/c2-x","Config":{{"Env":[{env}]}},"State":{{"Status":"exited"}}}},{{"Name":"/c2-x-wall","HostConfig":{{"PortBindings":{{{ports}}}}},"State":{{"Status":"running"}}}}]"#)
    }

    #[test]
    fn what_a_container_was_made_with_is_read_from_its_settings_and_its_holders_ports() {
        let port = r#""8790/tcp":[{"HostIp":"127.0.0.1","HostPort":"47000-47199"}]"#;
        assert_eq!(Inspected::parse(&made(r#""BOX_TOKEN_FILE=/x""#, port), "c2-x").unwrap().egress, None);
        assert_eq!(Inspected::parse(&made(r#""BOX_EGRESS_PROXY=1""#, ""), "c2-x").unwrap().egress, Some(false));
        assert_eq!(Inspected::parse(&made(r#""BOX_EGRESS_PROXY=1","BOX_EGRESS_TUNNEL=1""#, port), "c2-x").unwrap().egress, Some(true));
        assert_eq!(Inspected::parse(&made(r#""BOX_EGRESS_PROXY=1","BOX_EGRESS_TUNNEL=1""#, ""), "c2-x").unwrap().egress, Some(false), "no published port, no tunnel");
    }

    #[tokio::test]
    async fn a_lost_record_is_recovered_as_its_containers_were_made_whatever_is_wanted() {
        let dir = tmp("recover-made");
        std::fs::write(dir.join("state"), made(r#""BOX_TOKEN_FILE=/x""#, "")).unwrap();
        let body = format!("case \"$1\" in ps) echo c2-x-wall; echo c2-x;; inspect) cat \"{}/state\";; image) echo 1;; esac", dir.display());
        let p = DockerProvider::new("img:1", "c2").with_program(fake_docker(&dir, &body));
        let m = p.recover(&Spec { id: "cmp_1".into(), dir: dir.clone(), network: Network::Tunnel }, None).await.unwrap().unwrap();
        assert_eq!(m.egress, None, "made before the proxy, it stays without it");
        std::fs::write(dir.join("state"), made(r#""BOX_EGRESS_PROXY=1","BOX_EGRESS_TUNNEL=1""#, r#""8790/tcp":[]"#)).unwrap();
        let m = p.recover(&Spec { id: "cmp_1".into(), dir: dir.clone(), network: Network::Plain }, None).await.unwrap().unwrap();
        assert!(m.egress.is_some_and(|e| e.tunnel && e.bearer.is_some()), "made with the tunnel, it keeps it, with a new bearer");
    }

    #[tokio::test]
    async fn files_alone_are_recovered_with_the_proxy_only_on_an_image_that_has_it() {
        for (label, proxied) in [("1", true), ("", false)] {
            let dir = tmp("recover-files");
            let body = format!("case \"$1 $4\" in ps*) ;; volume*) echo c2-x-home;; inspect*) echo '[]'; echo 'Error: No such container: c2-x' >&2; exit 1;; image*app.buwiz.box.egress*) echo '{label}';; image*) echo 1;; esac");
            let p = DockerProvider::new("img:1", "c2").with_program(fake_docker(&dir, &body));
            let m = p.recover(&Spec { id: "cmp_1".into(), dir: dir.clone(), network: Network::Tunnel }, Some(&Reach::Container { name: "c2-x".into() })).await.unwrap().unwrap();
            assert_eq!(m.egress.is_some(), proxied, "label {label:?}");
        }
    }

    #[test]
    fn only_a_loopback_binding_counts() {
        let json = r#"[{"Name":"/c2-x","State":{"Status":"running"}},{"Name":"/c2-x-wall","State":{"Status":"running","StartedAt":"T1"},"NetworkSettings":{"Ports":{
            "1337/tcp":[{"HostIp":"127.0.0.1","HostPort":"47001"}],
            "1340/tcp":[{"HostIp":"0.0.0.0","HostPort":"47002"}],
            "6901/tcp":null}}}]"#;
        let found = Inspected::parse(json, "c2-x").unwrap();
        assert_eq!(found.status, "running");
        assert_eq!(found.published(1337).as_deref(), Some("127.0.0.1:47001"));
        assert_eq!(found.published(1340), None);
        assert_eq!(found.published(6901), None);
    }

    #[test]
    fn dockers_states_are_a_power() {
        assert_eq!(power_of("running"), Power::Awake);
        assert_eq!(power_of("paused"), Power::Asleep);
        assert_eq!(power_of("exited"), Power::Asleep);
        assert_eq!(power_of("created"), Power::Asleep);
        assert_eq!(power_of("dead").word(), "error");
    }

    #[tokio::test]
    async fn docker_not_running_is_a_sentence() {
        let dir = tmp("down");
        let program = fake_docker(&dir, "echo 'Cannot connect to the Docker daemon at unix:///x. Is the docker daemon running?' >&2; exit 1");
        let p = DockerProvider::new("img:1", "c2").with_program(program);
        let e = p.check().await.unwrap_err();
        assert_eq!(e, Problem::ProviderDown("Docker".into()));
        assert!(e.sentence().starts_with("Docker isn't running."));
    }

    #[tokio::test]
    async fn docker_not_installed_is_a_sentence() {
        let p = DockerProvider::new("img:1", "c2").with_program(PathBuf::from("/nonexistent/docker"));
        assert_eq!(p.check().await.unwrap_err(), Problem::ProviderMissing("Docker".into()));
    }

    #[tokio::test]
    async fn a_missing_image_is_a_sentence() {
        let dir = tmp("image");
        let program = fake_docker(&dir, "case \"$1\" in version) echo 29.8;; image) echo 'Error response from daemon: No such image: img:1' >&2; exit 1;; esac");
        let p = DockerProvider::new("img:1", "c2").with_program(program);
        assert_eq!(p.check().await.unwrap_err(), Problem::ImageMissing("img:1".into()));
    }

    /// `docker inspect`'s answer: the box as `status`, its network up since `started`.
    fn inspected(status: &str, net: &str, started: &str) -> String {
        format!(r#"[{{"Name":"/c2-x","Config":{{"Labels":{{"app.buwiz.box.users":"1"}}}},"State":{{"Status":"{status}","StartedAt":"U"}}}},{{"Name":"/c2-x-wall","Config":{{"Labels":{{"app.buwiz.box.users":"1"}}}},"State":{{"Status":"{net}","StartedAt":"{started}"}},"NetworkSettings":{{"Ports":{{"1337/tcp":[{{"HostIp":"127.0.0.1","HostPort":"47001"}}],"1340/tcp":[{{"HostIp":"127.0.0.1","HostPort":"47002"}}],"6902/tcp":[{{"HostIp":"127.0.0.1","HostPort":"47003"}}]}}}}}}]"#)
    }

    /// A provider whose `docker` answers `inspect` from the file `state`, and `exec` and `start`
    /// as told. A good `ifconfig` beside it.
    fn faked(dir: &Path, state: &str, exec: &str, start: &str) -> DockerProvider {
        let body = format!("case \"$1\" in inspect) cat \"{}/state\";; exec) {exec};; start) {start};; esac", dir.display());
        std::fs::write(dir.join("state"), state).unwrap();
        DockerProvider::new("img:1", "c2").with_program(fake_docker(dir, &body)).with_ifconfig(PathBuf::from("/usr/bin/true"))
    }

    fn calls(dir: &Path) -> String {
        std::fs::read_to_string(dir.join("calls")).unwrap_or_default()
    }

    fn at(dir: &Path, word: &str) -> usize {
        calls(dir).find(word).unwrap_or_else(|| panic!("no {word} in {}", calls(dir)))
    }

    const WALL_CALL: &str = "exec --user 0 --privileged c2-x-wall";

    const NO_HOLDER: &str = r#"[{"Name":"/c2-x","State":{"Status":"exited","StartedAt":"U"}}]"#;

    /// A `docker` that says what real Docker says when one or both containers are missing: the
    /// entries it found, and exit 1. After the first `create`, they exist again.
    fn losing(dir: &Path, first: &str) -> DockerProvider {
        std::fs::write(dir.join("first"), first).unwrap();
        std::fs::write(dir.join("after"), inspected("exited", "running", "T1")).unwrap();
        let d = dir.display();
        let body = format!("case \"$1\" in image) echo 1;; inspect) if [ -e \"{d}/made\" ]; then cat \"{d}/after\"; else cat \"{d}/first\"; echo 'Error response from daemon: No such container: c2-x-wall' >&2; exit 1; fi;; create) touch \"{d}/made\";; start) exit 1;; esac");
        DockerProvider::new("img:1", "c2").with_program(fake_docker(dir, &body)).with_ifconfig(PathBuf::from("/usr/bin/true"))
    }

    #[tokio::test]
    async fn a_missing_holder_isnt_a_missing_computer() {
        let dir = tmp("no-holder");
        let p = losing(&dir, NO_HOLDER);
        assert_eq!(p.power(&machine(&dir)).await.unwrap(), Power::Asleep, "a missing holder read as a gone computer loses its files");
    }

    #[tokio::test]
    async fn a_lost_holder_or_box_is_rebuilt_on_the_same_volumes() {
        for (label, first) in [("holder", NO_HOLDER), ("both", "[]")] {
            let dir = tmp(label);
            let p = losing(&dir, first);
            let _ = p.wake(&machine(&dir)).await;
            let seen = calls(&dir);
            assert!(!seen.contains("volume rm") && !seen.contains("rm -f -v"), "{label}: {seen}");
            assert!(seen.contains("create --name c2-x-wall") && seen.contains("create --name c2-x "), "{label}: {seen}");
            assert!(seen.contains("source=c2-x-workspace") && seen.contains("source=c2-x-home"), "{label}: {seen}");
        }
    }

    /// A computer from before users, as Docker holds it: its containers have no label.
    fn before_users(status: &str) -> String {
        inspected(status, "running", "T1").replace(r#""Config":{"Labels":{"app.buwiz.box.users":"1"}},"#, "")
    }

    #[tokio::test]
    async fn a_computer_from_before_users_is_made_again_on_the_same_volumes() {
        for status in ["exited", "running", "paused"] {
            let dir = tmp(&format!("old-{status}"));
            let p = losing(&dir, &before_users(status));
            let _ = p.wake(&Machine { image: Some("old:1".into()), ..machine(&dir) }).await;
            let seen = calls(&dir);
            assert!(!seen.contains("volume rm") && !seen.contains("rm -f -v"), "{status}: {seen}");
            assert!(seen.contains("rm -f c2-x\n") && seen.contains("rm -f c2-x-wall\n"), "{status}: {seen}");
            assert!(seen.contains("source=c2-x-workspace") && seen.contains("source=c2-x-home"), "{status}: {seen}");
            assert!(seen.contains("img:1") && !seen.contains("old:1"), "on this provider's image, not the record's: {seen}");
        }
    }

    #[tokio::test]
    async fn with_no_image_to_make_it_from_an_old_computer_is_left_alone() {
        let dir = tmp("old-no-image");
        let body = format!("case \"$1\" in inspect) cat \"{}/state\";; esac", dir.display());
        std::fs::write(dir.join("state"), before_users("exited")).unwrap();
        let p = DockerProvider::new("img:1", "c2").with_program(fake_docker(&dir, &body)).with_ifconfig(PathBuf::from("/usr/bin/true"));
        assert_eq!(p.wake(&machine(&dir)).await.unwrap_err(), Problem::ImageMissing("img:1".into()));
        assert!(!calls(&dir).contains("rm "), "{}", calls(&dir));
    }

    #[tokio::test]
    async fn an_image_from_before_users_is_as_good_as_none() {
        let dir = tmp("old-image");
        let p = DockerProvider::new("img:1", "c2").with_program(fake_docker(&dir, "case \"$1\" in image) echo;; esac"));
        assert_eq!(p.check().await.unwrap_err(), Problem::ImageMissing("img:1".into()));
    }

    #[tokio::test]
    async fn a_running_computer_from_before_users_is_stopped_not_used() {
        let dir = tmp("old-guard");
        let p = faked(&dir, &before_users("running"), "true", "true");
        assert_eq!(p.guard(&machine(&dir)).await.unwrap_err(), Problem::Asleep);
        assert!(calls(&dir).contains("stop -t 5 c2-x"), "{}", calls(&dir));
        assert!(!calls(&dir).contains("volume"), "{}", calls(&dir));
    }

    #[test]
    fn one_labelled_container_of_two_isnt_enough() {
        let holder_only = inspected("running", "running", "T1").replacen(r#""Config":{"Labels":{"app.buwiz.box.users":"1"}},"#, "", 1);
        assert!(!Inspected::parse(&holder_only, "c2-x").unwrap().users_apart);
        assert!(Inspected::parse(&inspected("running", "running", "T1"), "c2-x").unwrap().users_apart);
    }

    #[tokio::test]
    async fn only_the_word_from_outside_says_bots_are_apart() {
        let bots = ["a".to_owned(), "b".to_owned()];
        for (exec, apart) in [("echo apart", true), ("echo 'a Bot folder is open'", false), ("echo", false), ("echo apartment", false)] {
            let dir = tmp("apart");
            let p = faked(&dir, &inspected("running", "running", "T1"), exec, "true");
            assert_eq!(p.users_apart(&machine(&dir), &bots).await.unwrap(), apart, "{exec}");
            assert!(calls(&dir).contains("exec --user 0 --privileged c2-x /usr/bin/python3 -I -S -c"), "{}", calls(&dir));
        }
        let dir = tmp("apart-fails");
        let p = faked(&dir, &inspected("running", "running", "T1"), "echo no >&2; exit 1", "true");
        assert!(p.users_apart(&machine(&dir), &bots).await.is_err(), "a check that can't run says nothing");
        let dir = tmp("apart-old");
        let p = faked(&dir, &before_users("running"), "echo apart", "true");
        assert!(!p.users_apart(&machine(&dir), &bots).await.unwrap(), "an unlabelled box isn't asked");
        assert!(!calls(&dir).contains("exec"), "{}", calls(&dir));
    }

    /// Runs [`CLEAR`] on this Mac against a made-up `/proc` and `/home/box` under argv[2]: the
    /// owner of each path is root unless `OWNERS` says, as the kernel shows a process that isn't
    /// dumpable, and a kill moves the process's folder away instead of signalling anything here. A
    /// link to `/vanished` reads as an fd closed after its folder was listed, one to `/exited` as one
    /// whose process exits as it's read (its folder moves to `exited-<pid>`), and one to `/denied` as
    /// one the kernel won't show.
    const FAKE_PROC: &str = r#"
import builtins, os, sys
ROOT = sys.argv[2]
OWNERS = {'/proc/400': 20001}
def _at(p):
    p = os.fspath(p)
    return ROOT + p if p.startswith(('/proc', '/home/box')) else p
_open, _listdir, _stat, _lstat, _readlink, _rename = builtins.open, os.listdir, os.stat, os.lstat, os.readlink, os.rename
def _owned(p, st):
    f = list(st)
    f[4] = OWNERS.get(os.fspath(p), 0)
    return os.stat_result(f)
builtins.open = lambda p, *a, **k: _open(_at(p), *a, **k)
os.listdir = lambda p: _listdir(_at(p))
os.stat = lambda p: _owned(p, _stat(_at(p)))
os.lstat = lambda p: _owned(p, _lstat(_at(p)))
def _link(p):
    target = _readlink(_at(p))
    if target == '/exited':
        pid = os.fspath(p).split('/')[2]
        _rename(ROOT + '/proc/' + pid, ROOT + '/exited-' + pid)
    if target in ('/vanished', '/exited'):
        raise FileNotFoundError(2, 'No such file or directory', p)
    if target == '/denied':
        raise PermissionError(13, 'Permission denied', p)
    return target
os.readlink = _link
os.kill = lambda pid, sig: _rename(ROOT + '/proc/%d' % pid, ROOT + '/ended-%d' % pid)
"#;

    /// A process in the made-up `/proc`: its `status` says `uid` and `parent`, and it runs `exe`.
    fn fake_process(root: &Path, pid: u32, parent: u32, name: &str, uid: u32, exe: &str) {
        let dir = root.join(format!("proc/{pid}"));
        std::fs::create_dir_all(dir.join(format!("task/{pid}"))).unwrap();
        let stat = format!("{pid} ({name}) S 1 1 1 0 -1 0\n");
        std::fs::write(dir.join("stat"), &stat).unwrap();
        std::fs::write(dir.join(format!("task/{pid}/stat")), &stat).unwrap();
        std::fs::write(dir.join("status"), format!("Name:\t{name}\nUmask:\t0022\nState:\tS (sleeping)\nPPid:\t{parent}\nUid:\t{uid}\t{uid}\t{uid}\t{uid}\nGid:\t{uid}\t{uid}\t{uid}\t{uid}\n")).unwrap();
        std::fs::write(dir.join("maps"), "").unwrap();
        std::os::unix::fs::symlink(exe, dir.join("exe")).unwrap();
    }

    /// A process that isn't dumpable has its `/proc` folder owned by root, whoever it runs as: only
    /// the `Uid:` line of its `status` says who that is. One of the Bot's like that is still found,
    /// and another user's is left alone whatever its folder's owner.
    #[test]
    fn clearing_reads_a_processs_user_from_its_status_not_its_folders_owner() {
        let root = tmp("fake-proc");
        std::fs::create_dir_all(root.join("home/box/.box/bots")).unwrap();
        std::fs::write(root.join("home/box/.box/bots/b_ann"), "0\n").unwrap();
        // the Bot's desktop user is 20001: this one is its, and not dumpable
        fake_process(&root, 300, 1, "hidden", 20001, "/home/box/hidden");
        fake_process(&root, 400, 1, "someone", 1000, "/home/box/someone");
        std::fs::create_dir_all(root.join("proc/self")).unwrap();
        let out = std::process::Command::new("python3").args(["-I", "-S", "-c", &format!("{FAKE_PROC}{CLEAR}"), "b_ann"]).arg(&root).output().expect("python3 runs the scan on this Mac");
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), r#"["hidden"]"#);
        assert!(root.join("ended-300").exists() && root.join("proc/400").exists(), "only the Bot's own is ended");
    }

    /// What a launcher or shell in the made-up `/proc` was started with: its arguments, its `PATH`,
    /// and whether it holds `box-chromium` open.
    fn fake_start(root: &Path, pid: u32, args: &[&str], path: &str, holds_script: bool) {
        let dir = root.join(format!("proc/{pid}"));
        std::fs::write(dir.join("cmdline"), args.iter().map(|a| format!("{a}\0")).collect::<String>()).unwrap();
        std::fs::write(dir.join("environ"), format!("HOME=/home/box/desktops/b_ann\0PATH={path}\0")).unwrap();
        std::fs::create_dir_all(dir.join("fd")).unwrap();
        std::os::unix::fs::symlink("/dev/null", dir.join("fd/0")).unwrap();
        if holds_script {
            std::os::unix::fs::symlink("/usr/local/bin/box-chromium", dir.join("fd/10")).unwrap();
        }
    }

    /// An open browser lives through a hand-over: its launcher chain is spared by the process tree
    /// and what each part was started with. A shell that only looks like `box-chromium`, a launcher
    /// that runs something else or no Chromium, and the desktop's own stray, are ended. A command's
    /// process isn't this scan's (it can't reach the desktop), and stays as before.
    #[test]
    fn clearing_spares_an_open_browsers_launcher_chain_and_nothing_that_looks_like_it() {
        const SH: &str = "/usr/bin/dash";
        const PIPE: &str = "/usr/local/bin/box-chrome-pipe";
        const CHROMIUM: &str = "/usr/lib/chromium/chromium";
        const SYSTEM: &str = "/usr/local/bin:/usr/bin:/bin";
        let launched = |sock: &'static str, program: &'static str| [PIPE, sock, "--", program, "--user-data-dir=/home/box/desktops/b_ann/chrome-profile", "about:blank"];
        let looped = ["/bin/sh", "/usr/local/bin/box-chromium", "about:blank"];
        let root = tmp("fake-proc-browser");
        std::fs::create_dir_all(root.join("home/box/.box/bots")).unwrap();
        std::fs::write(root.join("home/box/.box/bots/b_ann"), "0\n").unwrap();
        // the screen's browser as box-screen starts it: box-chromium's loop, the launcher, Chromium
        // and a helper of Chromium's
        fake_process(&root, 500, 1, "box-chromium", 20001, SH);
        fake_start(&root, 500, &looped, SYSTEM, true);
        fake_process(&root, 501, 500, "box-chrome-pipe", 20001, PIPE);
        fake_start(&root, 501, &launched("/run/box/x/2/run/cdp.sock", "chromium"), SYSTEM, false);
        fake_process(&root, 502, 501, "chromium", 20001, CHROMIUM);
        fake_process(&root, 503, 502, "chromium", 20001, CHROMIUM);
        // an fd the loop closes while it's read: it held nothing, and the loop is still the loop
        std::os::unix::fs::symlink("/vanished", root.join("proc/500/fd/11")).unwrap();
        // the same on an image whose /bin isn't merged into /usr, where dash runs as /bin/dash
        fake_process(&root, 510, 1, "box-chromium", 20001, "/bin/dash");
        fake_start(&root, 510, &looped, SYSTEM, true);
        fake_process(&root, 511, 510, "box-chrome-pipe", 20001, PIPE);
        fake_start(&root, 511, &launched("/run/box/x/2/run/cdp.sock", "chromium"), SYSTEM, false);
        fake_process(&root, 512, 511, "chromium", 20001, CHROMIUM);
        // a stray that exits as it's read: nothing to end, and the scan goes on
        fake_process(&root, 590, 1, "python3", 20001, "/exited");
        // something the desktop's user left running
        fake_process(&root, 600, 1, "python3", 20001, "/usr/bin/python3");
        // a shell of its own around a real launcher, holding box-chromium open to look like it
        fake_process(&root, 610, 1, "sh", 20001, SH);
        fake_start(&root, 610, &["sh", "-c", "while :; do box-chrome-pipe s -- chromium; /home/box/x; done"], SYSTEM, true);
        fake_process(&root, 611, 610, "box-chrome-pipe", 20001, PIPE);
        fake_start(&root, 611, &launched("s", "chromium"), SYSTEM, false);
        fake_process(&root, 612, 611, "chromium", 20001, CHROMIUM);
        // box-chromium on a PATH of the desktop's own: its chromium could be anything next time
        fake_process(&root, 620, 1, "box-chromium", 20001, SH);
        fake_start(&root, 620, &looped, "/home/box/desktops/b_ann/bin:/usr/bin", true);
        fake_process(&root, 621, 620, "box-chrome-pipe", 20001, PIPE);
        fake_start(&root, 621, &launched("s", "chromium"), "/home/box/desktops/b_ann/bin:/usr/bin", false);
        fake_process(&root, 622, 621, "chromium", 20001, CHROMIUM);
        // a launcher around something that isn't Chromium
        fake_process(&root, 630, 1, "box-chrome-pipe", 20001, PIPE);
        fake_start(&root, 630, &launched("s", "chromium"), SYSTEM, false);
        fake_process(&root, 631, 630, "python3", 20001, "/usr/bin/python3");
        // a launcher told to run a program by its path, and one with no Chromium under it
        fake_process(&root, 640, 1, "box-chrome-pipe", 20001, PIPE);
        fake_start(&root, 640, &launched("s", "/home/box/desktops/b_ann/chromium"), SYSTEM, false);
        fake_process(&root, 641, 640, "chromium", 20001, CHROMIUM);
        fake_process(&root, 650, 1, "box-chromium", 20001, SH);
        fake_start(&root, 650, &looped, SYSTEM, true);
        fake_process(&root, 651, 650, "box-chrome-pipe", 20001, PIPE);
        fake_start(&root, 651, &launched("s", "chromium"), SYSTEM, false);
        // box-chromium's first argument, but it doesn't hold the script: it runs something else
        fake_process(&root, 660, 1, "box-chromium", 20001, SH);
        fake_start(&root, 660, &looped, SYSTEM, false);
        fake_process(&root, 661, 660, "chromium", 20001, CHROMIUM);
        // the loop in every other way, with an fd that's there and can't be read: no word is no loop
        fake_process(&root, 670, 1, "box-chromium", 20001, SH);
        fake_start(&root, 670, &looped, SYSTEM, true);
        std::os::unix::fs::symlink("/denied", root.join("proc/670/fd/12")).unwrap();
        fake_process(&root, 671, 670, "box-chrome-pipe", 20001, PIPE);
        fake_start(&root, 671, &launched("s", "chromium"), SYSTEM, false);
        fake_process(&root, 672, 671, "chromium", 20001, CHROMIUM);
        // the loop and the launcher in every way, each the parent of an allowed program that isn't
        // the browser: the panel, and Chromium's crash handler
        fake_process(&root, 680, 1, "box-chromium", 20001, SH);
        fake_start(&root, 680, &looped, SYSTEM, true);
        fake_process(&root, 681, 680, "xfce4-panel", 20001, "/usr/bin/xfce4-panel");
        fake_process(&root, 690, 1, "box-chrome-pipe", 20001, PIPE);
        fake_start(&root, 690, &launched("s", "chromium"), SYSTEM, false);
        fake_process(&root, 691, 690, "xfce4-panel", 20001, "/usr/bin/xfce4-panel");
        fake_process(&root, 695, 1, "box-chrome-pipe", 20001, PIPE);
        fake_start(&root, 695, &launched("s", "chromium"), SYSTEM, false);
        fake_process(&root, 696, 695, "chrome_crashpad", 20001, "/usr/lib/chromium/chrome_crashpad_handler");
        // a command's, as the Bot's agent user
        fake_process(&root, 700, 1, "python3", 20000, "/usr/bin/python3");
        std::fs::create_dir_all(root.join("proc/self")).unwrap();
        let out = std::process::Command::new("python3").args(["-I", "-S", "-c", &format!("{FAKE_PROC}{CLEAR}"), "b_ann"]).arg(&root).output().expect("python3 runs the scan on this Mac");
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), r#"["box-chrome-pipe", "box-chromium", "python3", "sh"]"#);
        let ended = |pid: u32| root.join(format!("ended-{pid}")).exists();
        assert!(root.join("exited-590").exists() && !ended(590), "the one that exited was ended");
        for pid in [500, 501, 502, 503, 510, 511, 512, 611, 612, 622, 641, 661, 671, 672, 681, 691, 696, 700] {
            assert!(!ended(pid) && root.join(format!("proc/{pid}")).exists(), "{pid} was ended");
        }
        for pid in [600, 610, 620, 621, 630, 631, 640, 650, 651, 660, 670, 680, 690, 695] {
            assert!(ended(pid), "{pid} was left");
        }
    }

    #[tokio::test]
    async fn clearing_is_for_one_bots_desktop() {
        let dir = tmp("clear");
        let p = faked(&dir, &inspected("running", "running", "T1"), "echo '[\"python3\"]'", "true");
        let screen = Screen { display: ":2".into(), viewer_port: 6902, folder: "agents/amy".into(), profile: None };
        assert_eq!(p.clear_strays(&machine(&dir), &screen).await.unwrap(), ["python3"]);
        assert!(calls(&dir).trim_end().ends_with(" amy"), "{}", calls(&dir));
        let shared = Screen { folder: "shared".into(), ..screen };
        assert!(p.clear_strays(&machine(&dir), &shared).await.is_err(), "nothing to go on is no clearing");
    }

    #[tokio::test]
    async fn a_lost_record_is_found_by_its_label_and_nothing_is_removed() {
        let dir = tmp("recover");
        let body = "case \"$1\" in ps) echo c2-x-wall; echo c2-x;; inspect) echo '[]'; exit 1;; esac";
        let p = DockerProvider::new("img:1", "c2").with_program(fake_docker(&dir, body));
        let m = p.recover(&Spec { id: "cmp_1".into(), dir: dir.clone(), network: Default::default() }, None).await.unwrap().unwrap();
        assert_eq!(m.reach, Reach::Container { name: "c2-x".into() });
        assert!(!calls(&dir).contains("rm "), "{}", calls(&dir));
        let none = tmp("recover-none");
        let p = DockerProvider::new("img:1", "c2").with_program(fake_docker(&none, "true"));
        assert_eq!(p.recover(&Spec { id: "cmp_1".into(), dir: none.clone(), network: Default::default() }, None).await.unwrap(), None);
    }

    #[tokio::test]
    async fn a_new_computers_volumes_carry_its_label_and_old_ones_are_used_as_they_are() {
        let dir = tmp("labelled");
        let p = DockerProvider::new("img:1", "c2").with_program(fake_docker(&dir, "case \"$1\" in image) echo 1;; esac"));
        let m = p.create(&Spec { id: "cmp_1".into(), dir: dir.clone(), network: Default::default() }).await.unwrap();
        let Reach::Container { name } = &m.reach else { panic!("{:?}", m.reach) };
        let seen = calls(&dir);
        for volume in [format!("{name}-workspace"), format!("{name}-home")] {
            let made = format!("volume create --label app.buwiz.computer=1 --label app.buwiz.computer.id=cmp_1 {volume}\n");
            assert!(seen.contains(&made), "{seen}");
            assert!(seen.find(&made).unwrap() < seen.find(&format!("create --name {name} ")).unwrap(), "made before the box mounts it: {seen}");
        }
        // one from before labels: a Docker that won't make it again still gives the box its files
        let old = tmp("unlabelled");
        let body = "case \"$1 $2\" in 'volume create') echo 'Error response from daemon: volume c2-x-home already exists' >&2; exit 1;; esac";
        let p = DockerProvider::new("img:1", "c2").with_program(fake_docker(&old, body));
        p.make_volumes("c2-x", "cmp_1").await.unwrap();
        p.destroy(&machine(&old)).await.unwrap();
        assert!(calls(&old).contains("volume rm c2-x-workspace") && calls(&old).contains("volume rm c2-x-home"), "{}", calls(&old));
        let down = tmp("volume-down");
        let p = DockerProvider::new("img:1", "c2").with_program(fake_docker(&down, "echo 'Cannot connect to the Docker daemon at unix:///x. Is the docker daemon running?' >&2; exit 1"));
        assert_eq!(p.make_volumes("c2-x", "cmp_1").await.unwrap_err(), Problem::ProviderDown("Docker".into()));
    }

    #[tokio::test]
    async fn leftovers_are_volumes_whose_box_is_gone_by_label_or_by_name() {
        let dir = tmp("leftovers");
        let volumes = [
            // labelled, its box gone
            "c2-0a1b2c3d-workspace\tcmp_1",
            "c2-0a1b2c3d-home\tcmp_1",
            // from before labels, its box gone: the name says whose
            "c2-deadbeef-home\t",
            // its box is there
            "c2-11223344-workspace\tcmp_2",
            "c2-55667788-home\t",
            // a person's own, and names that only look like ours
            "postgres-data\t",
            "c2-notahex1-home\t",
            "c2-0a1b2c3d-cache\tcmp_1",
        ]
        .join("\\n");
        let body = format!("case \"$1 $2\" in 'volume ls') printf '{volumes}\\n';; 'ps -a') printf 'c2-11223344\\nc2-11223344-wall\\nc2-55667788\\nc2-0a1b2c3d-wall\\n';; esac");
        let p = DockerProvider::new("img:1", "c2").with_program(fake_docker(&dir, &body));
        let found = p.leftovers().await.unwrap().unwrap();
        let shown: Vec<(&str, Option<&str>, &str)> = found.iter().map(|l| (l.name.as_str(), l.id.as_deref(), l.owner.as_str())).collect();
        assert_eq!(
            shown,
            [("c2-0a1b2c3d-home", Some("cmp_1"), "c2-0a1b2c3d"), ("c2-0a1b2c3d-workspace", Some("cmp_1"), "c2-0a1b2c3d"), ("c2-deadbeef-home", None, "c2-deadbeef")],
            "a holder left alone doesn't keep a box's files from showing"
        );
        assert!(calls(&dir).contains("volume ls --format {{.Name}}\t{{.Label \"app.buwiz.computer.id\"}}"), "{}", calls(&dir));
        assert!(!calls(&dir).contains(" rm"), "only listed: {}", calls(&dir));
        let down = tmp("leftovers-down");
        let p = DockerProvider::new("img:1", "c2").with_program(fake_docker(&down, "echo 'Cannot connect to the Docker daemon at unix:///x. Is the docker daemon running?' >&2; exit 1"));
        assert!(p.leftovers().await.is_err(), "a Docker that can't say isn't one with nothing left");
    }

    #[tokio::test]
    async fn leftovers_ask_docker_for_volumes_and_containers_at_once() {
        let dir = tmp("leftovers-at-once");
        // each answers only once both were asked: one after the other, the first never would
        let body = format!("d='{}'; : > \"$d/in-$1\"; i=0; while [ ! -e \"$d/in-volume\" ] || [ ! -e \"$d/in-ps\" ]; do i=$((i+1)); [ $i -gt 100 ] && exit 1; sleep 0.1; done; case \"$1 $2\" in 'volume ls') printf 'c2-deadbeef-home\\t\\n';; esac", dir.display());
        let p = DockerProvider::new("img:1", "c2").with_program(fake_docker(&dir, &body));
        assert_eq!(p.leftovers().await.unwrap().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn files_with_no_container_left_are_found_by_name() {
        let dir = tmp("volumes");
        let p = DockerProvider::new("img:1", "c2").with_program(fake_docker(&dir, "case \"$1\" in inspect) echo '[]'; exit 1;; esac"));
        let spec = Spec { id: "cmp_1".into(), dir: dir.clone(), network: Default::default() };
        let m = p.recover(&spec, Some(&Reach::Container { name: "c2-x".into() })).await.unwrap().expect("its volume is there");
        assert_eq!(m.reach, Reach::Container { name: "c2-x".into() });
        assert!(calls(&dir).contains("volume inspect --format {{.Name}} c2-x-workspace"), "{}", calls(&dir));
        assert!(!calls(&dir).contains("rm "), "{}", calls(&dir));
        let none = tmp("no-volumes");
        let p = DockerProvider::new("img:1", "c2").with_program(fake_docker(&none, "case \"$1\" in volume) echo 'Error response from daemon: get x: no such volume' >&2; exit 1;; esac"));
        assert_eq!(p.recover(&Spec { id: "cmp_1".into(), dir: none.clone(), network: Default::default() }, Some(&Reach::Container { name: "c2-x".into() })).await.unwrap(), None);
    }

    #[tokio::test]
    async fn a_surviving_home_volume_counts_as_files() {
        let dir = tmp("home-only");
        let body = "case \"$1 $2\" in 'volume inspect') case \"$*\" in *-workspace) echo 'Error response from daemon: get c2-x-workspace: no such volume' >&2; exit 1;; esac;; esac";
        let p = DockerProvider::new("img:1", "c2").with_program(fake_docker(&dir, body));
        let found = p.recover(&Spec { id: "cmp_1".into(), dir: dir.clone(), network: Default::default() }, Some(&Reach::Container { name: "c2-x".into() })).await.unwrap();
        assert!(found.is_some(), "its home would be swept as nothing left");
    }

    #[tokio::test]
    async fn a_docker_that_cant_say_doesnt_read_as_no_files() {
        let dir = tmp("volumes-down");
        let p = DockerProvider::new("img:1", "c2").with_program(fake_docker(&dir, "case \"$1\" in volume) echo 'Cannot connect to the Docker daemon' >&2; exit 1;; esac"));
        let spec = Spec { id: "cmp_1".into(), dir: dir.clone(), network: Default::default() };
        assert_eq!(p.recover(&spec, Some(&Reach::Container { name: "c2-x".into() })).await.unwrap_err(), Problem::ProviderDown("Docker".into()));
    }

    #[tokio::test]
    async fn a_persons_delete_with_the_record_lost_still_finds_the_files() {
        let dir = tmp("sweep-name");
        let p = DockerProvider::new("img:1", "c2").with_program(fake_docker(&dir, "true"));
        p.sweep("cmp_1", Some(&Reach::Container { name: "c2-x".into() })).await.unwrap();
        assert!(calls(&dir).contains("volume rm c2-x-workspace") && calls(&dir).contains("volume rm c2-x-home"), "{}", calls(&dir));
    }

    /// An `ifconfig` that prints the file `addresses` and counts its runs.
    fn fake_ifconfig(dir: &Path) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join("ifconfig");
        std::fs::write(&path, format!("#!/bin/sh\necho x >> \"{0}/ifconfig-runs\"\ncat \"{0}/addresses\"\n", dir.display())).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(dir.join("addresses"), "\tinet 203.0.113.9 netmask 0xffffff00\n").unwrap();
        path
    }

    fn ifconfig_runs(dir: &Path) -> usize {
        std::fs::read_to_string(dir.join("ifconfig-runs")).unwrap_or_default().lines().count()
    }

    #[tokio::test]
    async fn the_wall_follows_the_macs_addresses_without_reading_them_every_request() {
        let dir = tmp("addresses");
        let program = fake_ifconfig(&dir);
        let p = faked(&dir, &inspected("running", "running", "T1"), "true", "true").with_ifconfig(program.clone());
        let m = machine(&dir);
        p.guard(&m).await.unwrap();
        for _ in 0..5 {
            p.guard(&m).await.unwrap();
        }
        assert_eq!(ifconfig_runs(&dir), 1, "ifconfig ran per request");
        assert_eq!(calls(&dir).matches(WALL_CALL).count(), 1);
        // a new network: a wake reads it, and the wall goes up for it
        std::fs::write(dir.join("addresses"), "\tinet 203.0.113.77 netmask 0xffffff00\n").unwrap();
        p.wake(&m).await.unwrap();
        assert_eq!(calls(&dir).matches(WALL_CALL).count(), 2, "{}", calls(&dir));
        assert!(calls(&dir).contains("203.0.113.77"), "{}", calls(&dir));
        p.wake(&m).await.unwrap();
        assert_eq!(calls(&dir).matches(WALL_CALL).count(), 2, "nothing changed");
    }

    #[tokio::test]
    async fn a_guard_rereads_the_addresses_once_their_life_is_up() {
        let dir = tmp("address-timer");
        let program = fake_ifconfig(&dir);
        let p = faked(&dir, &inspected("running", "running", "T1"), "true", "true").with_ifconfig(program).with_address_life(Duration::ZERO);
        let m = machine(&dir);
        p.guard(&m).await.unwrap();
        std::fs::write(dir.join("addresses"), "\tinet 203.0.113.77 netmask 0xffffff00\n").unwrap();
        p.guard(&m).await.unwrap();
        assert_eq!(calls(&dir).matches(WALL_CALL).count(), 2, "the Mac moved and the wall didn't: {}", calls(&dir));
    }

    #[tokio::test]
    async fn a_running_box_rides_out_a_bad_read_but_not_three_in_a_row() {
        let dir = tmp("bad-reads");
        let program = fake_ifconfig(&dir);
        let p = faked(&dir, &inspected("running", "running", "T1"), "true", "true").with_ifconfig(program.clone()).with_address_life(Duration::ZERO);
        let m = machine(&dir);
        p.guard(&m).await.unwrap();
        let broken = || std::fs::write(&program, "#!/bin/sh\nexit 1\n").unwrap();
        broken();
        p.guard(&m).await.expect("one failed read killed a running box");
        p.guard(&m).await.expect("the wall it has stays up meanwhile");
        assert!(!calls(&dir).contains("stop -t 0"), "{}", calls(&dir));
        assert_eq!(p.guard(&m).await.unwrap_err().code(), "unwalled");
        assert!(calls(&dir).contains("stop -t 0 c2-x"), "the third in a row halts it: {}", calls(&dir));
    }

    #[tokio::test]
    async fn a_good_read_clears_the_count_and_a_wake_never_rides_it_out() {
        let dir = tmp("good-read");
        let program = fake_ifconfig(&dir);
        let p = faked(&dir, &inspected("running", "running", "T1"), "true", "true").with_ifconfig(program.clone()).with_address_life(Duration::ZERO);
        let m = machine(&dir);
        let good = std::fs::read(&program).unwrap();
        p.guard(&m).await.unwrap();
        for _ in 0..3 {
            std::fs::write(&program, "#!/bin/sh\nexit 1\n").unwrap();
            p.guard(&m).await.unwrap();
            std::fs::write(&program, &good).unwrap();
            p.guard(&m).await.unwrap();
        }
        std::fs::write(&program, "#!/bin/sh\nexit 1\n").unwrap();
        assert_eq!(p.wake(&m).await.unwrap_err().code(), "unwalled", "a wake fails closed on one bad read");
    }

    #[tokio::test]
    async fn a_rebuild_gives_the_box_time() {
        let dir = tmp("graceful");
        let running = r#"[{"Name":"/c2-x","State":{"Status":"running","StartedAt":"U"}}]"#;
        let p = losing(&dir, running);
        let _ = p.wake(&machine(&dir)).await;
        assert!(calls(&dir).contains("stop -t 5 c2-x") && !calls(&dir).contains("stop -t 0"), "{}", calls(&dir));
    }

    #[tokio::test]
    async fn recover_stops_nothing_itself() {
        let dir = tmp("recover-stop");
        let body = format!("case \"$1\" in ps) echo c2-x;; inspect) cat <<'J'\n{}\nJ\n;; esac", inspected("running", "running", "T1"));
        let p = DockerProvider::new("img:1", "c2").with_program(fake_docker(&dir, &body));
        p.recover(&Spec { id: "cmp_1".into(), dir: dir.clone(), network: Default::default() }, None).await.unwrap();
        assert!(!calls(&dir).contains("stop"), "the seats' lock is held here: {}", calls(&dir));
    }

    #[tokio::test]
    async fn removing_goes_on_past_a_volume_that_is_already_gone() {
        let dir = tmp("remove");
        let body = "case \"$1 $2 $3\" in 'volume rm c2-x-workspace') echo 'Error response from daemon: get c2-x-workspace: no such volume' >&2; exit 1;; esac";
        let p = DockerProvider::new("img:1", "c2").with_program(fake_docker(&dir, body));
        p.destroy(&machine(&dir)).await.expect("one gone volume would never let a retire finish");
        assert!(calls(&dir).contains("volume rm c2-x-home"), "{}", calls(&dir));
    }

    /// Each line is what Docker 29 prints, copied from a run.
    #[test]
    fn dockers_real_words_are_read_right() {
        for line in [
            "Error response from daemon: get c2-none-x: no such volume",
            "Error response from daemon: No such container: c2-none-x",
            "Error response from daemon: No such container: c2-none-x\nfailed to start containers: c2-none-x",
        ] {
            assert_eq!(classify(Some(1), line, "i"), Problem::Gone, "{line}");
        }
        for line in ["Error response from daemon: No such image: c2-none:1", "Unable to find image 'c2-none:1' locally\nError response from daemon: pull access denied for c2-none, repository does not exist or may require 'docker login'"] {
            assert_eq!(classify(Some(1), line, "i"), Problem::ImageMissing("i".into()), "{line}");
        }
        let down = "failed to connect to the docker API at unix:///tmp/x.sock; check if the path is correct and if the daemon is running: dial unix /tmp/x.sock: connect: no such file or directory";
        assert_eq!(classify(Some(1), down, "i"), Problem::ProviderDown("Docker".into()));
        let no_python = "OCI runtime exec failed: exec failed: unable to start container process: exec: \"python3\": executable file not found in $PATH";
        assert!(matches!(classify(Some(127), no_python, "i"), Problem::Failed(_)), "a missing python3 isn't a gone computer");
    }

    #[tokio::test]
    async fn a_box_older_than_its_network_is_stale() {
        let dir = tmp("stale");
        let state = |status: &str| inspected(status, "running", "2026-11-06T10:00:05.5Z").replace(r#""StartedAt":"U""#, r#""StartedAt":"2026-11-06T10:00:01.123456789Z""#);
        let p = faked(&dir, &state("running"), "true", "exit 1");
        assert_eq!(p.guard(&machine(&dir)).await.unwrap_err().code(), "unwalled");
        assert!(calls(&dir).contains("stop -t 0 c2-x") && !calls(&dir).contains("volume"), "{}", calls(&dir));
        // a wake gives it a fresh start in the network it has now
        let dir = tmp("stale-wake");
        let p = faked(&dir, &state("paused"), "true", "exit 1");
        let _ = p.wake(&machine(&dir)).await;
        assert!(calls(&dir).contains("stop -t 5 c2-x") && calls(&dir).contains("start c2-x\n"), "{}", calls(&dir));
    }

    #[test]
    fn dockers_times_compare_with_their_zeros_trimmed() {
        assert!(older("2026-11-06T10:00:01.5Z", "2026-11-06T10:00:01.51Z"));
        assert!(!older("2026-11-06T10:00:01.50Z", "2026-11-06T10:00:01.5Z"));
        assert!(older("2026-11-06T10:00:01Z", "2026-11-06T10:00:01.000000001Z"));
    }

    #[test]
    fn the_exemption_for_published_ports_goes_to_the_bridge_gateway_alone() {
        for port_rule in ["rule(100, 1, dst=bridge[0], plen=32, ports=(1337, 1337))", "rule(101, 1, dst=bridge[0], plen=32, ports=(1340, 1340))", "rule(102, 1, dst=bridge[0], plen=32, ports=(6901, 6905))"] {
            assert!(WALL.contains(port_rule), "{port_rule}");
        }
        // the tunnel's, for a computer made with it alone: the gateway, and that one source port
        assert!(WALL.contains("if tunnel:\n    rule(103, 1, dst=bridge[0], plen=32, ports=(8790, 8790))\n"));
        assert_eq!(WALL.matches("rule(10").count(), 4, "no other exemption");
        assert!(WALL.contains("tunnel = sys.argv[1:2] == ['--tunnel']"));
    }

    #[tokio::test]
    async fn the_wall_lets_the_tunnel_through_only_for_a_computer_made_with_it() {
        let dir = tmp("wall-tunnel");
        let box_wont_start = "case \"$2\" in *-wall) ;; *) exit 1;; esac";
        let p = faked(&dir, &inspected("exited", "exited", "T1"), "true", box_wont_start);
        let _ = p.wake(&machine(&dir)).await;
        assert!(calls(&dir).contains(WALL_CALL) && !calls(&dir).contains("    pass\n --tunnel"), "{}", calls(&dir));
        let dir = tmp("wall-tunnel-on");
        let p = faked(&dir, &inspected("exited", "exited", "T1"), "true", box_wont_start);
        let m = Machine { egress: Some(Egress { tunnel: true, bearer: Some(Secret::new("b-0123")) }), ..machine(&dir) };
        let _ = p.wake(&m).await;
        assert!(calls(&dir).contains("    pass\n --tunnel"), "the flag goes first after the script, before the Mac's addresses: {}", calls(&dir));
        assert!(!calls(&dir).contains("b-0123"), "the bearer is on no argument list: {}", calls(&dir));
    }

    #[tokio::test]
    async fn the_bearer_is_staged_for_a_start_and_taken_away_after() {
        let dir = tmp("stage-bearer");
        let start = dir.join("start");
        let m = Machine { egress: Some(Egress { tunnel: true, bearer: Some(Secret::new("b-4567")) }), ..machine(&dir) };
        {
            let _staged = Staged::new(start.clone(), &m).unwrap();
            assert_eq!(std::fs::read_to_string(start.join("egress")).unwrap(), "b-4567");
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(start.join("egress")).unwrap().permissions().mode() & 0o777, 0o600);
        }
        assert!(!start.join("egress").exists() && !start.join("token").exists());
        // a computer made without the tunnel gets no bearer file
        let _staged = Staged::new(start.clone(), &machine(&dir)).unwrap();
        assert!(!start.join("egress").exists());
    }

    #[test]
    fn the_rules_stay_staged_and_are_replaced_whole() {
        let dir = tmp("stage-rules");
        stage_rules(&dir, br#"{"v":1}"#).unwrap();
        assert_eq!(std::fs::read(dir.join("start/rules.json")).unwrap(), br#"{"v":1}"#);
        {
            let _staged = Staged::new(dir.join("start"), &machine(&dir)).unwrap();
        }
        assert!(dir.join("start/rules.json").exists(), "a start's end takes the secrets, not the rules");
        stage_rules(&dir, br#"{"v":2}"#).unwrap();
        assert_eq!(std::fs::read(dir.join("start/rules.json")).unwrap(), br#"{"v":2}"#);
        assert!(!dir.join("start/rules.json.new").exists());
        unstage_rules(&dir);
        assert!(!dir.join("start/rules.json").exists());
    }

    #[tokio::test]
    async fn nothing_of_the_box_runs_before_the_wall_is_up() {
        let dir = tmp("order");
        let p = faked(&dir, &inspected("exited", "exited", "T1"), "true", "case \"$2\" in *-wall) ;; *) exit 1;; esac");
        let _ = p.wake(&machine(&dir)).await;
        assert!(at(&dir, "start c2-x-wall") < at(&dir, WALL_CALL), "{}", calls(&dir));
        assert!(at(&dir, WALL_CALL) < at(&dir, "start c2-x\n"), "the box started before its wall: {}", calls(&dir));
        // the usual deep wake: its network never went down, and this broker hasn't walled it
        let dir = tmp("order-up");
        let p = faked(&dir, &inspected("exited", "running", "T1"), "true", "exit 1");
        let _ = p.wake(&machine(&dir)).await;
        assert!(at(&dir, WALL_CALL) < at(&dir, "start c2-x\n"), "the box started before its wall: {}", calls(&dir));
    }

    #[tokio::test]
    async fn a_port_lost_to_a_race_is_tried_again() {
        let dir = tmp("port-race");
        let once = format!("case \"$2\" in *-wall) [ -e \"{0}/tried\" ] || {{ touch \"{0}/tried\"; echo 'Error response from daemon: ports are not available: exposing port TCP 127.0.0.1:47001' >&2; exit 1; }};; *) exit 1;; esac", dir.display());
        let p = faked(&dir, &inspected("exited", "exited", "T1"), "true", &once);
        let _ = p.wake(&machine(&dir)).await;
        assert_eq!(calls(&dir).matches("start c2-x-wall").count(), 2, "{}", calls(&dir));
        assert!(calls(&dir).contains(WALL_CALL), "{}", calls(&dir));
    }

    #[tokio::test]
    async fn a_wall_that_fails_means_the_box_never_starts() {
        let dir = tmp("no-wall");
        let p = faked(&dir, &inspected("exited", "running", "T1"), "echo no >&2; exit 1", "true");
        assert_eq!(p.wake(&machine(&dir)).await.unwrap_err().code(), "unwalled");
        assert!(!calls(&dir).contains("start c2-x\n"), "{}", calls(&dir));
    }

    #[tokio::test]
    async fn addresses_that_cant_be_read_stop_the_wall() {
        let dir = tmp("no-ifconfig");
        for bad in ["/usr/bin/false", "/nonexistent/ifconfig"] {
            let p = faked(&dir, &inspected("exited", "running", "T1"), "true", "true").with_ifconfig(PathBuf::from(bad));
            assert_eq!(p.wake(&machine(&dir)).await.unwrap_err().code(), "unwalled", "{bad}");
            assert!(!calls(&dir).contains(WALL_CALL) && !calls(&dir).contains("start c2-x\n"), "{}", calls(&dir));
        }
    }

    #[tokio::test]
    async fn the_secrets_are_gone_after_a_start_that_failed() {
        let dir = tmp("start");
        let start = "ls \"$(dirname \"$0\")/start\" >> \"$(dirname \"$0\")/seen\"; echo boom >&2; exit 1";
        let p = faked(&dir, &inspected("exited", "running", "T1"), "true", start);
        assert_eq!(p.wake(&machine(&dir)).await.unwrap_err(), Problem::Failed("boom".into()));
        assert_eq!(std::fs::read_to_string(dir.join("seen")).unwrap(), "token\nviewer\n", "they were there for the start");
        assert_eq!(std::fs::read_dir(dir.join("start")).unwrap().count(), 0, "and aren't now");
    }

    #[tokio::test]
    async fn the_secrets_are_gone_after_a_start_that_was_abandoned() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tmp("cancel");
        let p = faked(&dir, &inspected("exited", "running", "T1"), "true", "sleep 30");
        let m = machine(&dir);
        let mut wake = Box::pin(p.wake(&m));
        // until the start is under way, with the secrets on disk, then let it go
        for _ in 0..500 {
            if tokio::time::timeout(Duration::from_millis(10), &mut wake).await.is_ok() {
                panic!("it can't finish: start sleeps");
            }
            if dir.join("start/viewer").exists() && calls(&dir).contains("start c2-x\n") {
                break;
            }
        }
        assert!(dir.join("start/viewer").exists());
        drop(wake);
        assert_eq!(std::fs::read_dir(dir.join("start")).unwrap().count(), 0, "a dropped start left the secrets on disk");
        assert_eq!(std::fs::metadata(dir.join("start")).unwrap().permissions().mode() & 0o777, 0o700);
    }

    #[test]
    fn the_secrets_folder_is_private_whatever_the_umask_made_it() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tmp("private").join("start");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        private_dir(&dir).unwrap();
        assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
    }

    #[tokio::test]
    async fn a_box_this_broker_didnt_start_is_walled_on_first_use_and_once() {
        let dir = tmp("first-use");
        let p = faked(&dir, &inspected("running", "running", "T1"), "true", "true");
        let m = machine(&dir);
        p.guard(&m).await.unwrap();
        p.guard(&m).await.unwrap();
        assert_eq!(calls(&dir).matches(WALL_CALL).count(), 1, "{}", calls(&dir));
        // its network came up again: a new one, so a new wall
        std::fs::write(dir.join("state"), inspected("running", "running", "T2")).unwrap();
        p.guard(&m).await.unwrap();
        assert_eq!(calls(&dir).matches(WALL_CALL).count(), 2);
    }

    #[tokio::test]
    async fn a_guard_that_cant_wall_stops_the_box() {
        let dir = tmp("guard-fail");
        let p = faked(&dir, &inspected("running", "running", "T1"), "echo 'OCI runtime exec failed: exec: \"python3\": executable file not found in $PATH' >&2; exit 127", "true");
        let e = p.guard(&machine(&dir)).await.unwrap_err();
        assert_eq!(e.code(), "unwalled", "a missing python3 isn't a gone computer");
        assert!(calls(&dir).contains("stop -t 0 c2-x"), "{}", calls(&dir));
    }

    #[tokio::test]
    async fn a_box_whose_network_went_is_stopped() {
        let dir = tmp("net-gone");
        let p = faked(&dir, &inspected("running", "exited", "T1"), "true", "true");
        assert_eq!(p.guard(&machine(&dir)).await.unwrap_err().code(), "unwalled");
        assert!(calls(&dir).contains("stop -t 0 c2-x"), "{}", calls(&dir));
    }

    #[tokio::test]
    async fn every_wake_that_cant_wall_stops_the_box() {
        for status in ["running", "paused"] {
            let dir = tmp(status);
            let p = faked(&dir, &inspected(status, "running", "T1"), "echo no >&2; exit 1", "true");
            assert_eq!(p.wake(&machine(&dir)).await.unwrap_err().code(), "unwalled", "{status}");
            assert!(calls(&dir).contains("stop -t 0 c2-x"), "{status}: {}", calls(&dir));
        }
    }

    #[tokio::test]
    async fn a_paused_computer_wakes_without_a_start() {
        let dir = tmp("pause");
        let p = faked(&dir, &inspected("paused", "running", "T1"), "true", "true");
        assert_eq!(p.wake(&machine(&dir)).await.unwrap(), Woke::Resumed);
        assert!(calls(&dir).contains("unpause c2-x") && !calls(&dir).contains("start c2-x"), "{}", calls(&dir));
        assert!(at(&dir, WALL_CALL) < at(&dir, "unpause c2-x"), "the wall comes first here too: {}", calls(&dir));
    }

    #[tokio::test]
    async fn a_failed_second_create_leaves_no_holder() {
        let dir = tmp("create");
        let body = "case \"$1 $2 $3\" in version*|network*) ;; image*) echo 1;; 'create --name c2-'*-wall) ;; create*) echo nope >&2; exit 1;; esac";
        let p = DockerProvider::new("img:1", "c2").with_program(fake_docker(&dir, body));
        assert!(p.create(&Spec { id: "cmp_1".into(), dir: dir.clone(), network: Default::default() }).await.is_err());
        assert!(calls(&dir).contains("-wall") && calls(&dir).lines().any(|l| l.starts_with("rm -f -v") && l.ends_with("-wall")), "{}", calls(&dir));
    }

    #[tokio::test]
    async fn a_planted_link_gets_no_token_written_through_it() {
        let dir = tmp("link");
        let start = dir.join("start");
        std::fs::create_dir_all(&start).unwrap();
        let victim = dir.join("victim");
        std::fs::write(&victim, "keep me").unwrap();
        std::os::unix::fs::symlink(&victim, start.join("token")).unwrap();
        stage_secrets(&start, &machine(&dir)).unwrap();
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep me");
        assert!(!std::fs::symlink_metadata(start.join("token")).unwrap().file_type().is_symlink());
    }

    #[tokio::test]
    async fn one_look_serves_a_request_and_the_next_request_looks_again() {
        let dir = tmp("ports");
        let p = faked(&dir, &inspected("running", "running", "T1"), "true", "true");
        let screen = Screen { display: ":2".into(), viewer_port: 6902, folder: "agents/a".into(), profile: None };
        p.guard(&machine(&dir)).await.unwrap();
        let inspects = || calls(&dir).matches("inspect").count();
        let before = inspects();
        let (_, m) = p.look(&machine(&dir)).await.unwrap();
        p.guard(&m).await.unwrap();
        p.guest(&m).await.unwrap();
        p.guest(&m).await.unwrap();
        assert!(p.viewer(&m, &screen).await.unwrap().is_some());
        assert_eq!(inspects(), before + 1, "a request reads the container once");
        p.look(&machine(&dir)).await.unwrap();
        assert_eq!(inspects(), before + 2, "and each request reads it afresh: a cached port could be another computer's");
    }

    #[test]
    fn dockers_path_is_worked_out_once() {
        assert!(std::ptr::eq(docker_path(), docker_path()));
        assert!(std::env::split_paths(docker_path()).any(|d| d == Path::new("/usr/local/bin")));
    }

    #[test]
    fn what_was_seen_is_never_saved() {
        let dir = PathBuf::from("/tmp/x");
        let m = Machine { seen: Some(Seen::default()), ..machine(&dir) };
        let back: Machine = serde_json::from_str(&serde_json::to_string(&m).unwrap()).unwrap();
        assert_eq!(back.seen, None);
    }

    #[test]
    fn this_macs_addresses_are_read_from_ifconfig() {
        let out = "lo0: flags\n\tinet 127.0.0.1 netmask 0xff000000\nen0: flags\n\tinet6 fe80::1%en0\n\tinet 203.0.113.9 netmask 0xffffff00 broadcast 203.0.113.255\n";
        assert_eq!(parse_inets(out), ["203.0.113.9"]);
    }

    #[test]
    fn a_daemon_that_is_down_isnt_a_gone_computer() {
        assert_eq!(classify(Some(1), "Cannot connect to the Docker daemon", "i"), Problem::ProviderDown("Docker".into()));
        assert_eq!(classify(Some(1), "Error: No such container: x", "i"), Problem::Gone);
        assert_ne!(classify(Some(127), "exec: \"python3\": executable file not found in $PATH", "i"), Problem::Gone);
        assert_eq!(classify(Some(1), "Unable to find image 'i' locally", "i"), Problem::ImageMissing("i".into()));
    }
}
