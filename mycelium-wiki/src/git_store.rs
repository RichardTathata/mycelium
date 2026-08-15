//! `GitStore` — the **git-as-truth** [`WikiStore`]: pages are markdown files in a git checkout, and
//! every write is a commit. The eligibility-envelope build of
//! `docs/design/wiki-git-store.md` (E1 public-record corpus · E2 single-curator topology · E3
//! operator-owned locked remote · E4 git-native readers), designed for the Transparency-Platform
//! council-wiki deployment (`docs/design/transparency-council-substrate.md`, Phase 1).
//!
//! **Layout.** One page = one file, `{dir}/{subdir}/{page}.md` — a real markdown document (page
//! attributes + manifest in front-matter, each section under an HTML-comment marker with a visible
//! `# heading`). Identity is the path; **no version token ever appears in the document** — the
//! commit history is the version history, and CAS tokens are derived (below).
//!
//! **CAS = content-hash equality, not counters.** A section's version token is a 64-bit FNV-1a of
//! its serialized content; the manifest's likewise. `write_section(expected)` conflicts iff the
//! section's *content at HEAD* differs from what the caller read — which is precisely what a
//! merge-based writer needs (its reconcile base must equal the committed state). Two consequences,
//! both deliberate:
//! - **Per-section independence survives the single-file layout**: a commit that changed section X
//!   leaves section Y's bytes — and therefore Y's token — unchanged, so a concurrent Y-writer's CAS
//!   still passes (the writer rebuilds its file on the new HEAD and retries the ref).
//! - Tokens are **equality-only** (unordered), and an A→B→A history re-yields A's token — safe for
//!   this contract, since a base equal to the committed content is a valid reconcile base. An
//!   accidental 64-bit collision (~2⁻⁶⁴ per comparison) is documented, not defended; this store is
//!   curator-side (CFT, not BFT).
//!
//! **Atomicity.** Writes go through git plumbing against a **private temporary index**
//! (`hash-object → read-tree → update-index → write-tree → commit-tree`) and land with an atomic
//! `update-ref <new> <old>` — a true compare-and-swap on the branch head. A raced ref simply
//! retries on the new head; a raced *section* (its content moved) is a [`WikiError::Conflict`].
//! One internal mutex serializes writes per store instance (the deployment contract is one store
//! instance per checkout — E2's single curator; the ref-CAS is the backstop against a second
//! instance, exercised by the two-instance race test). The user's real index and staging area are
//! never touched, and commits carry only the written path — the scoped-commit discipline
//! (`councils/<slug>`, never `-A`) falls out of the plumbing rather than being a rule.
//!
//! **Reads are at HEAD**, never the working tree ("measure from a named commit, never from the
//! working tree" — the deployment's own hard-won rule). After each commit the store syncs the
//! written file into the worktree (temp + rename) so humans and external validators see current
//! files, but truth is the committed blob.
//!
//! **Reserved sequences** (refused at write with a clear error, never silently mangled): a section
//! heading may not contain a newline, and a body may not contain a line beginning with the section
//! marker `<!-- mycelium-section `. Nothing else about the content is constrained.
//!
//! **Topology rule (P6.4).** One clone per curator node is the deployed shape: different councils.
//! curators then share NO local ref — the only serialization is per-round `publish` to the shared
//! remote (push + worktree-free merge). Co-locating many groups. stores over ONE checkout
//! re-introduces the shared-branch-ref ceiling: writes across all groups serialize on that ref
//! (jittered-backoff retries queue them — measured in the P6.4 gate — but throughput is bounded by
//! one commit at a time). Prefer clone-per-group when councils share a process.
//!
//! Zero dependencies beyond the `git` CLI. Replication is `refresh`/`publish` against the
//! configured remote (P6.3); push cadence is per applied round.

use std::collections::BTreeMap;
use std::io;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::model::{Manifest, Page, Predicate, Section, SectionId, SectionRef, WikiError};
use crate::store::{PageWrite, VersionedPage, WikiStore};

/// The section marker prefix — a line beginning with this opens a section block.
const MARKER: &str = "<!-- mycelium-section ";

/// Configuration for a [`GitStore`].
#[derive(Debug, Clone)]
pub struct GitStoreConfig {
    /// The checkout root. Created (and `git init -b {branch}`ed) if absent.
    pub dir: PathBuf,
    /// The branch the store commits to.
    pub branch: String,
    /// Pages live under `{dir}/{subdir}/…` — the store's scope inside the repo. For the
    /// council-wiki deployment this is `councils/<slug>` (group = council). May be empty
    /// (pages at the repo root).
    pub subdir: String,
    /// Committer/author identity on store commits.
    pub author_name: String,
    pub author_email: String,
    /// Commit-message prefix, e.g. `wiki(norfolk)` — messages read `{prefix}: write {page}`.
    pub message_prefix: String,
    /// Optional **write gate** (council-substrate Phase 3): a command run before every commit with
    /// the candidate file in place in the worktree — `cwd` = the repo dir, argv = this vector plus
    /// the candidate's repo-relative path. **Exit 0 admits; any other exit refuses** the write with
    /// the command's output as the findings (surfaced as [`WikiError::gate_refusal`], which the
    /// curator treats as drop-with-findings, not retry). The worktree is restored to its prior
    /// state after the check either way. For the council-wiki deployment this is the Node
    /// validator in listed-only mode (`["node", "validation/validate.js", "--root", ".",
    /// "--listed-only"]`) — its cost per run is the deployment's to manage (their #1359 tracks the
    /// validator's own speed); the gate contract is deliberately just "a command over the tree".
    pub validate_cmd: Option<Vec<String>>,
    /// Optional shared remote (P6.3 — the failover topology's source of truth across nodes; the
    /// E3 operator-owned, force-push-locked repo). When set: [`refresh`](crate::WikiStore::refresh)
    /// fetches and **adopts the remote head** (the promotion step — a promoted curator.s clone
    /// catches up before it serves), and [`publish`](crate::WikiStore::publish) pushes the branch
    /// (with a worktree-free **subtree splice** retry — no merge base needed, so cold-start
    /// unrelated roots converge — and a post-push `ls-remote` **divergence tripwire**, counted
    /// and warned, never auto-fixed).
    /// `None` = a local-only store (single-node deployments; every sync is a no-op).
    pub remote: Option<String>,
}

impl Default for GitStoreConfig {
    fn default() -> Self {
        Self {
            dir:            PathBuf::from("wiki-repo"),
            branch:         "main".to_string(),
            subdir:         "pages".to_string(),
            author_name:    "mycelium-wiki curator".to_string(),
            author_email:   "curator@wiki.invalid".to_string(),
            message_prefix: "wiki".to_string(),
            validate_cmd:   None,
            remote:         None,
        }
    }
}

impl GitStoreConfig {
    /// The **group-per-council convention** (council-substrate Phase 2,
    /// `docs/design/transparency-council-substrate.md` §4): one repo, one store instance per group,
    /// each scoped to its own subtree — `subdir = councils/{group}`, commit messages prefixed
    /// `wiki({group})`. The group label is the write domain: a council slug for one-council groups,
    /// or a shard label (a region, a batch) for a set of councils whose pages then nest as
    /// `{council}/{page}` inside the shard's subtree. N groups over one repo = N independent
    /// single-writer domains sharing one branch; commits never cross subtrees (each carries only
    /// its written path) and cross-instance interleaving is handled by the ref CAS.
    pub fn for_group(dir: impl Into<PathBuf>, group: &str) -> Self {
        Self {
            dir:            dir.into(),
            subdir:         format!("councils/{group}"),
            message_prefix: format!("wiki({group})"),
            ..Self::default()
        }
    }

    /// Set the shared remote (P6.3): the cross-node source of truth for refresh/publish.
    pub fn with_remote(mut self, remote: impl Into<String>) -> Self {
        self.remote = Some(remote.into());
        self
    }
}

/// A git-checkout-backed group wiki. See the module docs for the contract.
pub struct GitStore {
    cfg: GitStoreConfig,
    /// Serializes this instance's writes (the read-validate-commit sequence). Internal only, never
    /// composed with another lock; held across local git subprocess I/O by design — the store's
    /// writer is a single curator, and the atomic `update-ref` CAS is the cross-instance backstop.
    write_lock: Mutex<()>,
    /// The persistent `git cat-file --batch` child (P6.2): blob reads are pipe round-trips on one
    /// long-lived process, not a spawn per read — the difference between a corpus-scale `query`
    /// costing two subprocesses and costing ten thousand. Lazily spawned, respawned once on death,
    /// reaped on drop. The lock is internal-only and held for one pipe round-trip (µs).
    cat_file: Mutex<Option<CatFile>>,
    /// The post-push ancestry tripwire (P6.3): pushes after which `ls-remote` disagreed with the
    /// local head — a force-push-permitting or concurrently-written remote. Detection, never
    /// auto-fixed.
    push_divergences: AtomicU64,
    tmp_seq: AtomicU64,
}

/// The persistent read child. Dropping it closes stdin (git exits) and reaps the process.
struct CatFile {
    child:  std::process::Child,
    stdin:  std::process::ChildStdin,
    stdout: std::io::BufReader<std::process::ChildStdout>,
}

impl Drop for CatFile {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait(); // reap — no zombie
    }
}

/// Process-global temp-name uniqueness (P6.4 finding: per-INSTANCE counters collided when N
/// stores share one process + one checkout — same `.mycelium-index.{pid}.{seq}` names → concurrent
/// private indexes corrupted each other, and colliding worktree temp names could cross-rename).
static TMP_UNIQUE: AtomicU64 = AtomicU64::new(0);

// ── content hashing (version tokens) ────────────────────────────────────────────

fn fnv64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce4_84222325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

fn section_token(s: &Section) -> Result<u64, WikiError> {
    Ok(fnv64(&serde_json::to_vec(s)?))
}

fn manifest_token(m: &Manifest) -> Result<u64, WikiError> {
    Ok(fnv64(&serde_json::to_vec(m)?))
}

// ── the page file format (char-exact round trip) ────────────────────────────────

/// The parsed shape of one page file: the manifest (present iff the page "exists" for readers) and
/// every section block in file order — including orphans not referenced by the manifest.
#[derive(Default)]
struct PageFile {
    manifest: Option<Manifest>,
    blocks:   Vec<Section>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct BlockMeta {
    id: SectionId,
    #[serde(default)]
    attributes: BTreeMap<String, String>,
}

fn bad_content(msg: String) -> WikiError {
    WikiError::Io(io::Error::new(io::ErrorKind::InvalidInput, msg))
}

fn render(pf: &PageFile) -> Result<String, WikiError> {
    let mut out = String::with_capacity(256);
    out.push_str("---\n");
    if let Some(m) = &pf.manifest {
        out.push_str("manifest: ");
        out.push_str(&serde_json::to_string(m)?);
        out.push('\n');
    }
    out.push_str("---\n");
    for s in &pf.blocks {
        if s.heading.contains('\n') {
            return Err(bad_content(format!("section {:?}: heading may not contain a newline", &*s.id)));
        }
        if s.body.starts_with(MARKER) || s.body.contains(&format!("\n{MARKER}")) {
            return Err(bad_content(format!(
                "section {:?}: body may not contain a line beginning with the reserved marker {MARKER:?}",
                &*s.id
            )));
        }
        let meta = BlockMeta { id: s.id.clone(), attributes: s.attributes.clone() };
        out.push('\n');
        out.push_str(MARKER);
        out.push_str(&serde_json::to_string(&meta)?); // serde never emits raw newlines in strings
        out.push_str(" -->\n# ");
        out.push_str(&s.heading);
        out.push_str("\n\n");
        out.push_str(&s.body);
        out.push('\n');
    }
    Ok(out)
}

fn parse(text: &str) -> Result<PageFile, WikiError> {
    let corrupt = |what: &str| bad_content(format!("corrupt page file: {what}"));
    let rest = text.strip_prefix("---\n").ok_or_else(|| corrupt("missing front-matter open"))?;
    let (front, mut body) = match rest.strip_prefix("---\n") {
        Some(b) => ("", b), // empty front-matter block
        None => {
            let end = rest.find("\n---\n").ok_or_else(|| corrupt("missing front-matter close"))?;
            (&rest[..end], &rest[end + 5..])
        }
    };
    let mut pf = PageFile::default();
    for line in front.lines() {
        if let Some(json) = line.strip_prefix("manifest: ") {
            pf.manifest = Some(serde_json::from_str(json)?);
        } // unknown front-matter keys are ignored (forward compatibility)
    }
    // Section blocks: each starts at a line beginning with MARKER. Render always writes a blank
    // line before the marker, so boundaries are "\n" + MARKER (or MARKER at the very start).
    loop {
        let start = if body.starts_with('\n') && body[1..].starts_with(MARKER) {
            1
        } else if body.starts_with(MARKER) {
            0
        } else if body.trim().is_empty() {
            break; // no (more) blocks
        } else {
            return Err(corrupt("content outside any section block"));
        };
        let seg = &body[start + MARKER.len()..];
        let meta_end = seg.find(" -->\n").ok_or_else(|| corrupt("unterminated section marker"))?;
        let meta: BlockMeta = serde_json::from_str(&seg[..meta_end])?;
        let seg = &seg[meta_end + 5..];
        let seg = seg.strip_prefix("# ").ok_or_else(|| corrupt("missing section heading"))?;
        let h_end = seg.find('\n').ok_or_else(|| corrupt("unterminated heading"))?;
        let heading = &seg[..h_end];
        let seg = seg[h_end + 1..].strip_prefix('\n').ok_or_else(|| corrupt("missing blank line after heading"))?;
        // Body runs to the next block boundary ("\n" + MARKER — the newline is the NEXT block's
        // leading frame, not part of this body) or to end-of-file; strip exactly the one trailing
        // newline render appended, so any body round-trips byte-exactly.
        let (raw, next) = match seg.find(&format!("\n{MARKER}")) {
            Some(p) => (&seg[..p], &seg[p..]),
            None    => (seg, ""),
        };
        let body_text = raw.strip_suffix('\n').ok_or_else(|| corrupt("unterminated section body"))?;
        pf.blocks.push(Section {
            id:         meta.id,
            heading:    heading.to_string(),
            body:       body_text.to_string(),
            attributes: meta.attributes,
        });
        body = next;
    }
    Ok(pf)
}

// ── git plumbing ────────────────────────────────────────────────────────────────

enum CommitOutcome {
    Committed,
    /// The batch's tree equals the head tree — an idempotent re-apply; nothing to record.
    NoChange,
    /// The branch ref moved between our HEAD read and the update — rebuild on the new head and retry.
    RefMoved,
}

impl GitStore {
    /// Open (creating + `git init -b {branch}` if needed) the store.
    pub fn open(cfg: GitStoreConfig) -> Result<Self, WikiError> {
        std::fs::create_dir_all(&cfg.dir)?;
        let me = Self { cfg, write_lock: Mutex::new(()), cat_file: Mutex::new(None), push_divergences: AtomicU64::new(0), tmp_seq: AtomicU64::new(0) };
        if !me.cfg.dir.join(".git").exists() {
            // Tolerate losing an init race (P6.4 finding: N stores opening one shared checkout
            // concurrently): a failed init is fine iff someone else.s init landed.
            let (_, ok) = me.git_raw(&["init", "-q", "-b", &me.cfg.branch.clone()], None)?;
            if !ok && !me.cfg.dir.join(".git").exists() {
                return Err(WikiError::Io(io::Error::other("git init failed")));
            }
        }
        if let Some(remote) = &me.cfg.remote {
            // (Re)point origin at the configured shared remote (P6.3).
            if me.git_try(&["remote", "get-url", "origin"])?.is_some() {
                me.git_ok(&["remote", "set-url", "origin", remote], None)?;
            } else {
                me.git_ok(&["remote", "add", "origin", remote], None)?;
            }
        }
        Ok(me)
    }

    /// Pushes after which the remote head disagreed with the local head (the P6.3 divergence
    /// tripwire — a force-push-permitting or concurrently-written remote; detection, never fixed).
    pub fn push_divergences(&self) -> u64 {
        self.push_divergences.load(Ordering::Relaxed)
    }

    fn refname(&self) -> String {
        format!("refs/heads/{}", self.cfg.branch)
    }

    /// Run git in the checkout; `Ok(stdout)` on success, `Err` carrying stderr on failure.
    fn git_ok(&self, args: &[&str], stdin: Option<&[u8]>) -> Result<Vec<u8>, WikiError> {
        match self.git_raw(args, stdin)? {
            (out, true)  => Ok(out),
            (_, false)   => Err(WikiError::Io(io::Error::other(format!("git {args:?} failed")))),
        }
    }

    /// Run git; `Ok(None)` on a nonzero exit (an expected "absent" answer: unborn branch, missing path).
    fn git_try(&self, args: &[&str]) -> Result<Option<Vec<u8>>, WikiError> {
        let (out, ok) = self.git_raw(args, None)?;
        Ok(ok.then_some(out))
    }

    fn git_raw(&self, args: &[&str], stdin: Option<&[u8]>) -> Result<(Vec<u8>, bool), WikiError> {
        self.git_env(args, stdin, &[])
    }

    fn git_env(
        &self, args: &[&str], stdin: Option<&[u8]>, envs: &[(&str, &str)],
    ) -> Result<(Vec<u8>, bool), WikiError> {
        let mut cmd = Command::new("git");
        cmd.args(args).current_dir(&self.cfg.dir).stderr(Stdio::piped()).stdout(Stdio::piped());
        for (k, v) in envs {
            cmd.env(k, v);
        }
        let out = if let Some(bytes) = stdin {
            use std::io::Write;
            cmd.stdin(Stdio::piped());
            let mut child = cmd.spawn().map_err(io_ctx("spawning git"))?;
            child.stdin.take().expect("piped stdin").write_all(bytes)?;
            child.wait_with_output()?
        } else {
            cmd.output().map_err(io_ctx("spawning git"))?
        };
        Ok((out.stdout, out.status.success()))
    }

    /// The branch head, or `None` for an unborn branch (an empty store).
    fn head(&self) -> Result<Option<String>, WikiError> {
        Ok(self
            .git_try(&["rev-parse", "--verify", "--quiet", &self.refname()])?
            .map(|b| String::from_utf8_lossy(&b).trim().to_string())
            .filter(|s| !s.is_empty()))
    }

    /// Resolve + validate a page path to its repo-relative file path.
    fn rel_path(&self, page: &str) -> Result<String, WikiError> {
        for comp in page.split('/') {
            if comp.is_empty() || comp == "." || comp == ".." || comp == ".git" || comp.contains('\\') {
                return Err(WikiError::BadPath(page.to_string()));
            }
        }
        Ok(if self.cfg.subdir.is_empty() {
            format!("{page}.md")
        } else {
            format!("{}/{page}.md", self.cfg.subdir)
        })
    }

    fn spawn_cat_file(&self) -> Result<CatFile, WikiError> {
        let mut child = Command::new("git")
            .args(["cat-file", "--batch"])
            .current_dir(&self.cfg.dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(io_ctx("spawning git cat-file"))?;
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = std::io::BufReader::new(child.stdout.take().expect("piped stdout"));
        Ok(CatFile { child, stdin, stdout })
    }

    /// One blob via the persistent `cat-file --batch` child (P6.2): a pipe round-trip, not a
    /// process spawn. `Ok(None)` for a missing object (an absent page). A dead child is respawned
    /// once; a second failure surfaces.
    fn read_blob(&self, spec: &str) -> Result<Option<Vec<u8>>, WikiError> {
        let mut guard = self.cat_file.lock().unwrap_or_else(|e| e.into_inner());
        for attempt in 0..2 {
            if guard.is_none() {
                *guard = Some(self.spawn_cat_file()?);
            }
            let cf = guard.as_mut().expect("just spawned");
            match Self::read_blob_on(cf, spec) {
                Ok(v) => return Ok(v),
                Err(_) if attempt == 0 => *guard = None, // child died — respawn once and retry
                Err(e) => return Err(WikiError::Io(e)),
            }
        }
        unreachable!("two attempts always return or error")
    }

    fn read_blob_on(cf: &mut CatFile, spec: &str) -> io::Result<Option<Vec<u8>>> {
        use std::io::{BufRead, Read, Write};
        writeln!(cf.stdin, "{spec}")?;
        cf.stdin.flush()?;
        let mut header = String::new();
        if cf.stdout.read_line(&mut header)? == 0 {
            return Err(io::Error::other("cat-file: closed stream"));
        }
        let header = header.trim_end();
        if header.ends_with(" missing") || header.ends_with(" ambiguous") {
            return Ok(None); // an absent path at this commit — a normal answer
        }
        let size: usize = header
            .rsplit(' ')
            .next()
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| io::Error::other(format!("cat-file: unparseable header {header:?}")))?;
        let mut buf = vec![0u8; size + 1]; // content + the protocol's trailing newline
        cf.stdout.read_exact(&mut buf)?;
        buf.pop();
        Ok(Some(buf))
    }

    /// The committed page file at `head`, parsed. `Ok(None)` if the file does not exist there.
    fn load_at(&self, head: &str, rel: &str) -> Result<Option<(String, PageFile)>, WikiError> {
        let Some(bytes) = self.read_blob(&format!("{head}:{rel}"))? else { return Ok(None) };
        let text = String::from_utf8(bytes)
            .map_err(|_| bad_content(format!("page file {rel:?} is not UTF-8")))?;
        let pf = parse(&text)?;
        Ok(Some((text, pf)))
    }

    /// Every page-shaped file under the store scope at `head`, as `(page, rel-path)` — **one**
    /// `ls-tree` per call (P6.2: `query`/`list_pages` share this instead of re-walking per page).
    fn page_files_at(&self, head: &str) -> Result<Vec<(String, String)>, WikiError> {
        let mut args = vec!["ls-tree", "-r", "-z", "--name-only", head];
        let scope;
        if !self.cfg.subdir.is_empty() {
            scope = self.cfg.subdir.clone();
            args.push("--");
            args.push(&scope);
        }
        let out = self.git_ok(&args, None)?;
        let prefix = if self.cfg.subdir.is_empty() { String::new() } else { format!("{}/", self.cfg.subdir) };
        let mut files = Vec::new();
        for name in out.split(|&b| b == 0).filter(|s| !s.is_empty()) {
            let name = String::from_utf8_lossy(name).into_owned();
            let Some(rel) = name.strip_prefix(&prefix) else { continue };
            let Some(page) = rel.strip_suffix(".md") else { continue };
            files.push((page.to_string(), name.clone()));
        }
        Ok(files)
    }

    fn load(&self, page: &str) -> Result<Option<PageFile>, WikiError> {
        let rel = self.rel_path(page)?;
        let Some(head) = self.head()? else { return Ok(None) };
        Ok(self.load_at(&head, &rel)?.map(|(_, pf)| pf))
    }

    /// Commit `content` as the sole change at `rel`, on top of `head`, with an atomic ref CAS.
    fn commit_file(
        &self, head: Option<&str>, rel: &str, content: &str, message: &str,
    ) -> Result<CommitOutcome, WikiError> {
        self.commit_files(head, &[(rel, content)], message)
    }

    /// Commit `files` as **one commit** on top of `head`, with an atomic ref CAS (P6.1: the batch
    /// primitive — a per-meeting batch lands as the deployment's per-meeting boundary commit). A
    /// batch whose tree equals the head tree is [`CommitOutcome::NoChange`] — an idempotent
    /// re-apply records nothing.
    fn commit_files(
        &self, head: Option<&str>, files: &[(&str, &str)], message: &str,
    ) -> Result<CommitOutcome, WikiError> {
        // Blobs into the object database (content only; nothing touches the user's index).
        let mut blobs = Vec::with_capacity(files.len());
        for (_, content) in files {
            let blob =
                String::from_utf8_lossy(&self.git_ok(&["hash-object", "-w", "--stdin"], Some(content.as_bytes()))?)
                    .trim()
                    .to_string();
            blobs.push(blob);
        }
        // A private index: base it on the head tree, splice the blobs, write the new tree.
        let idx = self.cfg.dir.join(".git").join(format!(
            ".mycelium-index.{}.{}",
            std::process::id(),
            TMP_UNIQUE.fetch_add(1, Ordering::Relaxed)
        ));
        let idx_s = idx.to_string_lossy().into_owned();
        let index_env: [(&str, &str); 1] = [("GIT_INDEX_FILE", &idx_s)];
        let plumb = |args: &[&str]| -> Result<Vec<u8>, WikiError> {
            match self.git_env(args, None, &index_env)? {
                (out, true) => Ok(out),
                (_, false)  => Err(WikiError::Io(io::Error::other(format!("git {args:?} failed")))),
            }
        };
        let result = (|| {
            match head {
                Some(h) => plumb(&["read-tree", h])?,
                None    => plumb(&["read-tree", "--empty"])?,
            };
            // One index splice for the whole batch (P6.2): `--index-info` reads
            // `mode SP sha TAB path` lines from stdin — N files, one subprocess.
            let mut info = String::with_capacity(files.len() * 64);
            for ((rel, _), blob) in files.iter().zip(&blobs) {
                info.push_str(&format!("100644 {blob}\t{rel}\n"));
            }
            match self.git_env(&["update-index", "--add", "--index-info"], Some(info.as_bytes()), &index_env)? {
                (_, true)  => {}
                (_, false) => return Err(WikiError::Io(io::Error::other("git update-index --index-info failed"))),
            }
            let tree = String::from_utf8_lossy(&plumb(&["write-tree"])?).trim().to_string();
            // Idempotent batch: identical tree ⇒ nothing to record (no empty commits).
            if let Some(h) = head {
                let spec = format!("{h}^{{tree}}");
                let head_tree = String::from_utf8_lossy(&self.git_ok(&["rev-parse", &spec], None)?)
                    .trim()
                    .to_string();
                if head_tree == tree {
                    return Ok(CommitOutcome::NoChange);
                }
            }
            // The commit object (author/committer from config, not repo state).
            let ident: [(&str, &str); 4] = [
                ("GIT_AUTHOR_NAME", &self.cfg.author_name),
                ("GIT_AUTHOR_EMAIL", &self.cfg.author_email),
                ("GIT_COMMITTER_NAME", &self.cfg.author_name),
                ("GIT_COMMITTER_EMAIL", &self.cfg.author_email),
            ];
            let commit_args: Vec<&str> = match head {
                Some(h) => vec!["commit-tree", &tree, "-p", h, "-m", message],
                None    => vec!["commit-tree", &tree, "-m", message],
            };
            let commit = match self.git_env(&commit_args, None, &ident)? {
                (out, true) => String::from_utf8_lossy(&out).trim().to_string(),
                (_, false)  => return Err(WikiError::Io(io::Error::other("git commit-tree failed"))),
            };
            // The atomic compare-and-swap: advance the ref iff it still points at `head`
            // (old-value "" asserts creation for the unborn branch).
            let refname = self.refname();
            let old = head.unwrap_or("");
            let (_, ok) = self.git_raw(&["update-ref", &refname, &commit, old], None)?;
            if !ok {
                return Ok(CommitOutcome::RefMoved);
            }
            // Sync the worktree copies (temp + rename — atomic, never torn) so direct readers and
            // external validators see current files; truth remains the committed blobs.
            for (rel, content) in files {
                let dst = self.cfg.dir.join(rel);
                if let Some(parent) = dst.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let tmp = self.cfg.dir.join(format!(
                    ".mycelium-wt.{}.{}",
                    std::process::id(),
                    TMP_UNIQUE.fetch_add(1, Ordering::Relaxed)
                ));
                std::fs::write(&tmp, content)?;
                std::fs::rename(&tmp, &dst)?;
            }
            Ok(CommitOutcome::Committed)
        })();
        let _ = std::fs::remove_file(&idx); // best-effort cleanup of the private index
        result
    }

    /// The write loop shared by every mutating call: read head → validate → mutate → commit, with
    /// ref-moved retries. `validate_and_mutate` sees the current `PageFile` (empty if absent) and
    /// either returns the version token to report or a `Conflict`.
    fn write_with<F>(&self, page: &str, message: &str, mut validate_and_mutate: F) -> Result<u64, WikiError>
    where
        F: FnMut(&mut PageFile) -> Result<u64, WikiError>,
    {
        let rel = self.rel_path(page)?;
        let _guard = self.write_lock.lock().unwrap_or_else(|e| e.into_inner());
        for attempt in 0..32 {
            if attempt > 0 {
                self.backoff(attempt); // P6.4: queue briefly under ref contention, do not error
            }
            let head = self.head()?;
            let current = match &head {
                Some(h) => self.load_at(h, &rel)?,
                None    => None,
            };
            let (old_text, mut pf) = match current {
                Some((t, pf)) => (Some(t), pf),
                None          => (None, PageFile::default()),
            };
            let token = validate_and_mutate(&mut pf)?; // Conflict propagates from here
            let content = render(&pf)?;
            if old_text.as_deref() == Some(content.as_str()) {
                return Ok(token); // an idempotent re-apply: nothing to record, no empty commit
            }
            // The write gate (Phase 3): candidate on disk → run the deployment's validator →
            // refuse (a non-retry error carrying the findings) or admit. A refusal is checked
            // before the commit, so a refused write leaves neither a commit nor worktree residue.
            if self.cfg.validate_cmd.is_some() {
                self.gate_check(&rel, &content)?;
            }
            match self.commit_file(head.as_deref(), &rel, &content, message)? {
                CommitOutcome::Committed | CommitOutcome::NoChange => return Ok(token),
                CommitOutcome::RefMoved => continue, // rebuilt on the new head next iteration
            }
        }
        Err(WikiError::Conflict) // persistently losing the ref race — report as contention
    }

    /// Jittered EXPONENTIAL backoff between ref-contention retries (P6.4). The CAS window spans
    /// the whole commit build (~100 ms of subprocesses), so under a write burst the backoff must
    /// grow PAST the commit interval or losers retry straight into the storm and starve — the
    /// first cut (linear, 24 ms cap) did exactly that in the ten-council gate. Doubling with
    /// proportional jitter, capped at 800 ms: contention queues, never errors.
    fn backoff(&self, attempt: usize) {
        let step = (5u64 << attempt.min(8)).min(800); // 10, 20, 40, … 800 ms
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| u64::from(d.subsec_nanos()))
            .unwrap_or(0);
        let salt = self.tmp_seq.fetch_add(1, Ordering::Relaxed);
        let jitter = fnv64(&(nanos ^ salt).to_le_bytes()) % (step / 2 + 1);
        std::thread::sleep(std::time::Duration::from_millis(step + jitter));
    }

    fn message(&self, verb: &str, page: &str) -> String {
        format!("{}: {verb} {page}", self.cfg.message_prefix)
    }

    /// Run the deployment's write gate over the candidate: place the candidate file in the
    /// worktree, run `validate_cmd + [rel]` with `cwd` = the repo dir, then **restore the prior
    /// worktree state either way** (an admitted write is re-synced by the commit itself; a refused
    /// one must leave no residue). Exit 0 admits; anything else refuses with the command's output
    /// as the findings.
    fn gate_check(&self, rel: &str, candidate: &str) -> Result<(), WikiError> {
        self.gate_check_batch(&[(rel, candidate)])
    }

    /// The write gate over a whole batch (P6.1): place **every** candidate file in the worktree,
    /// run the gate **once** with the full file list as argv (the deployment's validator takes a
    /// file list — one 38–90 s run per batch, not per page), restore the prior worktree state
    /// either way, and refuse **the whole batch** on a nonzero exit.
    fn gate_check_batch(&self, files: &[(&str, &str)]) -> Result<(), WikiError> {
        let Some(cmd) = &self.cfg.validate_cmd else { return Ok(()) };
        let (program, args) = cmd.split_first().ok_or_else(|| {
            WikiError::Io(io::Error::new(io::ErrorKind::InvalidInput, "validate_cmd must not be empty"))
        })?;
        // Place candidates, remembering each file's prior state for the restore.
        let mut priors: Vec<(PathBuf, Option<Vec<u8>>)> = Vec::with_capacity(files.len());
        for (rel, candidate) in files {
            let dst = self.cfg.dir.join(rel);
            priors.push((dst.clone(), std::fs::read(&dst).ok())); // None = did not exist before
            if let Some(parent) = dst.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&dst, candidate)?;
        }
        let run = Command::new(program)
            .args(args)
            .args(files.iter().map(|(rel, _)| *rel))
            .current_dir(&self.cfg.dir)
            .output();
        // Restore before judging the outcome, so every exit path leaves the worktree as found.
        for (dst, prior) in &priors {
            match prior {
                Some(bytes) => std::fs::write(dst, bytes)?,
                None        => { let _ = std::fs::remove_file(dst); }
            }
        }
        let out = run.map_err(io_ctx("spawning the write gate"))?;
        if out.status.success() {
            return Ok(());
        }
        let findings = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout).trim(),
            String::from_utf8_lossy(&out.stderr).trim(),
        );
        Err(WikiError::gate_refusal(if findings.is_empty() {
            format!("gate {program:?} exited {:?} with no output", out.status.code())
        } else {
            findings
        }))
    }
}

fn io_ctx(what: &'static str) -> impl Fn(io::Error) -> io::Error {
    move |e| io::Error::new(e.kind(), format!("{what}: {e}"))
}

// ── the trait ───────────────────────────────────────────────────────────────────

impl WikiStore for GitStore {
    fn location(&self) -> String {
        format!("{} @{}:{}", self.cfg.dir.to_string_lossy(), self.cfg.branch, self.cfg.subdir)
    }

    fn read(&self, page: &str) -> Result<Option<Page>, WikiError> {
        let Some(pf) = self.load(page)? else { return Ok(None) };
        let Some(manifest) = pf.manifest else { return Ok(None) }; // orphan blocks alone ≠ a page
        let by_id: BTreeMap<&str, &Section> = pf.blocks.iter().map(|s| (&*s.id, s)).collect();
        let sections = manifest
            .order
            .iter()
            .filter_map(|id| by_id.get(&**id).map(|s| (*s).clone())) // a missing referent is skipped, not an error
            .collect();
        Ok(Some(Page { path: page.to_string(), attributes: manifest.attributes, sections }))
    }

    fn read_versioned(&self, page: &str) -> Result<Option<VersionedPage>, WikiError> {
        let Some(pf) = self.load(page)? else { return Ok(None) };
        let Some(manifest) = pf.manifest else { return Ok(None) };
        let mut sections = BTreeMap::new();
        for s in &pf.blocks {
            sections.insert(s.id.clone(), (section_token(s)?, s.clone()));
        }
        Ok(Some(VersionedPage {
            manifest_version: manifest_token(&manifest)?,
            order: manifest.order,
            attributes: manifest.attributes,
            sections,
        }))
    }

    fn query(&self, predicate: &Predicate) -> Result<Vec<SectionRef>, WikiError> {
        // P6.2: one head resolve + one ls-tree + one blob round-trip per page — no per-page spawns.
        let Some(head) = self.head()? else { return Ok(Vec::new()) };
        let mut hits = Vec::new();
        for (page, rel) in self.page_files_at(&head)? {
            let Some((_, pf)) = self.load_at(&head, &rel)? else { continue };
            let Some(manifest) = pf.manifest else { continue }; // orphan-only files are not pages
            let by_id: BTreeMap<&str, &Section> = pf.blocks.iter().map(|s| (&*s.id, s)).collect();
            for id in &manifest.order {
                // Manifest-referenced sections only, in order — identical semantics to `read`.
                let Some(s) = by_id.get(&**id) else { continue };
                if predicate.matches(&s.attributes) {
                    hits.push(SectionRef {
                        page:       page.clone(),
                        id:         s.id.clone(),
                        heading:    s.heading.clone(),
                        attributes: s.attributes.clone(),
                    });
                }
            }
        }
        Ok(hits)
    }

    fn write_section(&self, page: &str, section: &Section, expected: Option<u64>) -> Result<u64, WikiError> {
        let msg = self.message("write", page);
        self.write_with(page, &msg, |pf| {
            let cur = pf.blocks.iter().position(|b| b.id == section.id);
            let cur_token = match cur {
                Some(i) => Some(section_token(&pf.blocks[i])?),
                None    => None,
            };
            match (expected, cur_token) {
                (None, Some(_)) => return Err(WikiError::Conflict), // create over an existing section
                (Some(_), None) => return Err(WikiError::Conflict), // edit of a vanished section
                (Some(e), Some(t)) if e != t => return Err(WikiError::Conflict), // content moved
                _ => {}
            }
            match cur {
                Some(i) => pf.blocks[i] = section.clone(),
                None    => pf.blocks.push(section.clone()), // an orphan until a manifest references it
            }
            section_token(section)
        })
    }

    fn update_manifest(
        &self, page: &str, order: &[SectionId], attributes: &BTreeMap<String, String>, expected: Option<u64>,
    ) -> Result<u64, WikiError> {
        let msg = self.message("manifest", page);
        let next = Manifest { order: order.to_vec(), attributes: attributes.clone() };
        let token = manifest_token(&next)?;
        self.write_with(page, &msg, |pf| {
            let cur_token = match &pf.manifest {
                Some(m) => Some(manifest_token(m)?),
                None    => None,
            };
            match (expected, cur_token) {
                (None, Some(_)) => return Err(WikiError::Conflict),
                (Some(_), None) => return Err(WikiError::Conflict),
                (Some(e), Some(t)) if e != t => return Err(WikiError::Conflict),
                _ => {}
            }
            pf.manifest = Some(next.clone());
            Ok(token)
        })
    }

    fn write_page(
        &self, page: &str, sections: &[Section], attributes: &BTreeMap<String, String>,
    ) -> Result<(), WikiError> {
        let msg = self.message("replace", page);
        self.write_with(page, &msg, |pf| {
            pf.manifest = Some(Manifest {
                order:      sections.iter().map(|s| s.id.clone()).collect(),
                attributes: attributes.clone(),
            });
            pf.blocks = sections.to_vec(); // full replace drops unreferenced blocks
            Ok(0)
        })?;
        Ok(())
    }

    fn list_pages(&self) -> Result<Vec<String>, WikiError> {
        // P6.2: one head resolve + one ls-tree; manifest presence via the persistent read child.
        let Some(head) = self.head()? else { return Ok(Vec::new()) };
        let mut pages = Vec::new();
        for (page, rel) in self.page_files_at(&head)? {
            // A page "exists" only once it has a manifest (FsStore parity: orphan-only files are
            // invisible here too).
            if self.load_at(&head, &rel)?.is_some_and(|(_, pf)| pf.manifest.is_some()) {
                pages.push(page);
            }
        }
        pages.sort();
        Ok(pages)
    }

    /// P6.1 — the batch lands as **one commit** (the deployment's per-meeting boundary commit),
    /// the write gate runs **once** over the full batch, and a gate refusal is **whole-batch
    /// atomic**: nothing commits, so the repository only ever holds whole batches. An idempotent
    /// re-apply (identical tree) records nothing.
    fn write_pages(&self, pages: &[PageWrite], label: &str) -> Result<(), WikiError> {
        if pages.is_empty() {
            return Ok(());
        }
        // Resolve + render everything first — any content error refuses the batch before any
        // side effect (batch atomicity starts at validation, not at the commit).
        let mut files: Vec<(String, String)> = Vec::with_capacity(pages.len());
        for p in pages {
            let rel = self.rel_path(&p.path)?;
            let pf = PageFile {
                manifest: Some(Manifest {
                    order:      p.sections.iter().map(|s| s.id.clone()).collect(),
                    attributes: p.attributes.clone(),
                }),
                blocks: p.sections.to_vec(),
            };
            files.push((rel, render(&pf)?));
        }
        let message = format!("{}: batch({label}) — {} page(s)", self.cfg.message_prefix, files.len());
        let refs: Vec<(&str, &str)> = files.iter().map(|(r, c)| (r.as_str(), c.as_str())).collect();
        let _guard = self.write_lock.lock().unwrap_or_else(|e| e.into_inner());
        for attempt in 0..32 {
            if attempt > 0 {
                self.backoff(attempt);
            }
            let head = self.head()?;
            if self.cfg.validate_cmd.is_some() {
                self.gate_check_batch(&refs)?; // one gate run per batch; refusal refuses it all
            }
            match self.commit_files(head.as_deref(), &refs, &message)? {
                CommitOutcome::Committed | CommitOutcome::NoChange => return Ok(()),
                CommitOutcome::RefMoved => continue, // rebuilt on the new head next iteration
            }
        }
        Err(WikiError::Conflict)
    }

    /// P6.3 — the promotion step: fetch the shared remote and **adopt its head as local truth**.
    /// The reset-to-origin decision, recorded in the hardening plan: a promoted curator's clone
    /// catches up before serving; a local un-pushed tail (≤ one round on the dead curator) is
    /// knowingly discarded and re-lands via resubmission/re-drain (at-least-once + idempotent
    /// apply). An **empty** remote (no branch yet) is a valid fresh start, not an error.
    fn refresh(&self) -> Result<(), WikiError> {
        if self.cfg.remote.is_none() {
            return Ok(()); // local-only store: the local head already is the truth
        }
        let _guard = self.write_lock.lock().unwrap_or_else(|e| e.into_inner());
        let refname = self.refname();
        let ls = self.git_ok(&["ls-remote", "origin", &refname], None)?;
        let remote_sha = String::from_utf8_lossy(&ls).split_whitespace().next().unwrap_or("").to_string();
        if remote_sha.is_empty() {
            return Ok(()); // the remote has no branch yet — nothing to adopt
        }
        self.git_ok(&["fetch", "-q", "origin", &self.cfg.branch], None)?;
        self.git_ok(&["update-ref", &refname, &remote_sha], None)?;
        // Best-effort worktree sync (human readers + the validator's surrounding context); truth
        // is the committed head either way.
        let scope = if self.cfg.subdir.is_empty() { ".".to_string() } else { self.cfg.subdir.clone() };
        let _ = self.git_raw(&["restore", "--source", &remote_sha, "--worktree", "--", &scope], None);
        Ok(())
    }

    /// P6.3/P6.4 — make local commits visible to other nodes' clones: push the branch, with a
    /// **worktree-free subtree splice** retry on a non-fast-forward: their head's tree + this
    /// store's scope files (from our head), a two-parent commit, CAS our ref, push again.
    ///
    /// Why a splice and not a merge: the P6.4 cold-start measurement **falsified** the first cut
    /// (`merge-tree --write-tree`) — ten councils bootstrapping one empty origin create *unrelated
    /// root commits*, and a merge-base merge cannot exist. The splice needs no ancestor and is
    /// merge-*correct* for scoped stores: my `subdir` is mine alone (the topology rule), so
    /// "their tree with my subtree's current files spliced in" IS the merge. Caveat (documented):
    /// a clone hosting several groups' stores publishes the whole local branch's *history* even
    /// though each store splices only its own scope's tree — clone-per-group is the deployed shape.
    /// After a successful push the **divergence tripwire** checks `ls-remote` against the pushed
    /// head — counted and warned, never fixed.
    fn publish(&self) -> Result<(), WikiError> {
        if self.cfg.remote.is_none() {
            return Ok(());
        }
        let _guard = self.write_lock.lock().unwrap_or_else(|e| e.into_inner());
        let Some(mut local) = self.head()? else { return Ok(()) }; // nothing committed yet
        let refname = self.refname();
        let refspec = format!("{}:{}", self.cfg.branch, self.cfg.branch);
        for attempt in 0..8 {
            if attempt > 0 {
                self.backoff(attempt); // P6.4: concurrent publishers queue, not fail
            }
            let (_, pushed) = self.git_raw(&["push", "-q", "origin", &refspec], None)?;
            if pushed {
                let ls = self.git_ok(&["ls-remote", "origin", &refname], None)?;
                let remote_sha =
                    String::from_utf8_lossy(&ls).split_whitespace().next().unwrap_or("").to_string();
                if remote_sha != local {
                    self.push_divergences.fetch_add(1, Ordering::Relaxed);
                    tracing::warn!(local, remote = remote_sha,
                        "wiki git-store: remote head diverged after push (tripwire)");
                }
                return Ok(());
            }
            // Non-fast-forward: splice our scope onto their head — no worktree, no index of the
            // caller's, and no merge base required (unrelated cold-start roots splice fine).
            self.git_ok(&["fetch", "-q", "origin", &self.cfg.branch], None)?;
            let theirs =
                String::from_utf8_lossy(&self.git_ok(&["rev-parse", "FETCH_HEAD"], None)?).trim().to_string();
            if theirs == local {
                continue; // the remote already holds our head; the push failure was transient
            }
            let idx = self.cfg.dir.join(".git").join(format!(
                ".mycelium-index.{}.{}",
                std::process::id(),
                TMP_UNIQUE.fetch_add(1, Ordering::Relaxed)
            ));
            let idx_s = idx.to_string_lossy().into_owned();
            let index_env: [(&str, &str); 1] = [("GIT_INDEX_FILE", &idx_s)];
            let splice = (|| -> Result<String, WikiError> {
                // Their head's tree is the base of the private index…
                match self.git_env(&["read-tree", &theirs], None, &index_env)? {
                    (_, true)  => {}
                    (_, false) => return Err(WikiError::Io(io::Error::other("publish: read-tree failed"))),
                }
                // …and our scope's current files splice over it (`ls-tree -r -z` output is a valid
                // `update-index -z --index-info` input).
                let mut ls_args = vec!["ls-tree", "-r", "-z", &local];
                let scope;
                if !self.cfg.subdir.is_empty() {
                    scope = self.cfg.subdir.clone();
                    ls_args.push("--");
                    ls_args.push(&scope);
                }
                let listing = self.git_ok(&ls_args, None)?;
                match self.git_env(&["update-index", "-z", "--index-info"], Some(&listing), &index_env)? {
                    (_, true)  => {}
                    (_, false) => return Err(WikiError::Io(io::Error::other("publish: index splice failed"))),
                }
                match self.git_env(&["write-tree"], None, &index_env)? {
                    (out, true) => Ok(String::from_utf8_lossy(&out).trim().to_string()),
                    (_, false)  => Err(WikiError::Io(io::Error::other("publish: write-tree failed"))),
                }
            })();
            let _ = std::fs::remove_file(&idx);
            let tree = splice?;
            let ident: [(&str, &str); 4] = [
                ("GIT_AUTHOR_NAME", &self.cfg.author_name),
                ("GIT_AUTHOR_EMAIL", &self.cfg.author_email),
                ("GIT_COMMITTER_NAME", &self.cfg.author_name),
                ("GIT_COMMITTER_EMAIL", &self.cfg.author_email),
            ];
            let msg = format!("{}: merge concurrent writers", self.cfg.message_prefix);
            let commit = match self.git_env(
                &["commit-tree", &tree, "-p", &local, "-p", &theirs, "-m", &msg], None, &ident,
            )? {
                (out, true) => String::from_utf8_lossy(&out).trim().to_string(),
                (_, false)  => return Err(WikiError::Io(io::Error::other("publish: commit-tree failed"))),
            };
            let (_, ok) = self.git_raw(&["update-ref", &refname, &commit, &local], None)?;
            local = if ok { commit } else { self.head()?.unwrap_or(commit) };
        }
        Err(WikiError::Io(io::Error::other("publish: persistent contention pushing to the remote")))
    }
}
