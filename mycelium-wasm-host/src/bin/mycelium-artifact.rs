//! `mycelium-artifact publish | list | verify` — the artifact tool
//! (`docs/plans/design-time-tooling.md` §10, D10 / A1; §11 S2 for a store URL; the functions
//! are in `mycelium_wasm_host`).
//!
//! ```text
//! mycelium-artifact publish <description.toml> --library <dir|url> (--key <seed-file> | --key-env <VAR>)
//! mycelium-artifact list    <library-dir|url>
//! mycelium-artifact verify  <library-dir|url> [--trusted ed25519:<hex>]... [--descriptions <dir>]
//! ```
//!
//! A library is a directory, or — built with the `object_store` feature — a store URL:
//! `s3://bucket/prefix`, `gs://bucket/prefix`, `az://…`, `https://host/prefix`, `file:///dir`,
//! with the builders' credentials taken from the environment. The signed manifest stays the
//! library's truth (in the store, at `<prefix>/manifest`); the description is what a reviewer
//! reads, and this tool derives the one from the other. `verify` exits 1 with every problem named.

use std::path::PathBuf;

use mycelium_wasm_host::{list_manifest, publish_artifact, publisher_from_str, signing_key_from_hex, verify_library, VerifyReport};

fn usage() -> ! {
    eprintln!(
        "Usage:\n\
         \x20 mycelium-artifact publish <description.toml> --library <dir|url> (--key <seed-file> | --key-env <VAR>)\n\
         \x20 mycelium-artifact list    <library-dir|url>\n\
         \x20 mycelium-artifact verify  <library-dir|url> [--trusted ed25519:<hex>]... [--descriptions <dir>]\n\
         \n\
         A library is a directory or, with the object_store feature, a store URL (s3://, gs://, az://, https://, file://)\n\
         whose credentials come from the environment. A signing key is a 32-byte Ed25519 seed as 64 hex characters.\n\
         Exit 0 on success, 1 when verify finds a problem, 2 on a usage or file error."
    );
    std::process::exit(2)
}

fn is_url(s: &str) -> bool {
    s.contains("://")
}

fn fail(e: impl std::fmt::Display) -> i32 {
    eprintln!("mycelium-artifact: {e}");
    2
}

/// Run an async store operation on a runtime built for the call.
#[cfg(feature = "object_store")]
fn block_on<T>(f: impl std::future::Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread().enable_all().build().expect("runtime").block_on(f)
}

#[cfg(feature = "object_store")]
fn open_store(url: &str) -> Result<mycelium_wasm_host::ObjectStoreFetcher, String> {
    let egress = std::env::var("MYCELIUM_EGRESS_ALLOW_HOSTS")
        .ok()
        .map(|v| mycelium::EgressPolicy { allow_hosts: v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect() })
        .unwrap_or_default();
    mycelium_wasm_host::ObjectStoreFetcher::from_url(url, egress)
}

#[cfg(not(feature = "object_store"))]
fn not_built(url: &str) -> i32 {
    fail(format!("{url}: a store URL needs a build with the object_store feature"))
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

fn report_publish(out: mycelium_wasm_host::PublishOutcome) -> i32 {
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

fn publish(args: &[String]) -> i32 {
    let mut description: Option<PathBuf> = None;
    let mut library: Option<String> = None;
    let mut key_text: Option<String> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--library" => library = it.next().cloned(),
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
        Err(e) => return fail(format!("key: {e}")),
    };
    if is_url(&library) {
        #[cfg(feature = "object_store")]
        {
            return match open_store(&library) {
                Ok(store) => match block_on(mycelium_wasm_host::publish_to_store(&description, &store, &key)) {
                    Ok(out) => report_publish(out),
                    Err(e) => fail(e),
                },
                Err(e) => fail(e),
            };
        }
        #[cfg(not(feature = "object_store"))]
        return not_built(&library);
    }
    match publish_artifact(&description, std::path::Path::new(&library), &key) {
        Ok(out) => report_publish(out),
        Err(e) => fail(e),
    }
}

fn list(args: &[String]) -> i32 {
    let Some(library) = args.first() else { usage() };
    let listed = if is_url(library) {
        #[cfg(feature = "object_store")]
        {
            open_store(library).and_then(|store| block_on(mycelium_wasm_host::list_store(&store)))
        }
        #[cfg(not(feature = "object_store"))]
        return not_built(library);
    } else {
        list_manifest(std::path::Path::new(library))
    };
    match listed {
        Ok(text) => {
            print!("{text}");
            0
        }
        Err(e) => fail(e),
    }
}

fn report_verify(r: VerifyReport) -> i32 {
    if r.problems.is_empty() {
        println!("verified {} manifest line(s): ok", r.checked);
        0
    } else {
        for p in &r.problems {
            eprintln!("problem: {p}");
        }
        eprintln!("verified {} manifest line(s): {} problem(s)", r.checked, r.problems.len());
        1
    }
}

fn verify(args: &[String]) -> i32 {
    let mut library: Option<String> = None;
    let mut trusted = Vec::new();
    let mut descriptions: Option<PathBuf> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--trusted" => match publisher_from_str(it.next().unwrap_or_else(|| usage())) {
                Ok(k) => trusted.push(k),
                Err(e) => return fail(e),
            },
            "--descriptions" => descriptions = it.next().map(PathBuf::from),
            other if other.starts_with('-') => usage(),
            other => library = Some(other.to_string()),
        }
    }
    let Some(library) = library else { usage() };
    if is_url(&library) {
        #[cfg(feature = "object_store")]
        {
            if descriptions.is_some() {
                eprintln!("mycelium-artifact: --descriptions is checked against a library directory; against a store, verify checks provenance and the streamed hash");
            }
            return match open_store(&library).and_then(|store| block_on(mycelium_wasm_host::verify_store(&store, &trusted))) {
                Ok(r) => report_verify(r),
                Err(e) => fail(e),
            };
        }
        #[cfg(not(feature = "object_store"))]
        return not_built(&library);
    }
    match verify_library(std::path::Path::new(&library), &trusted, descriptions.as_deref()) {
        Ok(r) => report_verify(r),
        Err(e) => fail(e),
    }
}
