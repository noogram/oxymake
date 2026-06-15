//! Architectural invariant: codec ↔ storage symbol independence.
//!
//! Defends OX-3 (Separable Axes) — `ox-codec-core` (encoding axis) and
//! `ox-storage-local` (location axis) must remain orthogonal. Neither
//! crate may take a direct dependency on the other; they communicate
//! only through `ox-core` abstractions.
//!
//! Implements the defensive CI gate that enforces this axis independence.

use std::process::Command;

#[derive(Debug, serde::Deserialize)]
struct Package {
    name: String,
    dependencies: Vec<Dependency>,
}

#[derive(Debug, serde::Deserialize)]
struct Dependency {
    name: String,
}

#[derive(Debug, serde::Deserialize)]
struct Metadata {
    packages: Vec<Package>,
}

fn workspace_metadata() -> Metadata {
    // Walk up to find the workspace root by locating a Cargo.toml with
    // `[workspace]`. CARGO_MANIFEST_DIR points at this crate; the
    // workspace root is its grandparent (`crates/ox-core/../..`).
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let workspace_root = std::path::Path::new(manifest_dir)
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root must exist two levels above CARGO_MANIFEST_DIR");

    let output = Command::new(env!("CARGO"))
        .arg("metadata")
        .arg("--format-version")
        .arg("1")
        .arg("--no-deps")
        .current_dir(workspace_root)
        .output()
        .expect("failed to invoke `cargo metadata`");
    assert!(
        output.status.success(),
        "`cargo metadata` failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("failed to parse cargo metadata JSON")
}

fn assert_no_dependency(metadata: &Metadata, from: &str, to: &str) {
    let pkg = metadata
        .packages
        .iter()
        .find(|p| p.name == from)
        .unwrap_or_else(|| panic!("expected to find package `{from}` in workspace metadata"));
    let bad = pkg.dependencies.iter().find(|d| d.name == to);
    assert!(
        bad.is_none(),
        "OX-3 violated: crate `{from}` depends on `{to}`. \
         The codec axis (encoding) and storage axis (location) must \
         remain orthogonal — they communicate only through `ox-core` \
         abstractions. Re-introducing a direct edge requires a fresh ADR."
    );
}

#[test]
fn codec_and_storage_are_separable() {
    let metadata = workspace_metadata();
    assert_no_dependency(&metadata, "ox-codec-core", "ox-storage-local");
    assert_no_dependency(&metadata, "ox-storage-local", "ox-codec-core");
}
