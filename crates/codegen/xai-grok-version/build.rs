use std::fs;
use std::path::PathBuf;

fn main() {
    let manifest_dir =
        PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set"));
    let shipping_manifest = manifest_dir.join("../xai-grok-pager-bin/Cargo.toml");
    println!("cargo:rerun-if-changed={}", shipping_manifest.display());
    println!("cargo:rerun-if-env-changed=DTTN_VERSION");

    let manifest = fs::read_to_string(&shipping_manifest)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", shipping_manifest.display()));
    let shipping_version = manifest
        .lines()
        .find_map(|line| {
            line.strip_prefix("version = \"")
                .and_then(|value| value.strip_suffix('"'))
        })
        .expect("xai-grok-pager-bin package version must be present");

    let version = match std::env::var("DTTN_VERSION") {
        Ok(version) => {
            assert_eq!(
                version, shipping_version,
                "DTTN_VERSION must match the shipping xai-grok-pager-bin package version"
            );
            version
        }
        Err(std::env::VarError::NotPresent) => shipping_version.to_owned(),
        Err(error) => panic!("DTTN_VERSION is not valid Unicode: {error}"),
    };

    println!("cargo:rustc-env=DTTN_VERSION={version}");
}
