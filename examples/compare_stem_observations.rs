//! Consumer for first_stem_fleet's exported declaration and observation files.
//! Deliberately scenario-specific: demo/echo has a floor of two WASM providers.
//! The observation shape is this example's adapter, not a public telemetry schema.
use std::{collections::HashSet, path::Path};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = std::env::args()
        .nth(1)
        .ok_or("usage: compare_stem_observations <export-directory>")?;
    let schema: serde_json::Value =
        serde_json::from_str(include_str!("../docs/reference/declaration.schema.json"))?;
    let declared: serde_json::Value =
        serde_json::from_slice(&std::fs::read(Path::new(&dir).join("declared.json"))?)?;
    jsonschema::validator_for(&schema)?
        .validate(&declared)
        .map_err(|e| e.to_string())?;
    let observed: serde_json::Value =
        serde_json::from_slice(&std::fs::read(Path::new(&dir).join("observed.json"))?)?;
    let rows = observed.as_array().ok_or("observation must be an array")?;
    let units = declared["units"].as_array().ok_or("missing units")?;
    let mut nodes = HashSet::new();
    let mut mapped_units = HashSet::new();
    let mut matched = 0;
    let mut unexpected = 0;
    for row in rows {
        let node = row["node"].as_str().ok_or("missing node")?;
        let unit = row["unit"].as_str().ok_or("missing unit")?;
        let capability = row["capability"].as_str().ok_or("missing capability")?;
        // Never count duplicated telemetry as a second provider.
        if !nodes.insert(node) || !mapped_units.insert(unit) {
            return Err("duplicate node or unit in this one-node-per-unit example".into());
        }
        let hostable = units.iter().any(|u| {
            u["name"] == unit
                && u["hosts_kinds"]
                    .as_array()
                    .is_some_and(|k| k.iter().any(|v| v == "wasm-component"))
        });
        let declared_provider = declared["edges"].as_array().is_some_and(|edges| {
            edges.iter().any(|edge| {
                edge["ns"] == "demo"
                    && edge["name"] == "echo"
                    && edge["schema_id"].is_null()
                    && edge["providers"].as_array().is_some_and(|providers| {
                        providers.iter().any(|p| {
                            p["via"] == "artifact"
                                && p["artifact"] == "echo-component"
                                && p["hosts"]
                                    .as_array()
                                    .is_some_and(|hosts| hosts.iter().any(|h| h == unit))
                        })
                    })
            })
        });
        if hostable && declared_provider && capability == "demo/echo" {
            matched += 1;
        } else {
            unexpected += 1;
            println!("UNEXPECTED: {node} mapped to {unit} offering {capability}");
        }
    }
    // The schema exports presence descriptions, not a typed fleet-floor contract.
    // This consumer therefore names its scenario's floor instead of parsing prose.
    let missing = 2usize.saturating_sub(matched);
    let excess = matched.saturating_sub(2);
    println!(
        "schema valid; matched={matched}, missing={missing}, unexpected={unexpected}, excess={excess}"
    );
    println!("Snapshot comparison only; absence is not proof of failure or revocation.");
    if missing + unexpected + excess != 0 {
        return Err("snapshot differs from this scenario's expected providers".into());
    }
    Ok(())
}
