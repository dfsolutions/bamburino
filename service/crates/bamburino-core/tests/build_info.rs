// SPDX-License-Identifier: AGPL-3.0-or-later
#[test]
fn build_info_matches_upstream_toml() {
    assert_eq!(bamburino_core::build_info::STUDIO_VERSION, "02.08.04.57");
    assert_eq!(
        bamburino_core::build_info::TAG_COMMIT,
        "f977235e6d736c4c0b650520ac5a5b72cbfe9244"
    );
    if let Some(id) = bamburino_core::build_info::EXPECTED_PATCH_ID {
        assert!(id.len() == 12 && id.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
