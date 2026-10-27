//! `mycelium init` — a scaffold that CI builds and runs (docs/plans/adopter-surface.md, X2).
//!
//! The templates live under `templates/` in the repository, each a project of its own that CI builds and runs;
//! this module embeds every file of every template at compile time, so an installed binary carries them and the
//! directory and the binary cannot differ. `init` writes the embedded copy, with [`TAG_PLACEHOLDER`] replaced by
//! the tag of the release this binary came from, so a scaffold depends on the `mycelium` it was made by.

use std::path::{Path, PathBuf};

/// One template: its name, a one-line description, and every file as `(relative path, contents)`.
pub struct Template {
    pub name: &'static str,
    pub about: &'static str,
    pub files: &'static [(&'static str, &'static str)],
}

/// Replaced in every written file by [`tag`].
pub const TAG_PLACEHOLDER: &str = "{{MYCELIUM_TAG}}";

/// The tag of the release this binary came from: `v` and the crate's version.
pub fn tag() -> String {
    format!("v{}", env!("CARGO_PKG_VERSION"))
}

macro_rules! file {
    ($template:literal, $path:literal) => {
        ($path, include_str!(concat!("../templates/", $template, "/", $path)))
    };
}

/// Every template this binary can write. A file on disk under `templates/` that is not listed here fails
/// `every_template_file_on_disk_is_embedded`, so the list cannot fall behind the directory.
pub const TEMPLATES: &[Template] = &[
    Template {
        name: "governed-call",
        about: "Rust: two in-process nodes; a member's direct call to a provider's tool, permitted and refused by a \
                policy enforced at the provider and recorded in an evidence journal; a prompt skill from a file",
        files: &[
            file!("governed-call", "Cargo.toml"),
            file!("governed-call", ".gitignore"),
            file!("governed-call", "README.md"),
            file!("governed-call", "node.toml"),
            file!("governed-call", "units/purchaser.toml"),
            file!("governed-call", "skills/demo/greeting.md"),
            file!("governed-call", "src/main.rs"),
            file!("governed-call", "src/policy.rs"),
        ],
    },
    Template {
        name: "governed-call-py",
        about: "Python: the published node under a named-token table; an SDK agent declares a unit and writes a key, \
                and a second agent is refused by the gateway's own scope rule",
        files: &[
            file!("governed-call-py", ".gitignore"),
            file!("governed-call-py", "README.md"),
            file!("governed-call-py", "node.toml"),
            file!("governed-call-py", "units/purchaser.toml"),
            file!("governed-call-py", "main.py"),
        ],
    },
    Template {
        name: "two-domains",
        about: "Rust: two fleets of two nodes, each its own domain under secure-multi-domain — certificates issued \
                off-node, named tokens, egress lists naming the partner, a pinned key each way — calling each other \
                across one edge by named local callers, refused under a wrong pin, a rewritten body and a full slot count",
        files: &[
            file!("two-domains", "Cargo.toml"),
            file!("two-domains", ".gitignore"),
            file!("two-domains", "README.md"),
            file!("two-domains", "federation.toml"),
            file!("two-domains", "fleet.sh"),
            file!("two-domains", "nodes/foodbank-gw.toml"),
            file!("two-domains", "nodes/foodbank-1.toml"),
            file!("two-domains", "nodes/kitchen-gw.toml"),
            file!("two-domains", "nodes/kitchen-1.toml"),
            file!("two-domains", "src/main.rs"),
            file!("two-domains", "src/fleet.rs"),
            file!("two-domains", "src/authority.rs"),
        ],
    },
];

/// The template named `name`.
pub fn template(name: &str) -> Option<&'static Template> {
    TEMPLATES.iter().find(|t| t.name == name)
}

/// Write `template` under `dest`, which must not exist or must be an empty directory — a scaffold never
/// overwrites. Returns every path written. The placeholder is replaced in every file.
pub fn write(template: &Template, dest: &Path) -> Result<Vec<PathBuf>, String> {
    // An empty path is the current directory, which is never empty while this runs (#666's review, D2); a
    // destination that *is* the current directory is refused by the same rule, by comparing canonical paths.
    if dest.as_os_str().is_empty() {
        return Err("init needs a directory to write into (an empty path would mean the current directory)".into());
    }
    if let (Ok(here), Ok(there)) = (std::env::current_dir(), std::fs::canonicalize(dest))
        && here == there
    {
        return Err(format!("{} is the current directory; init writes only into a new or empty directory", dest.display()));
    }
    match std::fs::read_dir(dest) {
        Ok(mut entries) => {
            if entries.next().is_some() {
                return Err(format!("{} exists and is not empty; init writes only into a new or empty directory", dest.display()));
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("{}: {e}", dest.display())),
    }
    let tag = tag();
    let mut written = Vec::with_capacity(template.files.len());
    for (rel, contents) in template.files {
        let path = dest.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        std::fs::write(&path, contents.replace(TAG_PLACEHOLDER, &tag)).map_err(|e| format!("{}: {e}", path.display()))?;
        written.push(path);
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// The template's files as checked in: what its own `.gitignore` names (a run's `target/`, `data/`) and
    /// the desktop's droppings (`.DS_Store`) are not template files, so a `cargo run` inside the directory does
    /// not fail this test (#666's review, D8).
    fn on_disk(template: &str) -> BTreeSet<(String, String)> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("templates").join(template);
        let ignored: Vec<String> = std::fs::read_to_string(root.join(".gitignore"))
            .unwrap_or_default()
            .lines()
            .map(|l| l.trim().trim_start_matches('/').trim_end_matches('/').to_string())
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .collect();
        let mut out = BTreeSet::new();
        let mut stack = vec![root.clone()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).expect("the template directory") {
                let path = entry.expect("entry").path();
                let rel = path.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/");
                let top = rel.split('/').next().unwrap_or("").to_string();
                if ignored.contains(&top) || top == ".DS_Store" {
                    continue;
                }
                if path.is_dir() {
                    stack.push(path);
                } else {
                    out.insert((rel, std::fs::read_to_string(&path).expect("a template file is UTF-8 text")));
                }
            }
        }
        out
    }

    /// The directory is the source of truth and the binary embeds it: every file on disk is embedded with the
    /// same bytes, and nothing is embedded that is not on disk.
    #[test]
    fn every_template_file_on_disk_is_embedded() {
        for t in TEMPLATES {
            let embedded: BTreeSet<(String, String)> = t.files.iter().map(|(p, c)| (p.to_string(), c.to_string())).collect();
            let disk = on_disk(t.name);
            let missing: Vec<&String> = disk.iter().map(|(p, _)| p).filter(|p| !embedded.iter().any(|(q, _)| q == *p)).collect();
            assert!(missing.is_empty(), "{}: on disk but not embedded in src/init.rs: {missing:?}", t.name);
            assert_eq!(embedded, disk, "{}: the embedded files differ from the directory", t.name);
        }
    }

    #[test]
    fn init_writes_the_binarys_own_tag_and_no_placeholder() {
        let dest = std::env::temp_dir().join(format!("mycelium-init-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dest);
        let written = write(template("governed-call").unwrap(), &dest).expect("writes into a new directory");
        assert_eq!(written.len(), template("governed-call").unwrap().files.len());
        let manifest = std::fs::read_to_string(dest.join("Cargo.toml")).unwrap();
        assert!(manifest.contains(&format!("tag = \"{}\"", tag())), "{manifest}");
        for path in &written {
            assert!(!std::fs::read_to_string(path).unwrap().contains(TAG_PLACEHOLDER), "{}", path.display());
        }
        let _ = std::fs::remove_dir_all(&dest);
    }

    /// `init ""` and `init .` would write into the current directory, over whatever is there (#666's review, D2).
    #[test]
    fn init_refuses_an_empty_path_and_the_current_directory() {
        let t = template("governed-call-py").unwrap();
        let err = write(t, Path::new("")).expect_err("an empty path is refused");
        assert!(err.contains("current directory"), "{err}");
        let err = write(t, Path::new(".")).expect_err("the current directory is refused");
        assert!(err.contains("current directory"), "{err}");
        assert!(!Path::new("main.py").exists(), "nothing was written into the current directory");
    }

    #[test]
    fn init_refuses_a_directory_that_is_not_empty() {
        let dest = std::env::temp_dir().join(format!("mycelium-init-full-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dest);
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(dest.join("keep.txt"), b"mine").unwrap();
        let err = write(template("governed-call-py").unwrap(), &dest).expect_err("refused");
        assert!(err.contains("not empty"), "{err}");
        assert_eq!(std::fs::read_dir(&dest).unwrap().count(), 1, "nothing was written");
        let _ = std::fs::remove_dir_all(&dest);
    }

    /// The plan's size bar: a scaffold a reader takes in at one sitting.
    #[test]
    fn the_programs_stay_short() {
        let lines = |t: &str, f: &str| template(t).unwrap().files.iter().find(|(p, _)| *p == f).unwrap().1.lines().count();
        assert!(lines("governed-call", "src/main.rs") < 80, "main.rs: {}", lines("governed-call", "src/main.rs"));
        assert!(lines("governed-call-py", "main.py") < 60, "main.py: {}", lines("governed-call-py", "main.py"));
    }
}
