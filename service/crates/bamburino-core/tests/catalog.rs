// SPDX-License-Identifier: AGPL-3.0-or-later
use bamburino_core::build_info;
use bamburino_core::catalog::Catalog;

#[test]
fn embedded_catalog_has_at_least_590_keys() {
    assert!(Catalog::embedded().options.len() >= 590);
}

#[test]
fn embedded_catalog_version_matches_build_info() {
    assert_eq!(
        Catalog::embedded().studio_version,
        build_info::STUDIO_VERSION
    );
}

#[test]
fn every_option_has_type_label_scopes() {
    for (key, def) in &Catalog::embedded().options {
        assert_eq!(&def.key, key);
        assert!(!def.label.is_empty(), "{key}: empty label");
        // Excluded options (out of scope for the service) carry no scopes.
        assert!(def.excluded || !def.scopes.is_empty(), "{key}: no scopes");
        assert!(
            !matches!(def.kind, bamburino_core::catalog::OptionKind::Other(_)),
            "{key}: unmapped kind {:?}",
            def.kind
        );
    }
}

#[test]
fn search_infill_density_finds_sparse_infill_density() {
    let hits = Catalog::embedded().search("infill density", 5);
    assert_eq!(hits[0].key, "sparse_infill_density");
}

#[test]
fn search_is_case_insensitive() {
    let hits = Catalog::embedded().search("GYROID", 20);
    assert!(hits.iter().any(|h| h.key == "sparse_infill_pattern"));
}

#[test]
fn search_respects_limit_and_empty_query() {
    let c = Catalog::embedded();
    assert_eq!(c.search("speed", 3).len(), 3);
    assert!(c.search("   ", 5).is_empty());
}

#[test]
fn suggest_typo() {
    let s = Catalog::embedded().suggest("sparse_infil_density", 3);
    assert!(s.contains(&"sparse_infill_density".to_string()), "{s:?}");
}

#[test]
fn suggest_italian_label_returns_empty_not_panic() {
    assert!(Catalog::embedded().suggest("riempimento", 3).is_empty());
}

#[test]
fn from_json_rejects_garbage_and_handles_default_shapes() {
    assert!(Catalog::from_json(b"not json").is_err());
    let json = br#"{"studio_version":"1","source_revision":"r","options":{
      "a":{"type":"FloatOrPercent","nullable":false,"scopes":["project"],"label":"A","description":"d","enum_values":[],"default":"400%","extra":1},
      "b":{"type":"Weird","nullable":true,"scopes":["plate"],"label":"B","description":"","enum_values":[],"default_expression":"x","excluded_reason":"no"}}}"#;
    let c = Catalog::from_json(json).unwrap();
    assert_eq!(c.get("a").unwrap().default.as_ref().unwrap(), "400%");
    let b = c.get("b").unwrap();
    assert!(b.default.is_none() && b.excluded);
    assert!(matches!(&b.kind, bamburino_core::catalog::OptionKind::Other(s) if s == "Weird"));
}

#[test]
fn search_by_exact_key_returns_it_first() {
    let c = Catalog::embedded();
    assert_eq!(
        c.search("sparse_infill_density", 5)[0].key,
        "sparse_infill_density"
    );
    let partial = c.search("infill_density", 5);
    assert!(!partial.is_empty());
    assert_eq!(partial[0].key, "sparse_infill_density");
}

#[test]
fn search_skips_excluded_options() {
    let c = Catalog::embedded();
    assert!(c.options["host_type"].excluded);
    assert!(
        c.search("host_type", 50)
            .iter()
            .all(|h| h.key != "host_type")
    );
    assert!(!c.suggest("host_type", 5).contains(&"host_type".to_string()));
}

#[test]
fn description_length_only_breaks_ties() {
    let long = "x".repeat(50_000);
    let opt = |label: &str, desc: &str, enums: &str| {
        format!(
            r#""{label}":{{"type":"Enum","scopes":["project"],"label":"{label}","description":"{desc}","enum_values":[{enums}]}}"#
        )
    };
    let json = format!(
        r#"{{"studio_version":"1","source_revision":"r","options":{{{},{},{},{}}}}}"#,
        opt("aa_label_foo", &long, ""), // label+key match, long description
        opt("bb", "foo", ""),           // description match only, short
        opt("cc_enum_long", &long, "\"foo\""), // enum match only, long
        opt("dd_enum_short", "s", "\"foo\""), // enum match only, short
    );
    let c = Catalog::from_json(json.as_bytes()).unwrap();
    let keys: Vec<_> = c.search("foo", 10).into_iter().map(|h| h.key).collect();
    // Real weights decide first (key 3 > desc 1 > enum 0.5); length only orders equals.
    assert_eq!(
        keys,
        ["aa_label_foo", "bb", "dd_enum_short", "cc_enum_long"]
    );
}
