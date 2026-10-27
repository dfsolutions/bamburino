// SPDX-License-Identifier: AGPL-3.0-or-later
//! Reads `driver/upstream.toml` (single source of truth shared with the driver
//! build) and emits the constants included by `src/build_info.rs`.

use std::{env, fs, path::PathBuf};

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    // crates/bamburino-core -> service -> repo root
    let repo_root = manifest_dir.join("../../..");
    let upstream = repo_root.join("driver/upstream.toml");
    let catalog = repo_root.join("catalog/catalog.json");

    println!("cargo:rerun-if-changed={}", upstream.display());
    println!("cargo:rerun-if-changed={}", catalog.display());

    let text = fs::read_to_string(&upstream)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", upstream.display()));
    let doc: toml::Table = text
        .parse()
        .unwrap_or_else(|e| panic!("invalid {}: {e}", upstream.display()));

    let studio_version = field(&doc, "studio", "version");
    let tag_commit = field(&doc, "studio", "tag_commit");
    let patch_id = field(&doc, "driver", "patch_id");
    assert!(
        !studio_version.is_empty(),
        "studio.version must not be empty"
    );
    assert!(
        !tag_commit.is_empty(),
        "studio.tag_commit must not be empty"
    );

    let expected = if patch_id.is_empty() {
        "None".to_string()
    } else {
        format!("Some({patch_id:?})")
    };
    let out = format!(
        "pub const STUDIO_VERSION: &str = {studio_version:?};\n\
         pub const TAG_COMMIT: &str = {tag_commit:?};\n\
         pub const EXPECTED_PATCH_ID: Option<&str> = {expected};\n"
    );
    let dest = PathBuf::from(env::var("OUT_DIR").unwrap()).join("build_info_generated.rs");
    fs::write(dest, out).unwrap();
}

fn field(doc: &toml::Table, table: &str, key: &str) -> String {
    doc.get(table)
        .and_then(|t| t.get(key))
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| panic!("upstream.toml: missing string `{table}.{key}`"))
        .to_string()
}
