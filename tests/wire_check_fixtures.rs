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

/// W4: a governed requirer whose only admitting rule requires a mandate scope nobody declares is
/// `unauthorisable edge`; the co-op deployment, whose mandate and rules use the gateway's own
/// operation form, stays green.
#[test]
fn the_unauthorised_fixture_names_the_edge_no_declaration_could_authorise() {
    let r = check(&load_units("tests/fixtures/units/unauthorised"), &[], &CheckOptions::default());
    assert_eq!(r.exit_code(), 1);
    let text = r.render_text();
    assert!(text.contains("unauthorisable edge (worker): requirement route/optimize"), "{text}");
    assert!(text.contains("requires mandate scope \"depot\""), "{text}");
    // Off, the same directory is green: the overlay is what found it.
    let r = check(&load_units("tests/fixtures/units/unauthorised"), &[], &CheckOptions { authority: false, ..Default::default() });
    assert_eq!(r.exit_code(), 0, "{}", r.render_text());
}

/// **The co-op's hosted edge is wired by a rule its provider reads** (2026-11-18; the false green of
/// `docs/operations/what-is-proven.md#wire-check-does-not-know-the-hosted-capability-shape`). `route/optimize` is
/// provided only by a hosted component, which an enforcing provider decides as `cap.invoke` on
/// `cap:route/optimize@{node}` for the calling node. The fixture as it stood until this change authorised that
/// edge with `skill.invoke` on `skill:route/optimize` for the unit principal `worker` (`auditor.toml:11`, with
/// `worker.toml`'s mandate enumerating it) — a rule the provider never consults — and the check read it green. It
/// is now `unauthorisable edge`; the fixture's rule moved to the units that host the component, as the provider
/// decides it, and the co-op is green with no word about that edge. The provider's own decision on both rules is
/// `the_coop_fixtures_hosted_edge_is_decided_at_the_provider_as_wire_check_reads_it` (`src/agent/declared_authority_tests.rs`).
/// Seen failing first: on the code before this change the old declaration below exited 0 with no `unauthorisable
/// edge` — the false green itself.
#[test]
fn the_coops_hosted_edge_is_wired_by_a_rule_its_provider_reads() {
    // The fixture as it stood: no rule in the depots, the skill rule in the auditor, the mandate enumerating it.
    let mut old = load_units("tests/fixtures/units/coop");
    for u in old.iter_mut() {
        u.config.rules.retain(|r| r.resource != "cap:route/optimize");
        if u.name == "auditor" {
            let mut rule = mycelium::RuleDecl::new("worker", "skill.invoke", "skill:route/optimize");
            rule.requires_mandate = Some("routing".into());
            u.config.rules.push(rule);
        }
        if u.name == "worker" {
            u.config.mandates[0].operations.push("skill.invoke:skill:route/optimize".into());
        }
    }
    let r = check(&old, &load_artifacts("tests/fixtures/units/artifacts"), &CheckOptions::default());
    assert_eq!(r.exit_code(), 1, "FALSE GREEN: the provider refuses this edge and the check passed it:\n{}", r.render_text());
    let text = r.render_text();
    assert!(text.contains("unauthorisable edge (worker): requirement route/optimize"), "{text}");
    assert!(text.contains("cap.invoke on cap:route/optimize"), "{text}");
    // The other edge of the same requirer — a deployed capability, read as a skill — is still admitted.
    assert!(!text.contains("requirement depot/intake: no declared rule"), "{text}");

    // The fixture as it is: green, the edge neither refused nor warned about, and carrying the hosted shape.
    let r = coop();
    assert_eq!(r.exit_code(), 0, "{}", r.render_text());
    assert!(r.findings.iter().all(|f| f.kind != "unauthorisable edge" && f.kind != "node-decided edge"), "{}", r.render_text());
    let optimize = r.edges.iter().find(|e| e.ns == "route" && e.name == "optimize").expect("the worker's requirement");
    assert!(optimize.operations.iter().any(|s| s.operation == "cap.invoke" && s.resource == "cap:route/optimize"), "{:?}", optimize.operations);
    let declaring: Vec<String> =
        load_units("tests/fixtures/units/coop").into_iter().filter(|u| u.config.rules.iter().any(|r| r.operation == "cap.invoke")).map(|u| u.name).collect();
    assert_eq!(declaring, ["depot-a", "depot-b"], "the rule is declared by each unit that could host the component: `[authority]` loads a node's own unit's rules");
}

#[test]
fn a_proposed_artifact_would_bind_after_acceptance_not_by_provisioning() {
    // D20 (plan F2): the same co-op deployment over a library whose optimizer is *proposed* is
    // still wired — by an artifact that loads only into a shadow lane until a reviewer accepts it —
    // and the checker says so by name, in the text and in the document.
    let r = check(
        &load_units("tests/fixtures/units/coop"),
        &load_artifacts("tests/fixtures/units/artifacts-proposed"),
        &CheckOptions { strict_deployed: false, ..Default::default() },
    );
    assert_eq!(r.exit_code(), 0, "{}", r.render_text());
    let kinds: Vec<&str> = r.findings.iter().map(|f| f.kind.as_str()).collect();
    assert!(kinds.contains(&"would bind after acceptance"), "{}", r.render_text());
    assert!(!kinds.contains(&"would bind by provisioning"), "a proposal is not a provisioning: {}", r.render_text());
    let optimize = r.edges.iter().find(|e| e.ns == "route" && e.name == "optimize").expect("the worker's requirement");
    assert!(
        matches!(&optimize.providers[0], mycelium::wire_check::Provider::Artifact { proposed: true, .. }),
        "{:?}",
        optimize.providers
    );
    assert!(r.render_text().contains("(proposed)"), "{}", r.render_text());
    let json = serde_json::to_string(&r).unwrap();
    assert!(json.contains("\"proposed\":true"), "{json}");
    // And the accepted library's document does not carry the field at all (the golden is unchanged).
    let accepted = serde_json::to_string(&coop()).unwrap();
    assert!(!accepted.contains("proposed"), "{accepted}");
}
