//! W6's public half (`docs/plans/design-time-tooling.md` §9): the declaration document has a
//! pinned schema, `docs/reference/declaration.schema.json` (`mycelium.design/declaration/1`), and
//! both the golden co-op document and a report generated now validate against it — so a change
//! to the document's shape is a red test here before it is a surprise on a consumer's side.

use std::path::Path;

use mycelium::wire_check::{check, ArtifactDescription, CheckOptions, Unit, DECLARATION_SCHEMA};
use mycelium::NodeCapabilityConfig;

fn load_units(dir: &str) -> Vec<Unit> {
    let mut files: Vec<_> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().path()).filter(|p| p.extension().is_some_and(|x| x == "toml")).collect();
    files.sort();
    files
        .into_iter()
        .map(|p| Unit { name: p.file_stem().unwrap().to_string_lossy().into_owned(), config: NodeCapabilityConfig::load_from_file(&p).unwrap() })
        .collect()
}

fn load_artifacts(dir: &str) -> Vec<(String, ArtifactDescription)> {
    let mut files: Vec<_> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().path()).filter(|p| p.extension().is_some_and(|x| x == "toml")).collect();
    files.sort();
    files
        .into_iter()
        .map(|p| (p.file_stem().unwrap().to_string_lossy().into_owned(), ArtifactDescription::from_toml_str(&std::fs::read_to_string(&p).unwrap()).unwrap()))
        .collect()
}

fn validator() -> jsonschema::Validator {
    let text = std::fs::read_to_string(Path::new("docs/reference/declaration.schema.json")).expect("the pinned schema");
    let schema: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(schema["$id"], DECLARATION_SCHEMA, "the file is the pin for the schema the checker names");
    jsonschema::validator_for(&schema).expect("a valid schema")
}

fn errors(v: &jsonschema::Validator, doc: &serde_json::Value) -> Vec<String> {
    v.iter_errors(doc).map(|e| format!("{} at {}", e, e.instance_path())).collect()
}

#[test]
fn the_golden_document_validates_against_the_pinned_schema() {
    let v = validator();
    let golden: serde_json::Value = serde_json::from_str(&std::fs::read_to_string("tests/fixtures/units/coop.expected.json").unwrap()).unwrap();
    let errs = errors(&v, &golden);
    assert!(errs.is_empty(), "{errs:#?}");
}

#[test]
fn a_report_generated_now_validates_and_a_shape_change_would_not() {
    let v = validator();
    for (units, artifacts) in [
        ("tests/fixtures/units/coop", Some("tests/fixtures/units/artifacts")),
        ("tests/fixtures/units/coop", Some("tests/fixtures/units/artifacts-proposed")),
        ("tests/fixtures/units/unwired", None),
        ("tests/fixtures/units/unauthorised", None),
    ] {
        if artifacts.is_some_and(|a| !Path::new(a).is_dir()) {
            continue; // a fixture from a branch not yet merged (the proposed library, F2)
        }
        let r = check(&load_units(units), &artifacts.map(load_artifacts).unwrap_or_default(), &CheckOptions { revision: Some("test".into()), ..Default::default() });
        let doc = serde_json::to_value(&r).unwrap();
        let errs = errors(&v, &doc);
        assert!(errs.is_empty(), "{units}: {errs:#?}");
    }
    // The plant: the schema refuses what it does not know — an unknown top-level key, a provider
    // of an unknown kind, a finding of an unknown severity.
    let mut doc: serde_json::Value = serde_json::from_str(&std::fs::read_to_string("tests/fixtures/units/coop.expected.json").unwrap()).unwrap();
    doc["extra"] = serde_json::json!(1);
    assert!(!errors(&v, &doc).is_empty(), "an unknown top-level key is refused");
    let mut doc: serde_json::Value = serde_json::from_str(&std::fs::read_to_string("tests/fixtures/units/coop.expected.json").unwrap()).unwrap();
    doc["edges"][0]["providers"][0] = serde_json::json!({ "via": "oracle", "unit": "x" });
    assert!(!errors(&v, &doc).is_empty(), "a provider kind the schema does not know is refused");
    let mut doc: serde_json::Value = serde_json::from_str(&std::fs::read_to_string("tests/fixtures/units/coop.expected.json").unwrap()).unwrap();
    doc["findings"][0]["severity"] = serde_json::json!("fatal");
    assert!(!errors(&v, &doc).is_empty(), "an unknown severity is refused");
}
