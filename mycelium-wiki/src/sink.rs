//! Change sinks — downstream **projections** of the curator's applied writes.
//!
//! The system of record is the [`WikiStore`](crate::WikiStore) (per-object CAS, erasable, read
//! directly by agents). A [`ChangeSink`] is a best-effort observer the curator notifies **after** a
//! drain round lands: it derives something from the committed state — the shipped [`GitMirror`]
//! renders the corpus into a git repo so humans get reviewable commits, blame, and history — but it
//! is **never load-bearing**: a sink failure logs and counts, and can neither fail nor delay an
//! apply. Rationale + the rejected git-as-substrate alternative: `docs/design/wiki-git-store.md`.
//!
//! **Why a projection and not a `GitStore: WikiStore`:** a branch ref is a global sequencer (the
//! store contract promises *independent per-section* CAS slots), git history forfeits erasability
//! (`data-lifecycle-and-erasure.md`), and a push remote is an egress path. As a projection, git
//! orders nothing, the record stays erasable (erasure = delete the mirror, [`GitMirror::rebuild`]
//! from the store), and pushes are [`EgressPolicy`]-gated fail-closed.


/// What one curator drain round landed — the notification a [`ChangeSink`] receives. Pages name the
/// *store* state to derive from (the sink re-reads them), not a content payload: every successful
/// round re-renders from committed truth, so a missed round is healed by the next touch (or a
/// [`GitMirror::rebuild`]).
#[derive(Debug, Clone)]
pub struct AppliedRound {
    /// The wiki group.
    pub group: String,
    /// Distinct pages touched by this round, sorted.
    pub pages: Vec<String>,
    /// How many proposals landed.
    pub proposals: usize,
    /// Distinct proposal authors, sorted (provenance for the sink's record).
    pub authors: Vec<String>,
}

/// A best-effort observer of applied drain rounds. Called from the curator's drain loop **after**
/// the store writes and proposal tombstones land; implementations must be cheap or offload
/// internally (the [`GitMirror`] commits locally and pushes on a background thread), and must
/// swallow their own failures — the trait returns nothing because the apply must not depend on it.
pub trait ChangeSink: Send + Sync {
    fn round_applied(&self, round: &AppliedRound);
}

#[cfg(feature = "git-mirror")]
pub use git::{GitMirror, GitMirrorConfig};

#[cfg(feature = "git-mirror")]
mod git {
    use super::{AppliedRound, ChangeSink};
    use crate::model::Page;
    use crate::store::WikiStore;
    use mycelium::EgressPolicy;
    use std::io;
    use std::path::PathBuf;
    use std::process::Command;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

    /// Configuration for a [`GitMirror`].
    #[derive(Debug, Clone)]
    pub struct GitMirrorConfig {
        /// The mirror worktree. Created (and `git init`ed) if absent.
        pub dir: PathBuf,
        /// The branch the mirror commits to (created at init).
        pub branch: String,
        /// Optional push target (set as `origin`). `None` ⇒ local-only mirror. A non-local remote
        /// is egress-checked **fail-closed** at construction and again before every push.
        pub remote: Option<String>,
        /// The egress allowlist for the push host (WS3 posture). `None` ⇒ no restriction beyond
        /// "the host must parse"; an unparseable non-local remote is always refused.
        pub egress: Option<EgressPolicy>,
        /// Committer identity on mirror commits.
        pub author_name: String,
        /// Committer email on mirror commits.
        pub author_email: String,
    }

    impl Default for GitMirrorConfig {
        fn default() -> Self {
            Self {
                dir:          PathBuf::from("wiki-mirror"),
                branch:       "main".to_string(),
                remote:       None,
                egress:       None,
                author_name:  "mycelium-wiki curator".to_string(),
                author_email: "curator@wiki.invalid".to_string(),
            }
        }
    }

    /// The git projection sink: renders touched pages as **pure markdown** (page path + attributes
    /// as front-matter, section ids as comments — no CAS tokens; versioning is git's ancestry) into
    /// a local worktree, one commit per applied round with proposal provenance in the message, and
    /// optionally pushes to an operator-owned remote on a background thread.
    ///
    /// Uses the `git` CLI (zero added dependencies; universally present where an operator wants a
    /// git mirror). Contention discipline: [`round_applied`](ChangeSink::round_applied) `try_lock`s
    /// and **skips** if a mirror pass is already running — snapshots are idempotent, the next round
    /// catches up — so the sink never blocks the curator's drain loop on a lock.
    pub struct GitMirror<S: WikiStore> {
        store: Arc<S>,
        cfg:   GitMirrorConfig,
        /// Serialises mirror passes; `try_lock`-and-skip, never waited on.
        busy: parking_lot::Mutex<()>,
        /// At most one background push at a time; a round landing mid-push relies on the next push.
        push_in_flight: Arc<AtomicBool>,
        /// The post-push ancestry tripwire: pushes after which `ls-remote` disagreed with the local
        /// head (a force-push-permitting or diverged remote — detection, not prevention).
        push_divergences: Arc<AtomicU64>,
    }

    /// The host of a git remote URL, or `None` for a local path remote (no egress). `Err` when the
    /// remote looks non-local but the host cannot be parsed — refused, fail-closed.
    fn remote_host(remote: &str) -> Result<Option<String>, io::Error> {
        let bad = |m: &str| io::Error::new(io::ErrorKind::InvalidInput, format!("{m}: {remote:?}"));
        if let Some(rest) = remote.split_once("://").map(|(_, r)| r) {
            // scheme://[user@]host[:port]/path  (https, ssh, git, file)
            if remote.starts_with("file://") {
                return Ok(None);
            }
            let authority = rest.split('/').next().unwrap_or("");
            let host = authority.rsplit('@').next().unwrap_or("");
            let host = host.split(':').next().unwrap_or("");
            return if host.is_empty() { Err(bad("unparseable remote host")) } else { Ok(Some(host.to_string())) };
        }
        if let Some((user_host, _path)) = remote.split_once(':') {
            // scp-like user@host:path — but a Windows drive or a plain local path is not
            if user_host.contains('/') {
                return Ok(None); // a relative path containing ':' later — treat as local
            }
            let host = user_host.rsplit('@').next().unwrap_or("");
            return if host.is_empty() { Err(bad("unparseable remote host")) } else { Ok(Some(host.to_string())) };
        }
        Ok(None) // a plain filesystem path
    }

    impl<S: WikiStore> GitMirror<S> {
        /// Open (creating + `git init` if needed) the mirror over `store`. A configured non-local
        /// remote is egress-checked here, **fail-closed** — construction refuses rather than
        /// deferring the denial to the first push.
        pub fn open(store: Arc<S>, cfg: GitMirrorConfig) -> Result<Self, io::Error> {
            if let Some(remote) = &cfg.remote
                && let Some(host) = remote_host(remote)?
                && !cfg.egress.as_ref().is_none_or(|p| p.permits_host(&host))
            {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!("egress policy denies push host {host:?}"),
                ));
            }
            std::fs::create_dir_all(&cfg.dir)?;
            let me = Self {
                store,
                cfg,
                busy: parking_lot::Mutex::new(()),
                push_in_flight: Arc::new(AtomicBool::new(false)),
                push_divergences: Arc::new(AtomicU64::new(0)),
            };
            if !me.cfg.dir.join(".git").exists() {
                me.git(&["init", "-b", &me.cfg.branch])?;
            }
            if let Some(remote) = &me.cfg.remote {
                // (Re)point origin; ignore "already exists" by setting the URL unconditionally.
                if me.git(&["remote", "get-url", "origin"]).is_ok() {
                    me.git(&["remote", "set-url", "origin", remote])?;
                } else {
                    me.git(&["remote", "add", "origin", remote])?;
                }
            }
            Ok(me)
        }

        /// Run one git command in the mirror dir; non-zero exit → `Err` carrying stderr.
        fn git(&self, args: &[&str]) -> Result<String, io::Error> {
            let out = Command::new("git")
                .arg("-c").arg(format!("user.name={}", self.cfg.author_name))
                .arg("-c").arg(format!("user.email={}", self.cfg.author_email))
                .args(args)
                .current_dir(&self.cfg.dir)
                .output()?;
            if !out.status.success() {
                return Err(io::Error::other(format!(
                    "git {:?} failed: {}", args, String::from_utf8_lossy(&out.stderr).trim(),
                )));
            }
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        }

        /// Render one page as pure markdown: front-matter = page path + page attributes; each
        /// section under an id/attribute comment. **No CAS tokens** — the projection carries the
        /// document, git carries the versions.
        fn render(page: &Page) -> String {
            let mut s = String::with_capacity(256);
            s.push_str("---\n");
            s.push_str(&format!("page: {}\n", page.path));
            for (k, v) in &page.attributes {
                s.push_str(&format!("{k}: {v}\n"));
            }
            s.push_str("---\n");
            for sec in &page.sections {
                s.push('\n');
                s.push_str(&format!("<!-- section {}", sec.id));
                for (k, v) in &sec.attributes {
                    s.push_str(&format!(" · {k}={v}"));
                }
                s.push_str(" -->\n");
                s.push_str(&format!("# {}\n\n{}\n", sec.heading, sec.body));
            }
            s
        }

        /// The mirror file for a page — `{dir}/{page}.md`, defensively re-validating the path shape
        /// the store already enforces (`..`/absolute/empty components refused).
        fn page_file(&self, page: &str) -> Result<PathBuf, io::Error> {
            let mut p = self.cfg.dir.clone();
            for comp in page.split('/') {
                if comp.is_empty() || comp == "." || comp == ".." || comp.contains('\\') {
                    return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("unsafe page path {page:?}")));
                }
                p.push(comp);
            }
            p.set_extension("md");
            Ok(p)
        }

        /// Render `pages` from the store's committed state and commit if anything changed.
        /// Returns whether a commit was made.
        pub fn mirror_pages(&self, pages: &[String], message: &str) -> Result<bool, io::Error> {
            for page in pages {
                let Ok(Some(p)) = self.store.read(page) else { continue }; // vanished mid-round: next touch heals
                let file = self.page_file(page)?;
                if let Some(parent) = file.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&file, Self::render(&p))?;
            }
            self.commit_if_changed(message)
        }

        /// Regenerate the **whole** mirror from the store — the heal-everything path, and the
        /// second step of the erasure procedure (erase in the system of record, delete the mirror
        /// repo/remote, `rebuild()` a fresh history containing only the surviving corpus).
        pub fn rebuild(&self) -> Result<bool, io::Error> {
            // Drop every tracked file so pages deleted from the store leave the mirror too;
            // `git add -A` below records the deletions. Untracked files are left alone.
            let _ = self.git(&["rm", "-r", "-q", "--ignore-unmatch", "."]);
            let pages = self.store.list_pages().map_err(io::Error::other)?;
            self.mirror_pages(&pages, "wiki: rebuild projection from the system of record")
        }

        fn commit_if_changed(&self, message: &str) -> Result<bool, io::Error> {
            self.git(&["add", "-A"])?;
            if self.git(&["status", "--porcelain"])?.trim().is_empty() {
                return Ok(false); // an idempotent re-drain: nothing to record
            }
            self.git(&["commit", "-q", "-m", message])?;
            Ok(true)
        }

        /// Push `branch` to origin now (synchronous; the sink path runs this on a background
        /// thread). Re-checks egress fail-closed, then runs the **divergence tripwire**: after the
        /// push, `ls-remote` must agree with the local head — a mismatch (a force-push-permitting
        /// remote, a concurrent writer) is counted and warned, never "fixed" (detection, not
        /// prevention).
        pub fn push_now(&self) -> Result<(), io::Error> {
            push_sync(&self.cfg, &self.push_divergences)
        }

        /// Pushes after which the remote head disagreed with the local head (the tripwire count).
        pub fn push_divergences(&self) -> u64 {
            self.push_divergences.load(Ordering::Relaxed)
        }

        /// Background push with an in-flight guard: at most one push at a time; a round landing
        /// mid-push is carried by the next round's push (the mirror is a snapshot, nothing is lost).
        fn spawn_push(&self) {
            if self.cfg.remote.is_none() || self.push_in_flight.swap(true, Ordering::AcqRel) {
                return;
            }
            let cfg = self.cfg.clone();
            let divergences = Arc::clone(&self.push_divergences);
            let in_flight = Arc::clone(&self.push_in_flight);
            std::thread::spawn(move || {
                if let Err(e) = push_sync(&cfg, &divergences) {
                    tracing::warn!(error = %e, "wiki git-mirror: push failed (mirror is best-effort)");
                }
                in_flight.store(false, Ordering::Release);
            });
        }
    }

    /// The push body, free of `&GitMirror` so the background thread borrows nothing.
    fn push_sync(cfg: &GitMirrorConfig, divergences: &AtomicU64) -> Result<(), io::Error> {
        let Some(remote) = &cfg.remote else { return Ok(()) };
        if let Some(host) = remote_host(remote)?
            && !cfg.egress.as_ref().is_none_or(|p| p.permits_host(&host))
        {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied,
                format!("egress policy denies push host {host:?}")));
        }
        let run = |args: &[&str]| -> Result<String, io::Error> {
            let out = Command::new("git").args(args).current_dir(&cfg.dir).output()?;
            if !out.status.success() {
                return Err(io::Error::other(format!(
                    "git {:?} failed: {}", args, String::from_utf8_lossy(&out.stderr).trim())));
            }
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        };
        run(&["push", "-q", "origin", &cfg.branch])?;
        let local = run(&["rev-parse", "HEAD"])?.trim().to_string();
        let remote_head = run(&["ls-remote", "origin", &cfg.branch])?;
        if !remote_head.starts_with(&local) {
            divergences.fetch_add(1, Ordering::Relaxed);
            tracing::warn!(local, remote_head = remote_head.trim(),
                "wiki git-mirror: remote head diverged after push (tripwire)");
        }
        Ok(())
    }

    impl<S: WikiStore + 'static> ChangeSink for GitMirror<S> {
        fn round_applied(&self, round: &AppliedRound) {
            // Skip, don't wait, if a pass is already running: snapshots are idempotent and the next
            // round re-reads committed state — the sink never blocks the drain loop on a lock.
            let Some(_pass) = self.busy.try_lock() else { return };
            let message = format!(
                "wiki({}): {} proposal(s) by {} → {}",
                round.group, round.proposals,
                if round.authors.is_empty() { "?".to_string() } else { round.authors.join(", ") },
                round.pages.join(", "),
            );
            match self.mirror_pages(&round.pages, &message) {
                Ok(true)  => self.spawn_push(),
                Ok(false) => {}
                Err(e)    => tracing::warn!(error = %e, "wiki git-mirror: mirror pass failed (best-effort; rebuild() heals)"),
            }
        }
    }
}
