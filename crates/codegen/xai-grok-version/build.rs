use std::fs;
use std::path::PathBuf;

fn main() {
    let manifest_dir =
        PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set"));
    let shipping_manifest = manifest_dir.join("../xai-grok-pager-bin/Cargo.toml");
    println!("cargo:rerun-if-changed={}", shipping_manifest.display());

    let manifest = fs::read_to_string(&shipping_manifest)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", shipping_manifest.display()));
    let version = manifest
        .lines()
        .find_map(|line| {
            line.strip_prefix("version = \"")
                .and_then(|value| value.strip_suffix('"'))
        })
        .expect("xai-grok-pager-bin package version must be present");

    println!("cargo:rustc-env=DTTN_VERSION={version}");
}
