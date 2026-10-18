//! **Confined activations** (post-360 hardening row M): an `[[activation]]`'s `command` and `probe`
//! run under the OS confinement profile its `[activation.confine]` table names, or not at all.
//!
//! An activation without the table runs **unconfined** — the operator's command with the stem's own
//! privileges, the stated default. With it, the stem applies the profile through a backend:
//!
//! - **Seatbelt** (macOS): `/usr/bin/sandbox-exec` with a generated profile — `deny default`; read the
//!   system set ([`SEATBELT_SYSTEM_READ`]), the placement directory and `read`; read and write `write`;
//!   open a TCP connection only to `connect` (`localhost:<port>` or `*:<port>`; with a `*` entry, the
//!   resolver's socket, so a name resolves). Paths are passed as `-D` parameters, never spliced into
//!   the profile text.
//! - **Landlock** (Linux): a Landlock ruleset (filesystem rights at ABI 3 — truncation included — and,
//!   when `connect` names a port, TCP connect and bind at ABI 4) and a seccomp filter (no socket family
//!   but `AF_INET`/`AF_INET6` stream sockets over TCP, those only when `connect` names a port; no
//!   `io_uring`, which would open sockets past the filter), both applied to a **dedicated thread** that
//!   then spawns the command: Landlock domains and seccomp filters are per-thread and inherited by every
//!   child, so the stem's own threads stay unrestricted and nothing runs between `fork` and `exec`.
//!
//! **No silent fallback.** A profile the platform cannot apply — the backend absent, the kernel without
//! Landlock (or below the ABI the profile needs), an architecture the filter is not built for, a
//! `localhost:<port>` under Landlock (which limits ports, not addresses), a declared path that does not
//! exist — refuses the activation by name and runs nothing ([`Refusal`]).
//!
//! **Fixed for the activation's life** (row M's OpenShell note): the profile is resolved once, when the
//! activation runs, and its re-probes reuse it — re-resolved, every check repeated, only when a declared
//! path no longer names what it named ([`Profile::is_current`]). A network allow-list that changes live is **not built**:
//! the unit file is read at start, and every limit it states holds until the stem restarts.
//!
//! **What neither backend limits**, stated: metadata (`stat`) of any path; on Linux below Landlock ABI 6,
//! signals to the stem's own user's processes (Unix sockets, which Landlock there does not limit either,
//! cannot be created: the filter refuses every socket but TCP; a socket inherited across `exec` is not
//! looked at); device `ioctl`s (Landlock ABI 5); on Linux, name resolution beyond `/etc/hosts`
//! under a `*:<port>` profile (DNS is UDP, which the filter refuses); CPU and memory (no resource
//! limits); the environment, which the command inherits from the stem. The command is still the
//! operator's, run as the stem's user: confinement narrows what it can reach, it does not make an
//! untrusted command safe to run.

use std::path::{Path, PathBuf};

use mycelium::{ConfineBackend, ConfineDecl};

/// What applies to one activation on this node: the answer the stem logs at start and the guarantee
/// report summarises.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Confinement {
    /// No `[activation.confine]`: run with the stem's privileges (the stated default).
    Unconfined,
    /// Run under the profile, through this backend.
    Confined { backend: ConfineBackend },
    /// The profile cannot be applied here; the activation is refused, never run unconfined.
    Refused { reason: String },
}

/// Why an activation was refused rather than run under its profile. Each names what is missing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The platform cannot apply the profile at all (recorded as `confinement_unavailable`).
    Unavailable(String),
    /// The platform could, and this run could not: a declared path missing, the kernel refusing a rule
    /// (recorded as `confinement_failed`).
    Failed(String),
}

impl Refusal {
    /// The reason the decision trace records (`prov.activation`).
    pub fn reason(&self) -> &'static str {
        match self {
            Refusal::Unavailable(_) => "confinement_unavailable",
            Refusal::Failed(_) => "confinement_failed",
        }
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::Unavailable(m) | Refusal::Failed(m) => f.write_str(m),
        }
    }
}

/// The paths a Seatbelt profile always lets a command read — **exactly these**: the system's executables
/// and libraries (not `/usr/local`, not `/opt/homebrew`, not `/Applications` — name those in `read`), the
/// `sh` shim's selection, and from `/private/etc` only what a resolver, a user lookup, the time zone and
/// TLS read (review finding 2, #636: `/private/etc` whole and `/usr/local` were readable).
pub const SEATBELT_SYSTEM_READ: &[&str] = &[
    "/usr/bin", "/usr/sbin", "/usr/lib", "/usr/libexec", "/usr/share", "/bin", "/sbin", "/System",
    "/private/var/select", "/private/var/db/timezone",
    "/private/etc/hosts", "/private/etc/resolv.conf", "/private/var/run/resolv.conf", "/private/etc/passwd",
    "/private/etc/group", "/private/etc/localtime", "/private/etc/ssl/cert.pem", "/private/etc/ssl/certs",
    "/private/etc/ssl/openssl.cnf",
];

/// The paths a Landlock profile always lets a command read and execute — **exactly these**: the system's
/// executables and libraries (not `/usr/local`), and from `/etc` only what the dynamic loader, a resolver,
/// a user lookup, the time zone, `alternatives` and TLS read — not `/etc` whole, where the node key's
/// documented home is (review finding 2, #636). Absent ones are skipped.
pub const LANDLOCK_SYSTEM_READ: &[&str] = &[
    "/usr/bin", "/usr/sbin", "/usr/lib", "/usr/lib32", "/usr/lib64", "/usr/libx32", "/usr/libexec", "/usr/share",
    "/bin", "/sbin", "/lib", "/lib32", "/lib64", "/libx32",
    "/etc/ld.so.cache", "/etc/ld.so.conf", "/etc/ld.so.conf.d", "/etc/hosts", "/etc/resolv.conf", "/etc/host.conf",
    "/etc/gai.conf", "/etc/nsswitch.conf", "/etc/passwd", "/etc/group", "/etc/localtime", "/etc/alternatives",
    "/etc/ssl/certs", "/etc/ssl/openssl.cnf", "/etc/ca-certificates",
];

/// The device files both backends let a command read, and `/dev/null`, which it may also write.
pub const SYSTEM_DEVICES: &[&str] = &["/dev/null", "/dev/zero", "/dev/random", "/dev/urandom"];

const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

/// The backend `auto` means here, if any.
pub fn native_backend() -> Option<ConfineBackend> {
    if cfg!(target_os = "macos") {
        Some(ConfineBackend::Seatbelt)
    } else if cfg!(target_os = "linux") {
        Some(ConfineBackend::Landlock)
    } else {
        None
    }
}

/// What applies to an activation with this `[activation.confine]` (or none) on this node, as built and
/// as the kernel answers now. Checks the backend, not the paths (they are judged when the activation
/// runs, against the placed file).
pub fn assess(confine: Option<&ConfineDecl>) -> Confinement {
    let Some(c) = confine else { return Confinement::Unconfined };
    match backend_for(c) {
        Ok(backend) => Confinement::Confined { backend },
        Err(reason) => Confinement::Refused { reason },
    }
}

/// The backend a profile resolves to, or why it cannot be applied here.
fn backend_for(c: &ConfineDecl) -> Result<ConfineBackend, String> {
    let backend = match c.backend {
        ConfineBackend::Auto => native_backend().ok_or_else(|| {
            format!("no confinement backend on {} (Seatbelt is macOS's, Landlock Linux's)", std::env::consts::OS)
        })?,
        b => b,
    };
    match backend {
        ConfineBackend::Seatbelt => {
            if !cfg!(target_os = "macos") {
                return Err(format!("backend seatbelt is macOS's, and this is {}", std::env::consts::OS));
            }
            if !Path::new(SANDBOX_EXEC).exists() {
                return Err(format!("backend seatbelt needs {SANDBOX_EXEC}, which is not on this host"));
            }
        }
        ConfineBackend::Landlock => {
            if !cfg!(target_os = "linux") {
                return Err(format!("backend landlock is Linux's, and this is {}", std::env::consts::OS));
            }
            if let Some(local) = c.connect.iter().find(|e| e.starts_with("localhost:")) {
                return Err(format!(
                    "backend landlock limits a TCP port, not an address, so it cannot hold connect {local:?} — write `*:<port>` to accept any address on that port"
                ));
            }
            linux::available(!c.connect.is_empty())?;
        }
        ConfineBackend::Auto => unreachable!("resolved above"),
    }
    Ok(backend)
}

/// A profile resolved against one placed file: every path expanded, present and resolved — resolved
/// **once**, when the activation is first prepared, and kept (review finding 5, #636) while it is current.
///
/// Each path is resolved by a **walk** (review of #641, round 2): the part of it outside every writable
/// root (the operator's — an ancestor symlink there is followed) is canonicalised, and every component
/// below a root is opened from its parent's descriptor with `O_NOFOLLOW`, so a symlink a confined command
/// planted — at the last component or any between — is refused, never followed, whenever it is planted.
/// The identity (device, inode) is read from what the walk opened. On Linux the final descriptor
/// (`O_PATH`) is kept and Landlock's rules are made from it; Seatbelt matches path strings, so the profile
/// names the path's on-disk spelling, accepted only when it names exactly what the walk opened.
#[derive(Debug, Clone)]
pub struct Profile {
    pub backend: ConfineBackend,
    /// Readable: the placement directory, then `read`.
    pub read:    Vec<PathBuf>,
    /// Readable and writable.
    pub write:   Vec<PathBuf>,
    /// TCP destinations: `(localhost only, port)`.
    pub connect: Vec<(bool, u16)>,
    /// `read` then `write`, held open from the walk (Linux; empty elsewhere).
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub(crate) pinned: Pinned,
    /// `read` then `write`: each path's (device, inode), read from what the walk opened.
    pub(crate) ids: Vec<(u64, u64)>,
    /// `read` then `write`: each path as declared, expanded but not resolved — what the currency check
    /// walks again, so a flipped ancestor symlink or a component replaced below a root is seen.
    pub(crate) sources: Vec<PathBuf>,
    /// The writable roots' identities (the placement directory, then each `write`): at or below one, every
    /// component is walked without following a symlink.
    pub(crate) roots: Vec<(u64, u64)>,
}

impl Profile {
    /// Whether every path still names what was resolved (review follow-up 1, #636): each declared path
    /// walked again — no symlink below a writable root, at any component — reaching the same device and
    /// inode under the same name. A path removed, recreated or replaced is not current, nor one whose
    /// operator-side ancestor symlink was flipped (`current` → `v2`). The caller re-resolves it rather
    /// than run with a stale pin.
    pub fn is_current(&self) -> bool {
        self.stale_path().is_none()
    }

    /// The first path that no longer names what was resolved, if any.
    fn stale_path(&self) -> Option<&Path> {
        let paths: Vec<&PathBuf> = self.read.iter().chain(&self.write).collect();
        if self.ids.len() != paths.len() || self.sources.len() != paths.len() {
            return Some(Path::new("(an incomplete profile)"));
        }
        for (i, (p, src)) in paths.iter().zip(&self.sources).enumerate() {
            let again = walk(src, &self.roots, &src.display().to_string(), i == 0);
            if !again.is_ok_and(|w| w.path == **p && w.id == self.ids[i]) {
                return Some(p);
            }
        }
        None
    }
}

/// Descriptors of a profile's paths, opened at its resolution.
#[derive(Debug, Clone, Default)]
pub(crate) struct Pinned(#[cfg_attr(not(target_os = "linux"), allow(dead_code))] pub(crate) Vec<std::sync::Arc<std::os::fd::OwnedFd>>);

/// One declared path, walked.
struct Walked {
    /// The path the profile names: the on-disk spelling of what the walk opened.
    path: PathBuf,
    /// Its (device, inode), from what the walk opened.
    id:   (u64, u64),
    /// The final descriptor (`O_PATH`, Linux only).
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    fd:   Option<std::os::fd::OwnedFd>,
}

fn refused_symlink(raw: &str, p: &Path, at: &Path) -> Refusal {
    Refusal::Failed(format!(
        "confine path {raw:?} ({}) passes through a symlink at {} — a declared path is never a symlink, nor reached \
         through one a confined command could have planted; name its target",
        p.display(),
        at.display()
    ))
}

/// The most symlinks one walk follows (the kernel's own bound is 32–40).
const MAX_FOLLOWS: usize = 40;

/// Resolve one declared path by a walk from `/` (review of #641, rounds 2 and 3). Each component is
/// looked at from its parent's descriptor without following it. A symlink is followed only while the walk
/// has not yet entered a **writable root** — a directory whose (device, inode) is one of `roots` (the
/// placement directory and each `write` path) — so the operator's own links outside every root are
/// followed (`/tmp` on macOS, `/opt/models/current`, a shortcut into a root), and from the moment the
/// walk is at or below a root nothing is: a symlink there, at any component and whenever it was planted,
/// is refused, and so is a `..`. Roots are recognised by identity, not by spelling, so a path that reaches
/// a root through any name is treated as inside it (finding H1), the placement directory included
/// (finding H2).
///
/// The identity comes from what the walk opened. The name the profile carries (which Seatbelt matches)
/// is the path's canonical, on-disk spelling (case and normalisation as stored — round 2, B), accepted
/// only if it has the walked depth and names the walked identity. `allow_final_link`: the declared path's
/// own last component may be the operator's symlink (the placement directory); for every other declared
/// path it is refused, as `[activation.confine]` has always said.
fn walk(src: &Path, roots: &[(u64, u64)], raw: &str, allow_final_link: bool) -> Result<Walked, Refusal> {
    use rustix::fs::{openat, readlinkat, statat, AtFlags, Mode, OFlags, CWD};
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStrExt;
    let missing = |e: &dyn std::fmt::Display| {
        Refusal::Failed(format!("confine path {raw:?} ({}): {e} — a declared path must exist when the activation runs", src.display()))
    };
    // A directory opened for lookup only; a kernel that does not know the search flag answers `EINVAL`, and
    // the open is retried read-only.
    let open_dir = |at: &dyn std::os::fd::AsFd, name: &Path, follow: bool| -> rustix::io::Result<std::os::fd::OwnedFd> {
        let base = OFlags::DIRECTORY | OFlags::CLOEXEC | if follow { OFlags::empty() } else { OFlags::NOFOLLOW };
        match openat(at.as_fd(), name, base | search_flag(), Mode::empty()) {
            Err(rustix::io::Errno::INVAL) => openat(at.as_fd(), name, base | OFlags::RDONLY, Mode::empty()),
            other => other,
        }
    };
    let abs = std::path::absolute(src).map_err(|e| missing(&e))?;
    // The components still to resolve, front first; `true` marks the declared path's own last component.
    let mut queue: std::collections::VecDeque<(OsString, bool)> = std::collections::VecDeque::new();
    let push_path = |queue: &mut std::collections::VecDeque<(OsString, bool)>, p: &Path, declared_last: bool| -> bool {
        let comps: Vec<std::path::Component> = p.components().collect();
        let n = comps.len();
        let mut absolute = false;
        for (i, c) in comps.into_iter().enumerate().rev() {
            match c {
                std::path::Component::RootDir => absolute = true,
                std::path::Component::CurDir | std::path::Component::Prefix(_) => {}
                std::path::Component::ParentDir => queue.push_front((OsString::from(".."), false)),
                std::path::Component::Normal(n_) => queue.push_front((n_.to_os_string(), declared_last && i + 1 == n)),
            }
        }
        absolute
    };
    push_path(&mut queue, &abs, true);
    // The walk's state: the directories from `/` (descriptor, name), and whether a root has been entered.
    let root_fd = open_dir(&CWD, Path::new("/"), true).map_err(|e| missing(&e))?;
    let id_of = |fd: &std::os::fd::OwnedFd| rustix::fs::fstat(fd).map(|st| dev_ino(&st));
    let mut inside = roots.contains(&id_of(&root_fd).map_err(|e| missing(&e))?);
    let mut stack: Vec<(std::os::fd::OwnedFd, OsString)> = Vec::new();
    let here = |stack: &Vec<(std::os::fd::OwnedFd, OsString)>| -> PathBuf {
        let mut p = PathBuf::from("/");
        for (_, n) in stack {
            p.push(n);
        }
        p
    };
    let mut follows = 0;
    at_stage("anchored");
    while let Some((name, declared_last)) = queue.pop_front() {
        let at = here(&stack).join(&name);
        if name == ".." {
            if inside {
                return Err(Refusal::Failed(format!(
                    "confine path {raw:?} ({}): `..` at or below a writable root is not resolved — name the path directly",
                    src.display()
                )));
            }
            stack.pop();
            continue;
        }
        let cur: &std::os::fd::OwnedFd = stack.last().map(|(fd, _)| fd).unwrap_or(&root_fd);
        let st = statat(cur, &name, AtFlags::SYMLINK_NOFOLLOW).map_err(|e| missing(&e))?;
        let last = queue.is_empty();
        if rustix::fs::FileType::from_raw_mode(st.st_mode).is_symlink() {
            // Followed only on the operator's side, and never as a declared path's own last component.
            if inside || (declared_last && !allow_final_link) {
                return Err(refused_symlink(raw, src, &at));
            }
            follows += 1;
            if follows > MAX_FOLLOWS {
                return Err(missing(&"too many levels of symbolic links"));
            }
            let target = readlinkat(cur, &name, Vec::new()).map_err(|e| missing(&e))?;
            let target = PathBuf::from(std::ffi::OsStr::from_bytes(target.as_bytes()));
            if push_path(&mut queue, &target, false) {
                stack.clear();
                inside = roots.contains(&id_of(&root_fd).map_err(|e| missing(&e))?);
            }
            continue;
        }
        if !last {
            let fd = open_dir(cur, Path::new(&name), false).map_err(|e| {
                if e == rustix::io::Errno::LOOP || e == rustix::io::Errno::NOTDIR { refused_symlink(raw, src, &at) } else { missing(&e) }
            })?;
            inside = inside || roots.contains(&id_of(&fd).map_err(|e| missing(&e))?);
            stack.push((fd, name));
            continue;
        }
        let (id, final_fd) = final_component(cur, &name).map_err(|e| {
            if e == rustix::io::Errno::LOOP { refused_symlink(raw, src, &at) } else { missing(&e) }
        })?;
        let Some(id) = id else { return Err(refused_symlink(raw, src, &at)) };
        at_stage("walked");
        // The on-disk spelling, accepted only when it names what was opened: the walked depth, the same
        // device and inode, not a symlink.
        let walked_to = at;
        let path = canonical(&abs).map_err(|e| missing(&e))?;
        let same = path.components().count() == walked_to.components().count()
            && statat(CWD, &path, AtFlags::SYMLINK_NOFOLLOW).is_ok_and(|m| {
                !rustix::fs::FileType::from_raw_mode(m.st_mode).is_symlink() && dev_ino(&m) == id
            });
        if !same {
            return Err(Refusal::Failed(format!(
                "confine path {raw:?} ({}) resolved to {}, which is not what was opened at {} — a declared path changed \
                 while it was resolved; refused",
                src.display(),
                path.display(),
                walked_to.display()
            )));
        }
        return Ok(Walked { path, id, fd: final_fd });
    }
    // The path was `/` itself, or ended in `..`: name it directly.
    Err(missing(&"a declared path must name a file or directory below `/`"))
}

/// The identity a root has now: its canonical path's (device, inode), followed — the starting set the
/// walks then confirm (each root is itself a declared path, walked against this set).
fn root_identity(p: &Path) -> Result<(u64, u64), Refusal> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(p)
        .map(|m| (m.dev(), m.ino()))
        .map_err(|e| Refusal::Failed(format!("confine path {}: {e} — a declared path must exist when the activation runs", p.display())))
}

/// A stat's (device, inode) — `u64` on Linux, narrower types on macOS.
#[allow(clippy::unnecessary_cast)]
fn dev_ino(st: &rustix::fs::Stat) -> (u64, u64) {
    (st.st_dev as u64, st.st_ino as u64)
}

/// The flag that opens a directory for lookup only: `O_PATH` on Linux; `O_SEARCH` (`O_EXEC`) on macOS, so a
/// directory the stem may search but not list is walked as `stat` would (a kernel without it answers
/// `EINVAL`, and the walk retries read-only).
fn search_flag() -> rustix::fs::OFlags {
    #[cfg(target_os = "linux")]
    {
        rustix::fs::OFlags::PATH
    }
    #[cfg(target_os = "macos")]
    {
        rustix::fs::OFlags::from_bits_retain(0x4000_0000) // O_EXEC; with O_DIRECTORY, O_SEARCH
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        rustix::fs::OFlags::RDONLY
    }
}

/// The last component: its identity, `None` if it is a symlink, and on Linux its `O_PATH | O_NOFOLLOW`
/// descriptor (which opens a FIFO, a socket or a write-only file without blocking or needing access).
/// Elsewhere it is not opened at all (review of #641 round 2, finding C): `fstatat` without following.
#[allow(clippy::type_complexity)]
fn final_component(dir: &std::os::fd::OwnedFd, name: &std::ffi::OsStr) -> rustix::io::Result<(Option<(u64, u64)>, Option<std::os::fd::OwnedFd>)> {
    use rustix::fs::FileType;
    #[cfg(target_os = "linux")]
    {
        use rustix::fs::{fstat, openat, Mode, OFlags};
        let fd = openat(dir, name, OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC, Mode::empty())?;
        let st = fstat(&fd)?;
        if FileType::from_raw_mode(st.st_mode).is_symlink() {
            return Ok((None, None));
        }
        Ok((Some(dev_ino(&st)), Some(fd)))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let st = rustix::fs::statat(dir, name, rustix::fs::AtFlags::SYMLINK_NOFOLLOW)?;
        if FileType::from_raw_mode(st.st_mode).is_symlink() {
            return Ok((None, None));
        }
        Ok((Some(dev_ino(&st)), None))
    }
}

/// Resolve `c` against the placed file `placed`: the backend (or [`Refusal::Unavailable`]), then each
/// path with `{dir}`/`{path}` expanded and walked ([`walk`]) — one that does not exist is
/// [`Refusal::Failed`], naming it, and so is one that is, or passes through, a symlink below the placement
/// directory or a write path (review finding 5, #636: a confined run may write there, so a link there
/// could be its own plant — name its target instead). Ancestors outside every writable root are the
/// operator's and may be symlinks (`/tmp` is one on macOS).
pub fn prepare(c: &ConfineDecl, placed: &Path) -> Result<Profile, Refusal> {
    let backend = backend_for(c).map_err(Refusal::Unavailable)?;
    let dir = placed.parent().unwrap_or_else(|| Path::new("/"));
    let expand = |raw: &str| -> PathBuf {
        PathBuf::from(if let Some(rest) = raw.strip_prefix("{dir}") {
            format!("{}{rest}", dir.display())
        } else if let Some(rest) = raw.strip_prefix("{path}") {
            format!("{}{rest}", placed.display())
        } else {
            raw.to_string()
        })
    };
    // The writable roots by identity (review of #641 round 3): however a path is spelt — an alias
    // directory, `/tmp` against `/private/tmp`, an operator shortcut into a root — the walk knows when it is
    // inside one. The set is confirmed below: each root is itself walked, and must reach the same identity.
    let root_paths: Vec<PathBuf> = std::iter::once(dir.to_path_buf()).chain(c.write.iter().map(|w| expand(w))).collect();
    let roots: Vec<(u64, u64)> = root_paths.iter().map(|r| root_identity(r)).collect::<Result<_, _>>()?;
    at_stage("checked");
    let connect = c
        .connect
        .iter()
        .map(|e| mycelium::parse_confine_connect(e).map(|(host, port)| (host == "localhost", port)))
        .collect::<Result<Vec<_>, _>>()
        .map_err(Refusal::Failed)?;
    let mut walked = vec![walk(dir, &roots, &dir.display().to_string(), true)?];
    for raw in c.read.iter().chain(&c.write) {
        walked.push(walk(&expand(raw), &roots, raw, false)?);
    }
    // Each root reached by its walk is the root the set named: the placement directory first, then each
    // `write` (the last `c.write.len()` walks).
    let walked_roots = std::iter::once(walked[0].id).chain(walked[walked.len() - c.write.len()..].iter().map(|w| w.id));
    if !walked_roots.eq(roots.iter().copied()) {
        return Err(Refusal::Failed(
            "a writable root changed while the profile resolved (its walk reached another file than its name) — refused".into(),
        ));
    }
    at_stage("resolved");
    let n_read = 1 + c.read.len();
    let sources: Vec<PathBuf> = std::iter::once(dir.to_path_buf()).chain(c.read.iter().chain(&c.write).map(|r| expand(r))).collect();
    let ids = walked.iter().map(|w| w.id).collect();
    let paths: Vec<PathBuf> = walked.iter().map(|w| w.path.clone()).collect();
    let pinned = if cfg!(target_os = "linux") {
        Pinned(walked.into_iter().filter_map(|w| w.fd.map(std::sync::Arc::new)).collect())
    } else {
        Pinned::default()
    };
    let (read, write) = (paths[..n_read].to_vec(), paths[n_read..].to_vec());
    let profile = Profile { backend, read, write, connect, pinned, ids, sources, roots };
    at_stage("pinned");
    at_stage("rechecked");
    // Walked again: a component toggled while the profile resolved (a link at one walk, a directory at the
    // next) is caught by whichever walk saw the link, or by the identities differing.
    if let Some(p) = profile.stale_path() {
        return Err(Refusal::Failed(format!(
            "confine path {} changed while it was resolved (a symlink, another file, or another target now) — refused",
            p.display()
        )));
    }
    Ok(profile)
}

// A test-only hook `prepare` and `walk` call between their stages (`checked`, `anchored`, `walked`,
// `resolved`, `pinned`, `rechecked`), so a test can plant the swap a confined command racing the resolution
// would (review of #641, findings 1 and A). Nothing outside the tests sets it.
#[cfg(test)]
pub(crate) type StageHook = Option<Box<dyn FnMut(&str)>>;

#[cfg(test)]
thread_local! {
    pub(crate) static PREPARE_STAGE_HOOK: std::cell::RefCell<StageHook> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
fn at_stage(stage: &str) {
    PREPARE_STAGE_HOOK.with(|h| {
        if let Some(f) = h.borrow_mut().as_mut() {
            f(stage);
        }
    });
}

#[cfg(not(test))]
#[inline(always)]
fn at_stage(_stage: &str) {}

/// The kernel matches resolved paths (`/tmp` is `/private/tmp` on macOS), so a profile names those.
fn canonical(p: &Path) -> std::io::Result<PathBuf> {
    p.canonicalize()
}

/// Run `argv` under `profile` within `timeout`, through [`crate::activation::run`]'s runner (stderr
/// drained, killed at the deadline). `Err` names the command and why, or the confinement that failed.
pub fn run_confined(argv: &[String], timeout: std::time::Duration, profile: &Profile) -> Result<(), String> {
    match profile.backend {
        ConfineBackend::Seatbelt => crate::activation::run(&seatbelt_argv(profile, argv), timeout),
        ConfineBackend::Landlock => linux::run(argv, timeout, profile),
        ConfineBackend::Auto => Err("an unresolved backend — refused, not run unconfined".into()),
    }
}

/// `sandbox-exec -D R0=… -D W0=… -p <profile> -- argv…`: the profile refers to its paths by parameter,
/// so no path is ever parsed as profile text.
pub fn seatbelt_argv(profile: &Profile, argv: &[String]) -> Vec<String> {
    let mut out = vec![SANDBOX_EXEC.to_string()];
    for (i, p) in profile.read.iter().enumerate() {
        out.extend(["-D".into(), format!("R{i}={}", p.display())]);
    }
    for (i, p) in profile.write.iter().enumerate() {
        out.extend(["-D".into(), format!("W{i}={}", p.display())]);
    }
    out.extend(["-p".into(), seatbelt_profile(profile), "--".into()]);
    out.extend(argv.iter().cloned());
    out
}

/// The generated Seatbelt profile (SBPL). Ports are numbers; paths are parameters.
pub fn seatbelt_profile(profile: &Profile) -> String {
    let mut s = String::from(
        "(version 1)\n(deny default)\n(allow process-exec process-fork)\n(allow signal (target same-sandbox))\n\
         (allow sysctl-read)\n(allow file-read-metadata)\n(allow file-read* (literal \"/\")",
    );
    for p in SEATBELT_SYSTEM_READ {
        s.push_str(&format!(" (subpath \"{p}\")"));
    }
    for p in SYSTEM_DEVICES {
        s.push_str(&format!(" (literal \"{p}\")"));
    }
    s.push_str(")\n(allow file-write* (literal \"/dev/null\"))\n");
    for i in 0..profile.read.len() {
        s.push_str(&format!("(allow file-read* (subpath (param \"R{i}\")))\n"));
    }
    for i in 0..profile.write.len() {
        s.push_str(&format!("(allow file-read* file-write* (subpath (param \"W{i}\")))\n"));
    }
    for (local, port) in &profile.connect {
        let host = if *local { "localhost" } else { "*" };
        s.push_str(&format!("(allow network-outbound (remote tcp \"{host}:{port}\"))\n"));
    }
    if profile.connect.iter().any(|(local, _)| !local) {
        s.push_str("(allow network-outbound (remote unix-socket (path-literal \"/private/var/run/mDNSResponder\")))\n");
    }
    s
}

#[cfg(target_os = "linux")]
mod linux {
    use super::{Profile, LANDLOCK_SYSTEM_READ, SYSTEM_DEVICES};
    use landlock::{
        Access, AccessFs, AccessNet, CompatLevel, Compatible, NetPort, PathBeneath, PathFd, Ruleset, RulesetAttr,
        RulesetCreatedAttr, RulesetStatus, ABI,
    };
    use seccompiler::{
        BpfProgram, SeccompAction, SeccompCmpArgLen, SeccompCmpOp, SeccompCondition, SeccompFilter, SeccompRule,
        TargetArch,
    };
    use std::collections::BTreeMap;
    use std::time::Duration;

    /// Filesystem rights at ABI 3: everything ABI 1 handles, cross-directory links (2) and truncation (3).
    const FS_ABI: ABI = ABI::V3;

    fn arch() -> Result<TargetArch, String> {
        match std::env::consts::ARCH {
            "x86_64" => Ok(TargetArch::x86_64),
            "aarch64" => Ok(TargetArch::aarch64),
            other => Err(format!("backend landlock's seccomp filter is built for x86_64 and aarch64, and this is {other}")),
        }
    }

    /// The ruleset a profile needs, handled at a hard requirement: an older kernel is an error here,
    /// never a quieter sandbox.
    fn ruleset(net: bool) -> Result<landlock::RulesetCreated, String> {
        let mut r = Ruleset::default()
            .set_compatibility(CompatLevel::HardRequirement)
            .handle_access(AccessFs::from_all(FS_ABI))
            .map_err(|e| format!("backend landlock needs Landlock ABI 3 (Linux 6.2) for its filesystem limits: {e}"))?;
        if net {
            r = r
                .handle_access(AccessNet::ConnectTcp | AccessNet::BindTcp)
                .map_err(|e| format!("backend landlock needs Landlock ABI 4 (Linux 6.7) for `connect`: {e}"))?;
        }
        r.create().map_err(|e| format!("backend landlock: this kernel does not provide Landlock ({e})"))
    }

    pub(super) fn available(net: bool) -> Result<(), String> {
        arch()?;
        ruleset(net).map(|_| ())
    }

    /// `socket(2)`'s refusals: every socket without `connect`; with it, every socket but an IP stream over
    /// TCP. Rules are OR'd; a rule's conditions AND'd.
    fn filter(tcp: bool) -> Result<BpfProgram, String> {
        let e = |x: seccompiler::BackendError| x.to_string();
        let cond = |arg: u8, op: SeccompCmpOp, v: u64| SeccompCondition::new(arg, SeccompCmpArgLen::Dword, op, v).map_err(e);
        let (inet, inet6) = (libc::AF_INET as u64, libc::AF_INET6 as u64);
        let mut socket_rules = Vec::new();
        if tcp {
            // Any family but the two IP families.
            socket_rules.push(SeccompRule::new(vec![cond(0, SeccompCmpOp::Ne, inet)?, cond(0, SeccompCmpOp::Ne, inet6)?]).map_err(e)?);
            for fam in [inet, inet6] {
                // A stream socket over a protocol other than TCP (SCTP, MPTCP): Landlock limits TCP only.
                socket_rules.push(SeccompRule::new(vec![
                    cond(0, SeccompCmpOp::Eq, fam)?,
                    cond(1, SeccompCmpOp::MaskedEq(0xf), libc::SOCK_STREAM as u64)?,
                    cond(2, SeccompCmpOp::Ne, 0)?,
                    cond(2, SeccompCmpOp::Ne, libc::IPPROTO_TCP as u64)?,
                ]).map_err(e)?);
                // An IP socket that is not a stream (UDP, raw, …): every other value of the type's low four
                // bits, the flags (`SOCK_NONBLOCK`, `SOCK_CLOEXEC`) masked off.
                for ty in (0u64..16).filter(|t| *t != libc::SOCK_STREAM as u64) {
                    socket_rules.push(SeccompRule::new(vec![cond(0, SeccompCmpOp::Eq, fam)?, cond(1, SeccompCmpOp::MaskedEq(0xf), ty)?]).map_err(e)?);
                }
            }
        }
        let mut rules: BTreeMap<i64, Vec<SeccompRule>> = BTreeMap::new();
        // No `connect`: every socket(2) refused, whatever its family. With one: the rules above.
        rules.insert(libc::SYS_socket, socket_rules);
        // io_uring opens and connects sockets without socket(2)/connect(2).
        rules.insert(libc::SYS_io_uring_setup, Vec::new());
        // Review finding 1 (#636): TCP Fast Open connects inside sendto/sendmsg/sendmmsg — Landlock's
        // connect hook never sees it — so a send carrying MSG_FASTOPEN is refused, and so are the socket
        // options that turn connect(2) or a listener into Fast Open. listen(2) auto-binds an unbound socket
        // past Landlock's bind hook: refused outright (a confined command serves nothing).
        let fastopen = libc::MSG_FASTOPEN as u64;
        let flagged = |arg: u8| -> Result<Vec<SeccompRule>, String> {
            Ok(vec![SeccompRule::new(vec![cond(arg, SeccompCmpOp::MaskedEq(fastopen), fastopen)?]).map_err(e)?])
        };
        rules.insert(libc::SYS_sendto, flagged(3)?);
        rules.insert(libc::SYS_sendmsg, flagged(2)?);
        rules.insert(libc::SYS_sendmmsg, flagged(3)?);
        rules.insert(libc::SYS_listen, Vec::new());
        rules.insert(libc::SYS_setsockopt, vec![
            SeccompRule::new(vec![cond(1, SeccompCmpOp::Eq, libc::IPPROTO_TCP as u64)?, cond(2, SeccompCmpOp::Eq, libc::TCP_FASTOPEN as u64)?]).map_err(e)?,
            SeccompRule::new(vec![cond(1, SeccompCmpOp::Eq, libc::IPPROTO_TCP as u64)?, cond(2, SeccompCmpOp::Eq, libc::TCP_FASTOPEN_CONNECT as u64)?]).map_err(e)?,
        ]);
        let f = SeccompFilter::new(rules, SeccompAction::Allow, SeccompAction::Errno(libc::EACCES as u32), arch()?).map_err(e)?;
        f.try_into().map_err(e)
    }

    fn restrict(profile: &Profile) -> Result<(), String> {
        let net = !profile.connect.is_empty();
        let mut created = ruleset(net)?;
        let read = AccessFs::from_read(FS_ABI);
        let all = AccessFs::from_all(FS_ABI);
        let rule = |p: &std::path::Path, access: landlock::BitFlags<AccessFs>| -> Result<Option<PathBeneath<PathFd>>, String> {
            let Ok(fd) = PathFd::new(p) else { return Ok(None) };
            // A file takes only the rights a file has; a directory every one asked for.
            let access = if p.is_dir() { access } else { access & AccessFs::from_file(FS_ABI) };
            Ok(Some(PathBeneath::new(fd, access)))
        };
        // The system set is skipped where absent (a distribution without /lib32); the profile's own paths
        // were canonicalised by `prepare`, so they exist.
        for p in LANDLOCK_SYSTEM_READ.iter().chain(SYSTEM_DEVICES).map(std::path::Path::new) {
            if let Some(r) = rule(p, read)? {
                created = created.add_rule(r).map_err(|e| format!("landlock rule {}: {e}", p.display()))?;
            }
        }
        if let Some(r) = rule(std::path::Path::new("/dev/null"), all)? {
            created = created.add_rule(r).map_err(|e| format!("landlock rule /dev/null: {e}"))?;
        }
        // The profile's own paths by the descriptors opened when it was resolved (review finding 5, #636):
        // never re-opened by name, so a path replaced since by a symlink grants nothing new.
        let n_read = profile.read.len();
        if profile.pinned.0.len() != n_read + profile.write.len() {
            return Err("the profile's paths were not pinned at its resolution".into());
        }
        for (i, (p, fd)) in profile.read.iter().chain(&profile.write).zip(&profile.pinned.0).enumerate() {
            let access = if i < n_read { read } else { all };
            let file = std::fs::File::from(fd.try_clone().map_err(|e| format!("confine path {}: {e}", p.display()))?);
            let is_dir = file.metadata().map(|m| m.is_dir()).unwrap_or(false);
            let access = if is_dir { access } else { access & AccessFs::from_file(FS_ABI) };
            created = created
                .add_rule(PathBeneath::new(std::os::fd::OwnedFd::from(file), access))
                .map_err(|e| format!("landlock rule {}: {e}", p.display()))?;
        }
        for (_, port) in &profile.connect {
            created = created.add_rule(NetPort::new(*port, AccessNet::ConnectTcp)).map_err(|e| format!("landlock port {port}: {e}"))?;
        }
        let status = created.restrict_self().map_err(|e| format!("landlock restrict_self: {e}"))?;
        if status.ruleset != RulesetStatus::FullyEnforced || !status.no_new_privs {
            return Err(format!("landlock enforced the profile only partly ({:?}) — refused", status.ruleset));
        }
        seccompiler::apply_filter(&filter(net)?).map_err(|e| format!("seccomp: {e}"))?;
        if let Some(x32) = x32_filter() {
            seccompiler::apply_filter(&x32).map_err(|e| format!("seccomp (x32): {e}"))?;
        }
        Ok(())
    }

    /// On x86_64, refuse every syscall with the x32 bit (`nr & 0x40000000`) — review finding 4 (#636). The
    /// main filter matches syscall numbers exactly and allows the rest, so `socket(2)` through the x32
    /// table (41 | 0x40000000) matched no rule. A raw cBPF program: check the architecture, load the
    /// number, refuse anything at or above the x32 bit with `EACCES`, allow the rest. `None` off x86_64.
    fn x32_filter() -> Option<BpfProgram> {
        if std::env::consts::ARCH != "x86_64" {
            return None;
        }
        use seccompiler::sock_filter;
        const LD_W_ABS: u16 = 0x20; // BPF_LD (0) | BPF_W (0) | BPF_ABS
        const JEQ_K: u16 = 0x05 | 0x10; // BPF_JMP | BPF_JEQ | BPF_K
        const JGE_K: u16 = 0x05 | 0x30; // BPF_JMP | BPF_JGE | BPF_K
        const RET_K: u16 = 0x06; // BPF_RET | BPF_K
        const AUDIT_ARCH_X86_64: u32 = 0xC000_003E;
        const RET_ALLOW: u32 = 0x7fff_0000;
        let ret_eacces: u32 = 0x0005_0000 | libc::EACCES as u32;
        let f = |code, jt, jf, k| sock_filter { code, jt, jf, k };
        Some(vec![
            f(LD_W_ABS, 0, 0, 4),                   // seccomp_data.arch
            f(JEQ_K, 1, 0, AUDIT_ARCH_X86_64),      // x86_64: go on; else allow (the main filter judges it)
            f(RET_K, 0, 0, RET_ALLOW),
            f(LD_W_ABS, 0, 0, 0),                   // seccomp_data.nr
            f(JGE_K, 0, 1, 0x4000_0000),            // the x32 bit
            f(RET_K, 0, 0, ret_eacces),
            f(RET_K, 0, 0, RET_ALLOW),
        ])
    }

    /// Restrict a fresh thread, then run the command from it: the child inherits the thread's Landlock
    /// domain and seccomp filter; the stem's other threads are untouched, and the thread ends with the
    /// command.
    pub(super) fn run(argv: &[String], timeout: Duration, profile: &Profile) -> Result<(), String> {
        let (argv, profile) = (argv.to_vec(), profile.clone());
        std::thread::Builder::new()
            .name("activation-confined".into())
            .spawn(move || {
                restrict(&profile).map_err(|e| format!("confinement could not be applied: {e} — refused, not run unconfined"))?;
                crate::activation::run(&argv, timeout)
            })
            .map_err(|e| format!("confined runner: {e}"))?
            .join()
            .map_err(|_| "confined runner panicked".to_string())?
    }
}

#[cfg(not(target_os = "linux"))]
mod linux {
    use super::Profile;
    use std::time::Duration;

    pub(super) fn available(_net: bool) -> Result<(), String> {
        Err(format!("backend landlock is Linux's, and this is {}", std::env::consts::OS))
    }

    pub(super) fn run(_argv: &[String], _timeout: Duration, _profile: &Profile) -> Result<(), String> {
        Err(format!("backend landlock is Linux's, and this is {} — refused, not run unconfined", std::env::consts::OS))
    }
}

/// The guarantee a stem's activations carry, for `GossipAgent::register_guarantee` **before** `start()`
/// (`mycelium-stem` registers it): `stem.activation_confined` — *every activation runs under its
/// confinement profile*. Resolved from `units` and this platform: **enforced** when every
/// `[[activation]]` names a profile this node can apply; **not configured** when one is unconfined (the
/// default — it runs with the stem's privileges) or names a profile this node cannot apply (refused,
/// never run unconfined) — the missing text says which; **not applicable** with no `[[activation]]`.
pub fn confinement_guarantee(units: &mycelium::NodeCapabilityConfig) -> mycelium::GuaranteeDescriptor {
    use mycelium::{GuaranteeDescriptor, GuaranteeKind, GuaranteeResolution as Resolution};
    let assessed: Vec<Confinement> = units.activations.iter().map(|a| assess(a.confine.as_ref())).collect();
    let none = assessed.is_empty();
    GuaranteeDescriptor::new(
        "stem.activation_confined",
        1,
        "stem",
        GuaranteeKind::NodeEnforced,
        "every `[[activation]]` command and probe runs under its OS confinement profile — reading only the placement directory, its declared paths and the system set (Linux: /usr/{bin,sbin,lib,lib32,lib64,libx32,libexec,share}, /bin, /sbin, /lib, /lib32, /lib64, /libx32, /etc/{ld.so.cache,ld.so.conf,ld.so.conf.d,hosts,resolv.conf,host.conf,gai.conf,nsswitch.conf,passwd,group,localtime,alternatives,ssl/certs,ssl/openssl.cnf,ca-certificates}; macOS: /usr/{bin,sbin,lib,libexec,share}, /bin, /sbin, /System, /private/var/select, /private/var/db/timezone, /private/etc/{hosts,resolv.conf,passwd,group,localtime,ssl/cert.pem,ssl/certs,ssl/openssl.cnf}, /private/var/run/resolv.conf; both: /dev/{null,zero,random,urandom}), writing only its declared write paths and /dev/null, connecting only to declared TCP ports (no Fast Open, no listen) or nowhere — never by hostname",
        "`[activation.confine]` on every `[[activation]]`; Seatbelt (`sandbox-exec`, macOS) or Landlock ABI 3 (ABI 4 with `connect`) and seccomp (Linux)",
        &["activation::hook", "activation::spawn_reprobe", "activation_sandbox::run_confined"],
        "docs/reference/unit-file.md",
        move |_| none.then_some("this unit declares no [[activation]]"),
        move |_| {
            if assessed.iter().any(|c| matches!(c, Confinement::Unconfined)) {
                Resolution::NotConfigured { missing: "[activation.confine] on every [[activation]] — an activation without one runs unconfined, with the stem's privileges" }
            } else if assessed.iter().any(|c| matches!(c, Confinement::Refused { .. })) {
                Resolution::NotConfigured { missing: "a confinement backend this node can apply — an activation whose profile it cannot apply is refused, never run unconfined" }
            } else {
                Resolution::Enforced
            }
        },
    )
}

/// Whether this platform's own backend can confine here; when it cannot, a test that shows confinement
/// has nothing to show and says so — unless `MYCELIUM_CONFINE_LIVE_REQUIRED` is set (CI sets it on the
/// runners that have a backend), when that is a failure rather than a green skip.
#[cfg(test)]
pub(crate) fn native_backend_or_skip(test: &str) -> bool {
    match assess(Some(&ConfineDecl::default())) {
        Confinement::Confined { .. } => true,
        other => {
            if std::env::var_os("MYCELIUM_CONFINE_LIVE_REQUIRED").is_some() {
                panic!("{test}: MYCELIUM_CONFINE_LIVE_REQUIRED is set and this host cannot confine: {other:?}");
            }
            eprintln!("{test}: SKIPPED — this host cannot confine ({other:?}); set MYCELIUM_CONFINE_LIVE_REQUIRED to fail instead");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// The generated Seatbelt profile refers to every path by parameter — a path holding `"` or `)`
    /// cannot become profile text — and states exactly the declared TCP destinations.
    #[test]
    fn a_seatbelt_profile_names_paths_by_parameter_and_ports_by_number() {
        let p = Profile {
            backend: ConfineBackend::Seatbelt,
            read: vec![PathBuf::from("/a\")(allow default)")],
            write: vec![PathBuf::from("/w")],
            connect: vec![(true, 11434), (false, 443)],
            pinned: Pinned::default(),
            ids: Vec::new(),
            sources: Vec::new(),
            roots: Vec::new(),
        };
        let text = seatbelt_profile(&p);
        assert!(!text.contains("allow default"), "no path is spliced into the profile: {text}");
        assert!(text.contains("(subpath (param \"R0\"))") && text.contains("(subpath (param \"W0\"))"), "{text}");
        assert!(text.contains("(remote tcp \"localhost:11434\")") && text.contains("(remote tcp \"*:443\")"), "{text}");
        assert!(text.contains("mDNSResponder"), "a `*` destination may resolve a name: {text}");
        let argv = seatbelt_argv(&p, &["cmd".into(), "arg".into()]);
        assert_eq!(&argv[argv.len() - 3..], ["--", "cmd", "arg"]);
        assert!(argv.contains(&"R0=/a\")(allow default)".to_string()));

        let none = seatbelt_profile(&Profile { connect: Vec::new(), ..p });
        assert!(!none.contains("network"), "no `connect`: no network rule at all: {none}");
    }

    /// No table: unconfined, the stated default. A backend not of this platform: refused, by name.
    #[test]
    fn assess_says_unconfined_confined_or_refused() {
        assert_eq!(assess(None), Confinement::Unconfined);
        let foreign = if cfg!(target_os = "macos") { ConfineBackend::Landlock } else { ConfineBackend::Seatbelt };
        match assess(Some(&ConfineDecl { backend: foreign, ..Default::default() })) {
            Confinement::Refused { reason } => assert!(reason.contains(foreign.as_str()), "{reason}"),
            other => panic!("a foreign backend must be refused: {other:?}"),
        }
    }

    /// A declared path that does not exist is a refusal naming it, never a profile without it.
    #[test]
    fn a_missing_declared_path_refuses_by_name() {
        if !native_backend_or_skip("a_missing_declared_path_refuses_by_name") {
            return;
        }
        let dir = std::env::temp_dir();
        let placed = dir.join("whatever");
        let c = ConfineDecl { read: vec!["/nonexistent/models-7f".into()], ..Default::default() };
        match prepare(&c, &placed) {
            Err(Refusal::Failed(m)) => assert!(m.contains("/nonexistent/models-7f") && m.contains("must exist"), "{m}"),
            other => panic!("{other:?}"),
        }
    }

    /// The guarantee report says which applies (row M): no `[[activation]]` — not applicable; one without
    /// a profile — not configured, *unconfined*; one whose profile this node cannot apply — not
    /// configured, *refused*; every one confined here — enforced. Registered before `start()`, as
    /// `mycelium-stem` does. Written after the descriptor; planted — with its check answering `enforced`
    /// regardless, the first `not_configured` assertion fails.
    #[test]
    fn the_guarantee_report_says_confined_unconfined_or_refused() {
        let state = |activations: &str| -> (String, String) {
            let units = mycelium::NodeCapabilityConfig::from_toml_str(&format!(
                "[hosts]\nkinds = [\"blob\"]\naccept_unsigned = true\n{activations}"
            ))
            .unwrap();
            let id = mycelium::NodeId::new("127.0.0.1", mycelium::test_util::alloc_port()).unwrap();
            let agent = mycelium::GossipAgent::new(id, mycelium::GossipConfig::default());
            agent.register_guarantee(confinement_guarantee(&units)).expect("registered before start");
            let r = agent.guarantee_report();
            let e = r.entry("stem.activation_confined").expect("the report carries the stem's guarantee");
            (e.resolution.state().to_string(), format!("{:?}", e.resolution))
        };
        let act = |name: &str, confine: &str| format!("[[activation]]\nns = \"a\"\nname = \"{name}\"\ncommand = [\"true\"]\n{confine}");
        let foreign = if cfg!(target_os = "macos") { "landlock" } else { "seatbelt" };

        assert_eq!(state("").0, "not_applicable");
        let (st, why) = state(&act("b", ""));
        assert_eq!(st, "not_configured");
        assert!(why.contains("unconfined"), "{why}");
        let (st, why) = state(&format!("{}{}", act("b", "[activation.confine]\n"), act("c", "")));
        assert_eq!(st, "not_configured", "one unconfined activation is enough");
        assert!(why.contains("unconfined"), "{why}");
        let (st, why) = state(&act("b", &format!("[activation.confine]\nbackend = \"{foreign}\"\n")));
        assert_eq!(st, "not_configured");
        assert!(why.contains("refused") && why.contains("never run unconfined"), "{why}");
        if native_backend_or_skip("the_guarantee_report_says_confined_unconfined_or_refused") {
            assert_eq!(state(&act("b", "[activation.confine]\n")).0, "enforced");
        }
    }

    /// Review finding 2 (#636): the always-readable sets name no directory that holds a node's own
    /// secrets — not `/etc` or `/private/etc` whole, not `/usr` whole (so not `/usr/local`) — and the
    /// guarantee's promise lists every entry.
    #[test]
    fn the_always_readable_sets_hold_no_whole_configuration_tree() {
        for set in [SEATBELT_SYSTEM_READ, LANDLOCK_SYSTEM_READ] {
            for whole in ["/", "/etc", "/private/etc", "/usr", "/usr/local", "/opt", "/home", "/Users", "/var", "/private/var"] {
                assert!(!set.contains(&whole), "{whole} is readable whole");
            }
        }
        let units = mycelium::NodeCapabilityConfig::default();
        let promise = confinement_guarantee(&units).promise;
        for p in SEATBELT_SYSTEM_READ.iter().chain(LANDLOCK_SYSTEM_READ) {
            let leaf = p.rsplit('/').next().unwrap();
            assert!(promise.contains(leaf), "the promise names {p}");
        }
    }

    /// Review of #641, finding 1: a declared path swapped for a symlink **while `prepare` resolves it** —
    /// after the symlink check and before the canonicalisation, or after it and before the pinning — must
    /// be refused, never resolved to (or pinned at) the symlink's target. Planted deterministically through
    /// the test-only stage hook. Seen failing first: at both stages `prepare` returned a profile.
    #[test]
    fn a_symlink_planted_while_prepare_resolves_is_refused() {
        if !native_backend_or_skip("a_symlink_planted_while_prepare_resolves_is_refused") {
            return;
        }
        for stage in ["checked", "resolved"] {
            let root = std::env::temp_dir().join(format!("confine-race-{stage}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            for d in ["placed", "out", "outside"] {
                std::fs::create_dir_all(root.join(d)).unwrap();
            }
            let root = root.canonicalize().unwrap();
            let (out, outside) = (root.join("out"), root.join("outside"));
            let c = ConfineDecl { write: vec![out.display().to_string()], ..Default::default() };
            let (o, x, aside) = (out.clone(), outside.clone(), root.join("out.real"));
            PREPARE_STAGE_HOOK.with(|h| {
                *h.borrow_mut() = Some(Box::new(move |at: &str| {
                    if at == stage {
                        std::fs::rename(&o, &aside).unwrap();
                        std::os::unix::fs::symlink(&x, &o).unwrap();
                    }
                }))
            });
            let got = prepare(&c, &root.join("placed/blob"));
            PREPARE_STAGE_HOOK.with(|h| *h.borrow_mut() = None);
            let _ = std::fs::remove_dir_all(&root);
            match got {
                Err(Refusal::Failed(m)) => assert!(m.contains("symlink") || m.contains("changed"), "{stage}: {m}"),
                other => panic!("{stage}: a symlink planted during the resolution must be refused, got {:?}", other.map(|p| p.write)),
            }
        }
    }

    /// Review of #641 round 2, finding A: an **intermediate** component inside a write root toggled between
    /// a real directory and a symlink out of it — real at one check, a link at the next resolution — must
    /// never leave a profile naming the link's target. Every combination over `prepare`'s three stages is
    /// tried. Seen failing first at `bd22b544`'s logic (with the test hook's stages placed as here): a link
    /// at `checked` and `resolved`, the real directory at `pinned`, a link at `rechecked` returned a profile
    /// reading `outside/x`.
    #[test]
    fn an_intermediate_component_toggled_during_prepare_never_widens_the_profile() {
        const T: &str = "an_intermediate_component_toggled_during_prepare_never_widens_the_profile";
        if !native_backend_or_skip(T) {
            return;
        }
        // `anchored` and `walked` are the walk's own stages (absent before it), so the combinations that
        // failed first are among these.
        let stages = ["checked", "resolved", "pinned", "rechecked", "anchored", "walked"];
        for combo in 0u8..64 {
            let root = std::env::temp_dir().join(format!("confine-toggle-{combo}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            for d in ["placed", "out/sub", "outside"] {
                std::fs::create_dir_all(root.join(d)).unwrap();
            }
            let root = root.canonicalize().unwrap();
            std::fs::write(root.join("out/sub/x"), b"inside").unwrap();
            std::fs::write(root.join("outside/x"), b"outside").unwrap();
            let c = ConfineDecl {
                write: vec![root.join("out").display().to_string()],
                read: vec![root.join("out/sub/x").display().to_string()],
                ..Default::default()
            };
            let (sub, real, outside) = (root.join("out/sub"), root.join("out/sub.real"), root.join("outside"));
            // Bit i set: a link at stage i; clear: the real directory.
            let toggle = move |link: bool| {
                let is_link = std::fs::symlink_metadata(&sub).is_ok_and(|m| m.file_type().is_symlink());
                if link && !is_link {
                    std::fs::rename(&sub, &real).unwrap();
                    std::os::unix::fs::symlink(&outside, &sub).unwrap();
                } else if !link && is_link {
                    std::fs::remove_file(&sub).unwrap();
                    std::fs::rename(&real, &sub).unwrap();
                }
            };
            PREPARE_STAGE_HOOK.with(|h| {
                *h.borrow_mut() = Some(Box::new(move |at: &str| {
                    if let Some(i) = stages.iter().position(|s| *s == at) {
                        toggle(combo & (1 << i) != 0);
                    }
                }))
            });
            let got = prepare(&c, &root.join("placed/blob"));
            PREPARE_STAGE_HOOK.with(|h| *h.borrow_mut() = None);
            if combo == 0 {
                assert!(got.is_ok(), "the plant: untouched, the profile resolves: {:?}", got.as_ref().err());
            }
            let named: Vec<PathBuf> = got.as_ref().map(|p| p.read.iter().chain(&p.write).cloned().collect()).unwrap_or_default();
            let _ = std::fs::remove_dir_all(&root);
            assert!(
                !named.iter().any(|p| p.starts_with(root.join("outside"))),
                "combination {combo:06b} (bits: walked/anchored/rechecked/pinned/resolved/checked; 1 = link): the profile names {named:?}"
            );
        }
    }

    /// Review of #641 round 3, finding H1: an operator symlink outside every root (`shortcut`) that points
    /// into a write root's interior (`out/sub`) is followed — the operator's — but from the moment the
    /// resolution enters the root nothing is followed, so a confined command that replaces `out/sub` with a
    /// link out of the root is refused, and the profile resolved before it stops being current. Seen failing
    /// first at `656388c0`: `prepare` named `outside/m` and `outside/w`, and the old profile stayed current.
    #[test]
    fn an_operator_symlink_into_a_write_roots_interior_cannot_be_redirected() {
        const T: &str = "an_operator_symlink_into_a_write_roots_interior_cannot_be_redirected";
        if !native_backend_or_skip(T) {
            return;
        }
        let root = std::env::temp_dir().join(format!("confine-shortcut-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for d in ["placed", "out/sub/w", "outside/w"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        let root = root.canonicalize().unwrap();
        std::fs::write(root.join("out/sub/m"), b"inside").unwrap();
        std::fs::write(root.join("outside/m"), b"outside").unwrap();
        std::os::unix::fs::symlink("out/sub", root.join("shortcut")).unwrap();
        let c = ConfineDecl {
            write: vec![root.join("out").display().to_string(), root.join("shortcut/w").display().to_string()],
            read: vec![root.join("shortcut/m").display().to_string()],
            ..Default::default()
        };
        let placed = root.join("placed/blob");
        let before = prepare(&c, &placed).expect("the plant: an operator shortcut into the root resolves while untouched");
        assert!(before.read.contains(&root.join("out/sub/m")), "{:?}", before.read);
        // What the confined command (write on `out`) can do.
        std::fs::rename(root.join("out/sub"), root.join("out/sub.real")).unwrap();
        std::os::unix::fs::symlink(root.join("outside"), root.join("out/sub")).unwrap();
        let current = before.is_current();
        let after = prepare(&c, &placed).map(|p| (p.read, p.write));
        let _ = std::fs::remove_dir_all(&root);
        assert!(!current, "the profile resolved before the redirect is not current after it");
        match after {
            Err(Refusal::Failed(m)) => assert!(m.contains("symlink"), "{m}"),
            other => panic!("a redirect inside the write root must be refused, got {other:?}"),
        }
    }

    /// Review of #641 round 3, finding H2: a placement directory inside a write root is walked like any
    /// path below one — a confined command that replaces it with a link widens nothing; the profile is
    /// refused. Seen failing first at `656388c0`: the placement directory resolved to the link's target.
    #[test]
    fn a_placement_directory_inside_a_write_root_cannot_be_redirected() {
        const T: &str = "a_placement_directory_inside_a_write_root_cannot_be_redirected";
        if !native_backend_or_skip(T) {
            return;
        }
        let root = std::env::temp_dir().join(format!("confine-placement-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for d in ["stem/placement", "outside"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        let root = root.canonicalize().unwrap();
        std::fs::write(root.join("stem/placement/blob"), b"placed").unwrap();
        let c = ConfineDecl { write: vec![root.join("stem").display().to_string()], ..Default::default() };
        let placed = root.join("stem/placement/blob");
        let before = prepare(&c, &placed).expect("the plant: untouched, the profile resolves");
        assert_eq!(before.read[0], root.join("stem/placement"));
        std::fs::rename(root.join("stem/placement"), root.join("stem/placement.real")).unwrap();
        std::os::unix::fs::symlink(root.join("outside"), root.join("stem/placement")).unwrap();
        let current = before.is_current();
        let after = prepare(&c, &placed).map(|p| p.read);
        let _ = std::fs::remove_dir_all(&root);
        assert!(!current, "the profile resolved before the redirect is not current after it");
        match after {
            Err(Refusal::Failed(m)) => assert!(m.contains("symlink"), "{m}"),
            other => panic!("a placement directory redirected inside a write root must be refused, got {other:?}"),
        }
    }

    /// Review of #641 round 2, finding B: on a case-insensitive volume (APFS by default) a declared path
    /// spelt in another case, or in another Unicode normalisation, names the same file and is accepted, the
    /// profile naming the on-disk spelling. Seen failing first at `bd22b544`: refused as "not where the
    /// symlink check found it".
    #[test]
    fn a_declared_path_spelt_in_another_case_or_normalisation_is_accepted() {
        const T: &str = "a_declared_path_spelt_in_another_case_or_normalisation_is_accepted";
        if !native_backend_or_skip(T) {
            return;
        }
        let root = std::env::temp_dir().join(format!("confine-case-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("placed")).unwrap();
        let root = root.canonicalize().unwrap();
        std::fs::create_dir(root.join("models")).unwrap();
        std::fs::create_dir(root.join("cafe\u{301}")).unwrap(); // NFD on disk
        let insensitive = root.join("MODELS").exists();
        let mut cases = vec![("models", "models"), ("cafe\u{301}", "cafe\u{301}")];
        if insensitive {
            cases.push(("Models", "models"));
        }
        if root.join("caf\u{e9}").exists() {
            cases.push(("caf\u{e9}", "cafe\u{301}")); // NFC declared
        }
        for (declared, on_disk) in cases {
            let c = ConfineDecl { read: vec![root.join(declared).display().to_string()], ..Default::default() };
            let got = prepare(&c, &root.join("placed/blob")).map(|p| p.read);
            let ok = got.as_ref().map(|r| r.iter().any(|p| p.parent() == Some(root.as_path())
                && p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.eq_ignore_ascii_case(on_disk) || n == on_disk || (declared != on_disk && n == declared)))).unwrap_or(false);
            assert!(ok, "{declared:?} (on disk {on_disk:?}): {got:?}");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Review of #641 round 2, finding C: a declared path that is a FIFO, a Unix socket or a write-only file
    /// resolves (as it did before #641) — never a hang on the FIFO, never a refusal for want of read access.
    /// Seen failing first at `bd22b544` on macOS: the socket and the write-only file were refused (the FIFO
    /// is run on a thread with a deadline, so a hang fails the test rather than wedging it).
    #[test]
    fn a_declared_fifo_socket_or_write_only_file_resolves_without_hanging() {
        const T: &str = "a_declared_fifo_socket_or_write_only_file_resolves_without_hanging";
        if !native_backend_or_skip(T) {
            return;
        }
        let root = std::env::temp_dir().join(format!("confine-kinds-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("placed")).unwrap();
        let root = root.canonicalize().unwrap();
        assert!(std::process::Command::new("mkfifo").arg(root.join("fifo")).status().unwrap().success());
        let _sock = std::os::unix::net::UnixListener::bind(root.join("sock")).unwrap();
        std::fs::write(root.join("wo"), b"").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(root.join("wo"), std::fs::Permissions::from_mode(0o200)).unwrap();
        for name in ["fifo", "sock", "wo"] {
            let c = ConfineDecl { write: vec![root.join(name).display().to_string()], ..Default::default() };
            let placed = root.join("placed/blob");
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let _ = tx.send(prepare(&c, &placed).map(|p| p.write));
            });
            let got = rx.recv_timeout(Duration::from_secs(5));
            if got.is_err() && name == "fifo" {
                // Unblock the stuck open so the thread can end.
                let _ = std::fs::OpenOptions::new().write(true).open(root.join("fifo"));
            }
            match got {
                Ok(Ok(w)) => assert_eq!(w, vec![root.join(name)], "{name}"),
                Ok(Err(e)) => panic!("{name}: refused: {e}"),
                Err(_) => panic!("{name}: prepare did not return within 5 s"),
            }
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Review of #641, finding 2: a declared path reached through an ancestor symlink that is flipped (a
    /// `current` link rotated from `v1` to `v2`) no longer resolves to what the profile named, so the
    /// profile is not current and is re-resolved. Seen failing first: `is_current` held after the flip.
    #[test]
    fn a_flipped_ancestor_symlink_makes_the_profile_not_current() {
        if !native_backend_or_skip("a_flipped_ancestor_symlink_makes_the_profile_not_current") {
            return;
        }
        let root = std::env::temp_dir().join(format!("confine-flip-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for d in ["placed", "models/v1", "models/v2"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        let root = root.canonicalize().unwrap();
        std::fs::write(root.join("models/v1/m"), b"1").unwrap();
        std::fs::write(root.join("models/v2/m"), b"2").unwrap();
        std::os::unix::fs::symlink(root.join("models/v1"), root.join("models/current")).unwrap();
        let c = ConfineDecl { read: vec![root.join("models/current/m").display().to_string()], ..Default::default() };
        let p = prepare(&c, &root.join("placed/blob")).expect("an operator's ancestor symlink outside every root is allowed");
        assert!(p.read.contains(&root.join("models/v1/m")));
        assert!(p.is_current());
        std::os::unix::fs::symlink(root.join("models/v2"), root.join("models/next")).unwrap();
        std::fs::rename(root.join("models/next"), root.join("models/current")).unwrap();
        let current = p.is_current();
        let again = prepare(&c, &root.join("placed/blob")).map(|p| p.read);
        let _ = std::fs::remove_dir_all(&root);
        assert!(!current, "the declared path now resolves elsewhere: the profile is not current");
        assert!(again.unwrap().contains(&root.join("models/v2/m")), "and a re-resolution names the new target");
    }

    /// Review follow-up 2 (#636): the symlink check compared spellings, so a declared path written through
    /// another spelling of a write root (here an alias directory) skipped its intermediate components and a
    /// symlink among them passed. Prefixes and roots are compared canonically now. Seen failing first: the
    /// profile resolved.
    #[test]
    fn a_symlink_behind_another_spelling_of_a_write_root_is_refused() {
        if !native_backend_or_skip("a_symlink_behind_another_spelling_of_a_write_root_is_refused") {
            return;
        }
        let root = std::env::temp_dir().join(format!("confine-spell-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("real/out")).unwrap();
        std::fs::create_dir_all(root.join("elsewhere/models")).unwrap();
        std::fs::create_dir_all(root.join("placed")).unwrap();
        let root = root.canonicalize().unwrap();
        std::os::unix::fs::symlink(root.join("real"), root.join("alias")).unwrap();
        std::os::unix::fs::symlink(root.join("elsewhere"), root.join("real/out/link")).unwrap();
        let c = ConfineDecl {
            write: vec![root.join("real/out").display().to_string()],
            read: vec![root.join("alias/out/link/models").display().to_string()],
            ..Default::default()
        };
        let got = prepare(&c, &root.join("placed/blob"));
        let _ = std::fs::remove_dir_all(&root);
        match got {
            Err(Refusal::Failed(m)) => assert!(m.contains("symlink"), "{m}"),
            other => panic!("a symlink inside a write root, spelt another way, must be refused: {:?}", other.map(|p| p.read)),
        }
    }
}
