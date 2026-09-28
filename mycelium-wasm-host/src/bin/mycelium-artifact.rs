//! `mycelium-artifact publish | list | verify` — the artifact tool
//! (`docs/plans/design-time-tooling.md` §10, D10 / A1; the functions are in `mycelium_wasm_host`).
//!
//! ```text
//! mycelium-artifact publish <description.toml> --library <dir> (--key <seed-file> | --key-env <VAR>)
//! mycelium-artifact list    <library-dir>
//! mycelium-artifact verify  <library-dir> [--trusted ed25519:<hex>]... [--descriptions <dir>]
//! ```
//!
//! The signed manifest stays the library's truth; the description is what a reviewer reads, and
//! this tool derives the one from the other. `verify` exits 1 with every problem named.

use std::path::PathBuf;

use mycelium_wasm_host::{list_manifest, publish_artifact, publisher_from_str, signing_key_from_hex, verify_library};

fn usage() -> ! {
    eprintln!(
        "Usage:\n\
         \x20 mycelium-artifact publish <description.toml> --library <dir> (--key <seed-file> | --key-env <VAR>)\n\
         \x20 mycelium-artifact list    <library-dir>\n\
         \x20 mycelium-artifact verify  <library-dir> [--trusted ed25519:<hex>]... [--descriptions <dir>]\n\
         \n\
         A signing key is a 32-byte Ed25519 seed as 64 hex characters, in a file or an environment variable.\n\
         Exit 0 on success, 1 when verify finds a problem, 2 on a usage or file error."
    );
    std::process::exit(2)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first() else { usage() };
    let code = match cmd.as_str() {
        "publish" => publish(&args[1..]),
        "list" => list(&args[1..]),
        "verify" => verify(&args[1..]),
        _ => usage(),
    };
    std::process::exit(code);
}

fn publish(args: &[String]) -> i32 {
    let mut description: Option<PathBuf> = None;
    let mut library: Option<PathBuf> = None;
    let mut key_text: Option<String> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--library" => library = it.next().map(PathBuf::from),
            "--key" => {
                let path = it.next().unwrap_or_else(|| usage());
                key_text = Some(std::fs::read_to_string(path).unwrap_or_else(|e| {
                    eprintln!("mycelium-artifact: {path}: {e}");
                    std::process::exit(2)
                }));
            }
            "--key-env" => {
                let var = it.next().unwrap_or_else(|| usage());
                key_text = Some(std::env::var(var).unwrap_or_else(|_| {
                    eprintln!("mycelium-artifact: ${var} is not set");
                    std::process::exit(2)
                }));
            }
            other if other.starts_with('-') => usage(),
            other => description = Some(PathBuf::from(other)),
        }
    }
    let (Some(description), Some(library), Some(key_text)) = (description, library, key_text) else { usage() };
    let key = match signing_key_from_hex(&key_text) {
        Ok(k) => k,
        Err(e) => {
            eprintln!("mycelium-artifact: key: {e}");
            return 2;
        }
    };
    match publish_artifact(&description, &library, &key) {
        Ok(out) => {
            println!(
                "published {}/{} as {} ({} B, {}) signed by {}",
                out.provides.namespace,
                out.provides.name,
                out.artifact.to_hex(),
                out.size_bytes,
                mycelium_wasm_host::kind_name(out.kind),
                out.signer
            );
            0
        }
        Err(e) => {
            eprintln!("mycelium-artifact: {e}");
            2
        }
    }
}

fn list(args: &[String]) -> i32 {
    let Some(library) = args.first() else { usage() };
    match list_manifest(std::path::Path::new(library)) {
        Ok(text) => {
            print!("{text}");
            0
        }
        Err(e) => {
            eprintln!("mycelium-artifact: {e}");
            2
        }
    }
}

fn verify(args: &[String]) -> i32 {
    let mut library: Option<PathBuf> = None;
    let mut trusted = Vec::new();
    let mut descriptions: Option<PathBuf> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--trusted" => match publisher_from_str(it.next().unwrap_or_else(|| usage())) {
                Ok(k) => trusted.push(k),
                Err(e) => {
                    eprintln!("mycelium-artifact: {e}");
                    return 2;
                }
            },
            "--descriptions" => descriptions = it.next().map(PathBuf::from),
            other if other.starts_with('-') => usage(),
            other => library = Some(PathBuf::from(other)),
        }
    }
    let Some(library) = library else { usage() };
    match verify_library(&library, &trusted, descriptions.as_deref()) {
        Ok(r) if r.problems.is_empty() => {
            println!("verified {} manifest line(s): ok", r.checked);
            0
        }
        Ok(r) => {
            for p in &r.problems {
                eprintln!("problem: {p}");
            }
            eprintln!("verified {} manifest line(s): {} problem(s)", r.checked, r.problems.len());
            1
        }
        Err(e) => {
            eprintln!("mycelium-artifact: {e}");
            2
        }
    }
}
