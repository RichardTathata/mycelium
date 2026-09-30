//! X1's exit gate (`docs/plans/design-time-tooling.md` §16): every example's declaration directory
//! under `examples/units/` checks clean, and deleting a capability from a directory turns it red —
//! precisely: deleting the **last** provider of any advertised `ns/name` (a redundant provider's
//! block is, by design, deletable without a finding: `elastic_intent` runs five rush workers so
//! that four remain). So an advertised capability without a requirer written down cannot hide here.
//! Seen failing first, twice: deleting `catalog`'s `artifact/librarian` left the check green,
//! because nothing had written down that the installer resolves the librarian; and the clean check
//! found `provisioning`'s `done` lane produced and consumed by nobody — the demo's own depth read
//! is the consumer, now a unit.

use std::path::{Path, PathBuf};

use mycelium::wire_check::{check, ArtifactDescription, CheckOptions, Severity, Unit};
use mycelium::NodeCapabilityConfig;

fn tomls(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "toml"))
        .collect();
    files.sort();
    files
}

fn load_units(dir: &Path) -> Vec<Unit> {
    tomls(dir)
        .into_iter()
        .map(|p| {
            let name = p.file_stem().unwrap().to_string_lossy().into_owned();
            let config = NodeCapabilityConfig::load_from_file(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
            Unit { name, config }
        })
        .collect()
}

fn load_artifacts(dir: &Path) -> Vec<(String, ArtifactDescription)> {
    if !dir.is_dir() {
        return Vec::new();
    }
    tomls(dir)
        .into_iter()
        .map(|p| {
            let name = p.file_stem().unwrap().to_string_lossy().into_owned();
            let text = std::fs::read_to_string(&p).unwrap();
            (name, ArtifactDescription::from_toml_str(&text).unwrap_or_else(|e| panic!("{}: {e}", p.display())))
        })
        .collect()
}

fn example_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<_> = std::fs::read_dir("examples/units")
        .expect("examples/units")
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    assert!(dirs.len() >= 15, "the declaring examples each have a directory: {}", dirs.len());
    dirs
}

#[test]
fn every_example_directory_checks_clean() {
    for dir in example_dirs() {
        let r = check(&load_units(&dir), &load_artifacts(&dir.join("artifacts")), &CheckOptions::default());
        assert_eq!(r.exit_code(), 0, "{}:\n{}", dir.display(), r.render_text());
    }
}

#[test]
fn deleting_the_last_provider_of_any_capability_turns_its_directory_red() {
    let mut planted = 0;
    for dir in example_dirs() {
        let units = load_units(&dir);
        let artifacts = load_artifacts(&dir.join("artifacts"));
        let mut advertised: Vec<(String, String)> = units
            .iter()
            .flat_map(|u| u.config.capabilities.iter().map(|c| (c.ns.clone(), c.name.clone())))
            .collect();
        advertised.sort();
        advertised.dedup();
        for (ns, name) in advertised {
            let mut cut = units.clone();
            let mut providers = Vec::new();
            for u in cut.iter_mut() {
                let before = u.config.capabilities.len();
                u.config.capabilities.retain(|c| !(c.ns == ns && c.name == name));
                if u.config.capabilities.len() != before {
                    providers.push(u.name.clone());
                }
            }
            let r = check(&cut, &artifacts, &CheckOptions::default());
            let names_it = r.findings.iter().any(|f| f.severity == Severity::Error && f.message.contains(&format!("{ns}/{name}")));
            assert!(
                names_it,
                "{}: deleting {ns}/{name} (provided by {}) left the check green — the example's requirer is not written down:\n{}",
                dir.display(), providers.join(", "), r.render_text()
            );
            planted += 1;
        }
    }
    assert!(planted > 25, "the plant ran over the advertised capabilities: {planted}");
}
