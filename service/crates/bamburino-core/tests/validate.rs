// SPDX-License-Identifier: AGPL-3.0-or-later
use bamburino_core::catalog::Catalog;
use bamburino_core::errors::{ToolError, ToolErrorCode};
use bamburino_core::validate::{Scope, validate_change};
use serde_json::{Value, json};

fn ok(key: &str, v: Value, scope: Scope) -> String {
    validate_change(Catalog::embedded(), key, &v, scope)
        .unwrap_or_else(|e| panic!("{key}={v}: {e:?}"))
        .0
}

fn err(key: &str, v: Value, scope: Scope) -> ToolError {
    validate_change(Catalog::embedded(), key, &v, scope).expect_err("expected error")
}

#[test]
fn percent_from_number() {
    assert_eq!(
        ok("sparse_infill_density", json!(35), Scope::Project),
        "35%"
    );
}

#[test]
fn percent_from_string() {
    assert_eq!(
        ok("sparse_infill_density", json!("35%"), Scope::Project),
        "35%"
    );
}

#[test]
fn percent_fraction_rejected() {
    let e = err("sparse_infill_density", json!(0.35), Scope::Project);
    assert_eq!(e.code, ToolErrorCode::InvalidValue);
    assert_eq!(e.details["hint"], "use 35 for 35 %");
}

#[test]
fn percent_over_100_rejected() {
    let e = err("sparse_infill_density", json!(101), Scope::Project);
    assert_eq!(e.code, ToolErrorCode::InvalidValue);
}

#[test]
fn enum_case_insensitive() {
    assert_eq!(
        ok("sparse_infill_pattern", json!("Gyroid"), Scope::Project),
        "gyroid"
    );
}

#[test]
fn enum_unknown_lists_allowed() {
    let e = err("sparse_infill_pattern", json!("spiral"), Scope::Project);
    assert_eq!(e.code, ToolErrorCode::InvalidValue);
    assert!(
        e.details["allowed"]
            .as_array()
            .unwrap()
            .contains(&json!("gyroid"))
    );
}

#[test]
fn int_forms() {
    assert_eq!(ok("wall_loops", json!(4), Scope::Project), "4");
    assert_eq!(ok("wall_loops", json!("4"), Scope::Project), "4");
}

#[test]
fn int_fraction_rejected() {
    assert_eq!(
        err("wall_loops", json!(4.5), Scope::Project).code,
        ToolErrorCode::InvalidValue
    );
}

#[test]
fn float_shortest_form() {
    assert_eq!(ok("layer_height", json!(0.2), Scope::Project), "0.2");
    assert_eq!(ok("layer_height", json!("0.2"), Scope::Project), "0.2");
    assert_eq!(ok("layer_height", json!(0.1 + 0.1), Scope::Project), "0.2");
}

#[test]
fn float_comma_rejected() {
    let e = err("layer_height", json!("0,20"), Scope::Project);
    assert_eq!(e.code, ToolErrorCode::InvalidValue);
    assert_eq!(e.details["hint"], "use a dot as decimal separator");
}

#[test]
fn bool_forms() {
    assert_eq!(ok("enable_support", json!(true), Scope::Project), "1");
    assert_eq!(ok("enable_support", json!("false"), Scope::Project), "0");
    assert_eq!(ok("enable_support", json!(1), Scope::Project), "1");
}

#[test]
fn enum_with_parens() {
    assert_eq!(
        ok("support_type", json!("tree(auto)"), Scope::Project),
        "tree(auto)"
    );
}

#[test]
fn unknown_key_has_candidates() {
    let e = err("sparse_infil_density", json!(10), Scope::Project);
    assert_eq!(e.code, ToolErrorCode::UnknownKey);
    assert_eq!(e.details["candidates"][0], "sparse_infill_density");
}

#[test]
fn scope_not_allowed() {
    let e = err("layer_height", json!(0.2), Scope::Plate);
    assert_eq!(e.code, ToolErrorCode::ScopeNotAllowed);
    assert!(
        e.details["allowed_scopes"]
            .as_array()
            .unwrap()
            .contains(&json!("project"))
    );
}

#[test]
fn demo_keys_accept_project_scope() {
    ok("sparse_infill_density", json!(20), Scope::Project);
    ok("sparse_infill_pattern", json!("gyroid"), Scope::Project);
    ok("wall_loops", json!(3), Scope::Project);
    ok("layer_height", json!(0.2), Scope::Project);
    ok("enable_support", json!(true), Scope::Project);
}

#[test]
fn support_keys_accept_object_scope() {
    ok("enable_support", json!(true), Scope::Object);
    ok("support_type", json!("tree(auto)"), Scope::Object);
}

#[test]
fn excluded_options_not_writable() {
    let cat = Catalog::embedded();
    let o = cat
        .options
        .values()
        .find(|o| o.excluded)
        .expect("an excluded option");
    let e = validate_change(cat, &o.key, &json!(1), Scope::Project).unwrap_err();
    assert_eq!(e.code, ToolErrorCode::NotWritable);
}

#[test]
fn revision_conflict_to_json() {
    let j = ToolError::new(ToolErrorCode::RevisionConflict, "stale").to_json();
    assert_eq!(j["error"]["http_status"], 409);
    assert_eq!(j["error"]["code"], "REVISION_CONFLICT");
}

fn code(key: &str, v: Value) -> ToolErrorCode {
    err(key, v, Scope::Project).code
}

#[test]
fn strings_use_bambu_cstyle_quoting() {
    assert_eq!(
        ok(
            "compatible_printers",
            json!(["#FF0000", "#00FF00"]),
            Scope::Project
        ),
        r##""#FF0000";"#00FF00""##
    );
    assert_eq!(
        ok("compatible_printers", json!([r#"a;b"c\d"#]), Scope::Project),
        r#""a;b\"c\\d""#
    );
}

#[test]
fn numeric_vectors_join_with_comma() {
    assert_eq!(
        ok("bridge_speed", json!([10, 20.5]), Scope::Project),
        "10,20.5"
    );
    assert_eq!(
        ok(
            "activate_air_filtration",
            json!([true, false]),
            Scope::Project
        ),
        "1,0"
    );
}

#[test]
fn empty_array_rejected() {
    let e = err("compatible_printers", json!([]), Scope::Project);
    assert_eq!(e.code, ToolErrorCode::InvalidValue);
    assert!(e.details["hint"].is_string());
}

#[test]
fn scalar_for_vector_rejected() {
    assert_eq!(code("bridge_speed", json!(10)), ToolErrorCode::InvalidValue);
}

#[test]
fn null_rejected_with_hint() {
    for k in [
        "layer_height",
        "wall_loops",
        "enable_support",
        "support_type",
        "sparse_infill_density",
    ] {
        let e = err(k, Value::Null, Scope::Project);
        assert_eq!(e.code, ToolErrorCode::InvalidValue, "{k}");
        assert_eq!(
            e.details["hint"],
            "resetting to the inherited value is not supported in this version"
        );
    }
}

#[test]
fn enum_uses_serialized_values() {
    let cat = Catalog::embedded();
    let o = cat.get("filament_map_mode").unwrap();
    assert!(
        o.serialized_enum_values
            .contains(&"Auto For Quality".to_string())
    );
    assert_eq!(
        ok(
            "filament_map_mode",
            json!("auto for quality"),
            Scope::Project
        ),
        "Auto For Quality"
    );
    let e = err("filament_map_mode", json!("Default"), Scope::Project);
    assert_eq!(e.code, ToolErrorCode::InvalidValue);
    assert!(
        e.details["allowed"]
            .as_array()
            .unwrap()
            .contains(&json!("Auto For Quality"))
    );
}

#[test]
fn enums_vector_uses_serialized_values() {
    let k = "overhang_threshold_participating_cooling";
    assert_eq!(ok(k, json!(["95%", "0%"]), Scope::Project), "95%,0%");
    let e = err(k, json!(["100%"]), Scope::Project);
    assert_eq!(e.code, ToolErrorCode::InvalidValue);
    assert!(
        e.details["allowed"]
            .as_array()
            .unwrap()
            .contains(&json!("95%"))
    );
}

#[test]
fn percent_uses_catalog_range_only() {
    let cat = Catalog::embedded();
    let o = cat.get("monotonic_travel_into_wall").expect("key");
    assert_eq!(o.max, Some(200.0));
    assert_eq!(
        ok("monotonic_travel_into_wall", json!(150), Scope::Project),
        "150%"
    );
    assert_eq!(
        code("monotonic_travel_into_wall", json!(201)),
        ToolErrorCode::InvalidValue
    );
}

#[test]
fn negative_percent_rejected() {
    assert_eq!(
        code("sparse_infill_density", json!(-5)),
        ToolErrorCode::InvalidValue
    );
    assert_eq!(
        code("sparse_infill_density", json!("-5%")),
        ToolErrorCode::InvalidValue
    );
    assert_eq!(
        code("retract_before_wipe", json!(-5)),
        ToolErrorCode::InvalidValue
    );
}

#[test]
fn double_percent_rejected() {
    assert_eq!(
        code("sparse_infill_density", json!("35%%")),
        ToolErrorCode::InvalidValue
    );
}

#[test]
fn float_or_percent_forms() {
    assert_eq!(ok("seam_slope_gap", json!(0.5), Scope::Project), "0.5");
    assert_eq!(ok("seam_slope_gap", json!("15%"), Scope::Project), "15%");
    assert_eq!(
        code("seam_slope_gap", json!(-1)),
        ToolErrorCode::InvalidValue
    );
    assert_eq!(
        code("seam_slope_gap", json!("-1%")),
        ToolErrorCode::InvalidValue
    );
    assert_eq!(
        code("seam_slope_gap", json!("1,5")),
        ToolErrorCode::InvalidValue
    );
}

#[test]
fn int_tolerances() {
    assert_eq!(ok("wall_loops", json!(4.0), Scope::Project), "4");
    assert_eq!(ok("wall_loops", json!("4.0"), Scope::Project), "4");
    assert_eq!(
        code("wall_loops", json!("4.5")),
        ToolErrorCode::InvalidValue
    );
    assert_eq!(
        code("wall_loops", json!(3_000_000_000_i64)),
        ToolErrorCode::InvalidValue
    );
}

#[test]
fn bool_tolerances() {
    assert_eq!(ok("enable_support", json!(1.0), Scope::Project), "1");
    assert_eq!(ok("enable_support", json!("0.0"), Scope::Project), "0");
    assert_eq!(
        code("enable_support", json!(2)),
        ToolErrorCode::InvalidValue
    );
}

#[test]
fn readonly_not_writable() {
    let e = err(
        "machine_max_acceleration_travel",
        json!([1, 2]),
        Scope::Project,
    );
    assert_eq!(e.code, ToolErrorCode::NotWritable);
}

#[test]
fn unsupported_kind_not_writable() {
    let e = err("best_object_pos", json!([0.5, 0.5]), Scope::Project);
    assert_eq!(e.code, ToolErrorCode::NotWritable);
    assert!(e.message.contains("not supported in MVP"));
}
