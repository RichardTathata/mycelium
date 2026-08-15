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
//! Zero dependencies beyond the `git` CLI. No pushing — replication cadence and the remote
//! discipline are the deployment's (Phase-1 scope; see the design record's open questions).

use std::collections::BTreeMap;
use std::io;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::model::{Manifest, Page, Predicate, Section, SectionId, SectionRef, WikiError};
use crate::store::{VersionedPage, WikiStore};

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
}

/// A git-checkout-backed group wiki. See the module docs for the contract.
pub struct GitStore {
    cfg: GitStoreConfig,
    /// Serializes this instance's writes (the read-validate-commit sequence). Internal only, never
    /// composed with another lock; held across local git subprocess I/O by design — the store's
    /// writer is a single curator, and the atomic `update-ref` CAS is the cross-instance backstop.
    write_lock: Mutex<()>,
    tmp_seq: AtomicU64,
}

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
    /// The branch ref moved between our HEAD read and the update — rebuild on the new head and retry.
    RefMoved,
}

impl GitStore {
    /// Open (creating + `git init -b {branch}` if needed) the store.
    pub fn open(cfg: GitStoreConfig) -> Result<Self, WikiError> {
        std::fs::create_dir_all(&cfg.dir)?;
        let me = Self { cfg, write_lock: Mutex::new(()), tmp_seq: AtomicU64::new(0) };
        if !me.cfg.dir.join(".git").exists() {
            me.git_ok(&["init", "-q", "-b", &me.cfg.branch.clone()], None)?;
        }
        Ok(me)
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

    /// The committed page file at `head`, parsed. `Ok(None)` if the file does not exist there.
    fn load_at(&self, head: &str, rel: &str) -> Result<Option<(String, PageFile)>, WikiError> {
        let Some(bytes) = self.git_try(&["show", &format!("{head}:{rel}")])? else { return Ok(None) };
        let text = String::from_utf8(bytes)
            .map_err(|_| bad_content(format!("page file {rel:?} is not UTF-8")))?;
        let pf = parse(&text)?;
        Ok(Some((text, pf)))
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
        // Blob into the object database (content only; nothing touches the user's index).
        let blob = String::from_utf8_lossy(&self.git_ok(&["hash-object", "-w", "--stdin"], Some(content.as_bytes()))?)
            .trim()
            .to_string();
        // A private index: base it on the head tree, splice the blob, write the new tree.
        let idx = self.cfg.dir.join(".git").join(format!(
            ".mycelium-index.{}.{}",
            std::process::id(),
            self.tmp_seq.fetch_add(1, Ordering::Relaxed)
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
            plumb(&["update-index", "--add", "--cacheinfo", &format!("100644,{blob},{rel}")])?;
            let tree = String::from_utf8_lossy(&plumb(&["write-tree"])?).trim().to_string();
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
            // Sync the worktree copy (temp + rename — atomic, never torn) so direct readers and
            // external validators see current files; truth remains the committed blob.
            let dst = self.cfg.dir.join(rel);
            if let Some(parent) = dst.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let tmp = self.cfg.dir.join(format!(
                ".mycelium-wt.{}.{}",
                std::process::id(),
                self.tmp_seq.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::write(&tmp, content)?;
            std::fs::rename(&tmp, &dst)?;
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
        for _ in 0..16 {
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
            match self.commit_file(head.as_deref(), &rel, &content, message)? {
                CommitOutcome::Committed => return Ok(token),
                CommitOutcome::RefMoved  => continue, // rebuilt on the new head next iteration
            }
        }
        Err(WikiError::Conflict) // persistently losing the ref race — report as contention
    }

    fn message(&self, verb: &str, page: &str) -> String {
        format!("{}: {verb} {page}", self.cfg.message_prefix)
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
        let mut hits = Vec::new();
        for page in self.list_pages()? {
            let Some(p) = self.read(&page)? else { continue };
            for s in p.sections {
                if predicate.matches(&s.attributes) {
                    hits.push(SectionRef { page: page.clone(), id: s.id, heading: s.heading, attributes: s.attributes });
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
        let Some(head) = self.head()? else { return Ok(Vec::new()) };
        let mut args = vec!["ls-tree", "-r", "-z", "--name-only", head.as_str()];
        let scope;
        if !self.cfg.subdir.is_empty() {
            scope = self.cfg.subdir.clone();
            args.push("--");
            args.push(&scope);
        }
        let out = self.git_ok(&args, None)?;
        let prefix = if self.cfg.subdir.is_empty() { String::new() } else { format!("{}/", self.cfg.subdir) };
        let mut pages = Vec::new();
        for name in out.split(|&b| b == 0).filter(|s| !s.is_empty()) {
            let name = String::from_utf8_lossy(name);
            let Some(rel) = name.strip_prefix(&prefix) else { continue };
            let Some(page) = rel.strip_suffix(".md") else { continue };
            // A page "exists" only once it has a manifest (FsStore parity: orphan-only files are
            // invisible here too).
            if self.load_at(&head, &name)?.is_some_and(|(_, pf)| pf.manifest.is_some()) {
                pages.push(page.to_string());
            }
        }
        pages.sort();
        Ok(pages)
    }
}
