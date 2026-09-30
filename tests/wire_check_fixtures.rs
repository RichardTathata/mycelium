//! The wire-check fixtures (`docs/plans/design-time-tooling.md` W2's exit gate), run through the
//! library rather than the binary so the test needs no `cli` feature: the co-op deployment checks
//! green with its artifact library and reports exactly the provisioning it relies on; the unwired
//! deployment exits 1 naming its ghost; the co-op JSON is a golden file, so a change to the
//! document's shape is a visible diff. Revision is stripped before comparing — it names a commit.

use std::path::Path;

use mycelium::wire_check::{check, ArtifactDescription, CheckOptions, Report, Severity, Unit};
use mycelium::NodeCapabilityConfig;

fn load_units(dir: &str) -> Vec<Unit> {
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{dir}: {e}"))
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "toml"))
        .collect();
    files.sort();
    files
        .into_iter()
        .map(|p| Unit {
            name:   p.file_stem().unwrap().to_string_lossy().into_owned(),
            config: NodeCapabilityConfig::load_from_file(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display())),
        })
        .collect()
}

fn load_artifacts(dir: &str) -> Vec<(String, ArtifactDescription)> {
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "toml"))
        .collect();
    files.sort();
    files
        .into_iter()
        .map(|p| {
            let text = std::fs::read_to_string(&p).unwrap();
            (
                p.file_stem().unwrap().to_string_lossy().into_owned(),
                ArtifactDescription::from_toml_str(&text).unwrap_or_else(|e| panic!("{}: {e}", p.display())),
            )
        })
        .collect()
}

fn coop() -> Report {
    check(
        &load_units("tests/fixtures/units/coop"),
        &load_artifacts("tests/fixtures/units/artifacts"),
        &CheckOptions { strict_deployed: false, ..Default::default() },
    )
}

#[test]
fn the_coop_deployment_checks_green_and_names_the_one_provisioning_it_relies_on() {
    let r = coop();
    assert_eq!(r.exit_code(), 0, "{}", r.render_text());
    let kinds: Vec<(&str, Severity)> = r.findings.iter().map(|f| (f.kind.as_str(), f.severity)).collect();
    assert!(
        kinds.contains(&("would bind by provisioning", Severity::Warning)),
        "route/optimize is installed, not deployed: {}",
        r.render_text()
    );
    assert!(kinds.iter().all(|(_, s)| *s == Severity::Warning), "{}", r.render_text());
    let optimize = r.edges.iter().find(|e| e.ns == "route" && e.name == "optimize").expect("the worker's requirement");
    assert!(
        matches!(&optimize.providers[0], mycelium::wire_check::Provider::Artifact { hosts, .. } if hosts == &["depot-a".to_string(), "depot-b".to_string()]),
        "{:?}",
        optimize.providers
    );
    // The same directory is an error under strict mode: it is wired only by what could be installed.
    let strict = check(
        &load_units("tests/fixtures/units/coop"),
        &load_artifacts("tests/fixtures/units/artifacts"),
        &CheckOptions { strict_deployed: true, ..Default::default() },
    );
    assert_eq!(strict.exit_code(), 1);
}

#[test]
fn the_coop_deployment_without_its_library_could_not_bind_the_optimizer() {
    let r = check(&load_units("tests/fixtures/units/coop"), &[], &CheckOptions::default());
    assert_eq!(r.exit_code(), 1);
    let text = r.render_text();
    assert!(text.contains("unwired requirement (worker): requirement route/optimize"), "{text}");
    assert!(text.contains("presence unhostable (depot-a)"), "{text}");
}

#[test]
fn the_unwired_deployment_exits_one_naming_its_ghost() {
    let r = check(&load_units("tests/fixtures/units/unwired"), &[], &CheckOptions::default());
    assert_eq!(r.exit_code(), 1);
    let text = r.render_text();
    // The unit is named by its file stem; its principal ("lonely") is what a runtime record would carry.
    assert!(text.contains("unwired requirement (needs-a-ghost): requirement ghost/town"), "{text}");
    assert_eq!(r.units[0].principal.as_deref(), Some("lonely"));
    assert!(text.contains("could not bind"), "{text}");
}

/// The golden document. Regenerate deliberately, in the open, when the shape changes:
/// `UPDATE_GOLDEN=1 cargo test --test wire_check_fixtures`.
#[test]
fn the_coop_json_is_the_golden_document() {
    let golden = Path::new("tests/fixtures/units/coop.expected.json");
    let mut r = coop();
    r.revision = None;
    let json = r.render_json() + "\n";
    if std::env::var("UPDATE_GOLDEN").is_ok() {
        std::fs::write(golden, &json).unwrap();
    }
    let expected = std::fs::read_to_string(golden)
        .unwrap_or_else(|e| panic!("{}: {e} — run once with UPDATE_GOLDEN=1", golden.display()));
    assert_eq!(json, expected, "the declaration document's shape or content changed; if intended, UPDATE_GOLDEN=1");
    let parsed: serde_json::Value = serde_json::from_str(&expected).unwrap();
    assert_eq!(parsed["schema"], mycelium::wire_check::DECLARATION_SCHEMA);
}

/// W3: with the schema directory beside the units, a schema nobody defined is named, and a provider
/// on `v1` against a requirement on `v2` is the rollout-window case with both ids on the line.
#[test]
fn the_schema_window_fixture_names_the_window_and_the_unknown_schema() {
    let mut known = std::collections::BTreeSet::new();
    fn walk(root: &Path, dir: &Path, out: &mut std::collections::BTreeSet<String>) {
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                walk(root, &p, out);
            } else if p.extension().is_some_and(|x| x == "json") {
                let rel = p.strip_prefix(root).unwrap().with_extension("");
                out.insert(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    walk(Path::new("tests/fixtures/units/schemas"), Path::new("tests/fixtures/units/schemas"), &mut known);
    assert_eq!(known.len(), 2, "{known:?}");
    let r = check(
        &load_units("tests/fixtures/units/schema-window"),
        &[],
        &CheckOptions { known_schemas: Some(known), ..Default::default() },
    );
    assert_eq!(r.exit_code(), 1);
    let text = r.render_text();
    assert!(text.contains("unknown schema (consumer): requirement plan/route names schema \"plan/route/v9\""), "{text}");
    assert!(text.contains("schema-only mismatch (consumer): requirement llm/inference wants schema \"llm/inference/v2\"; offers match except for schema (llm/inference/v1)"), "{text}");
}
