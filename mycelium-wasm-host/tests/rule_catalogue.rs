//! The rule catalogue is generated from the descriptors each crate registers and checked here
//! (plan I1): unique ids, every relation naming a registered rule, typed reasons, a named test that
//! exists in the tree, and the checked-in document current with the code. Regenerate with
//! `UPDATE_RULE_CATALOGUE=1 cargo test -p mycelium-wasm-host --features stem --test rule_catalogue`.
#![cfg(feature = "stem")]

use mycelium::rule::Catalogue;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf()
}

fn source_text() -> String {
    let root = repo_root();
    let mut out = String::new();
    for dir in ["src", "mycelium-core/src", "mycelium-wasm-host/src", "tests", "mycelium-wasm-host/tests"] {
        let mut stack = vec![root.join(dir)];
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else { continue };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() { stack.push(p); } else if p.extension().is_some_and(|x| x == "rs") {
                    out.push_str(&std::fs::read_to_string(&p).unwrap_or_default());
                }
            }
        }
    }
    out
}

#[test]
fn the_catalogue_is_structurally_sound_and_every_named_test_exists() {
    let cat = Catalogue::gather(&[mycelium::rules::RULES, mycelium_wasm_host::rules::RULES])
        .unwrap_or_else(|errs| panic!("{}", errs.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("\n")));
    assert!(cat.rules.len() >= 20, "{} rules", cat.rules.len());
    let src = source_text();
    let mut missing = Vec::new();
    for r in &cat.rules {
        for t in r.tests {
            if !src.contains(&format!("fn {t}(")) { missing.push(format!("{}: `{t}`", r.id)); }
        }
    }
    assert!(missing.is_empty(), "tests named by a descriptor that do not exist in the tree:\n{}", missing.join("\n"));
}

#[test]
fn the_checked_in_catalogue_is_current() {
    let cat = Catalogue::gather(&[mycelium::rules::RULES, mycelium_wasm_host::rules::RULES]).expect("sound");
    let json = serde_json::to_string_pretty(&cat).unwrap() + "\n";
    let md = cat.to_markdown();
    let root = repo_root();
    let (jp, mp) = (root.join("docs/reference/rule-catalogue.json"), root.join("docs/reference/rule-catalogue.md"));
    if std::env::var("UPDATE_RULE_CATALOGUE").is_ok() {
        std::fs::write(&jp, &json).unwrap();
        std::fs::write(&mp, &md).unwrap();
        return;
    }
    let have_json = std::fs::read_to_string(&jp).unwrap_or_default();
    let have_md = std::fs::read_to_string(&mp).unwrap_or_default();
    assert!(have_json == json && have_md == md, "docs/reference/rule-catalogue.{{json,md}} are stale: regenerate with UPDATE_RULE_CATALOGUE=1 cargo test -p mycelium-wasm-host --features stem --test rule_catalogue");
}
