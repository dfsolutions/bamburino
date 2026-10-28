// SPDX-License-Identifier: AGPL-3.0-or-later
//! Validation and normalization of a setting change against the catalog.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::catalog::{Catalog, CatalogScope, OptionDef, OptionKind};
use crate::errors::{ToolError, ToolErrorCode};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Project,
    Plate,
    Object,
}

impl Scope {
    fn to_catalog(self) -> CatalogScope {
        match self {
            Scope::Project => CatalogScope::Project,
            Scope::Plate => CatalogScope::Plate,
            Scope::Object => CatalogScope::Object,
        }
    }
}

/// Native serialized string accepted by the driver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeValue(pub String);

fn invalid(key: &str, msg: impl Into<String>, details: Value) -> ToolError {
    ToolError::new(
        ToolErrorCode::InvalidValue,
        format!("{key}: {}", msg.into()),
    )
    .with_details(details)
}

fn null_error(key: &str) -> ToolError {
    invalid(
        key,
        "null is not accepted",
        json!({ "hint": "resetting to the inherited value is not supported in this version" }),
    )
}

/// Shortest round-trip decimal form (`0.2`, never `0.20000000000000001`).
fn fmt_f64(v: f64) -> String {
    format!("{v}")
}

/// Validates `value` for `key` at `scope` and returns the native string form.
///
/// Deliberate deviations from the original brief (controller rulings):
/// - `Strings` vectors are serialized as Bambu Studio does
///   (`escape_strings_cstyle`): each element wrapped in `"`, with `\` and `"`
///   backslash-escaped (newlines as `\n`/`\r`), elements joined by `;`.
///   Other vector kinds are joined by `,`.
/// - An empty array is `InvalidValue` (Bambu would read an empty vector).
/// - Enums are matched case-insensitively against the *serialized* values
///   (`serialized_enum_values`, falling back to `enum_values`); the canonical
///   serialized form is emitted and `allowed` lists the serialized values.
/// - Percent range comes from the catalog min/max only (no fixed `0..=100`),
///   but negative percent values are rejected when `min` is absent or `>= 0`.
/// - `null` is rejected even for nullable options: resetting to the inherited
///   value is not supported in this version.
/// - Numeric tolerance is symmetric: integral decimals (`4.0`, `"4.0"`) are
///   accepted for Int and `1.0`/`0.0` (or `"1.0"`/`"0.0"`) for Bool; Int must
///   fit in i32.
pub fn validate_change(
    cat: &Catalog,
    key: &str,
    value: &Value,
    scope: Scope,
) -> Result<NativeValue, ToolError> {
    let Some(opt) = cat.get(key) else {
        return Err(
            ToolError::new(ToolErrorCode::UnknownKey, format!("unknown key `{key}`"))
                .with_details(json!({ "candidates": cat.suggest(key, 5) })),
        );
    };
    if opt.excluded || opt.readonly {
        let why = if opt.excluded {
            "excluded"
        } else {
            "read-only"
        };
        return Err(ToolError::new(
            ToolErrorCode::NotWritable,
            format!("`{key}` is {why} and cannot be changed"),
        ));
    }
    let want = scope.to_catalog();
    if !opt.scopes.contains(&want) {
        let allowed: Vec<&str> = opt.scopes.iter().map(CatalogScope::as_str).collect();
        return Err(ToolError::new(
            ToolErrorCode::ScopeNotAllowed,
            format!("`{key}` cannot be set at scope `{}`", want.as_str()),
        )
        .with_details(json!({ "allowed_scopes": allowed })));
    }
    normalize(opt, value).map(NativeValue)
}

fn normalize(opt: &OptionDef, v: &Value) -> Result<String, ToolError> {
    match &opt.kind {
        OptionKind::Point
        | OptionKind::Points
        | OptionKind::PointsGroups
        | OptionKind::IntsGroups
        | OptionKind::Other(_) => Err(ToolError::new(
            ToolErrorCode::NotWritable,
            format!("`{}`: type not supported in MVP", opt.key),
        )),
        OptionKind::Bools
        | OptionKind::Ints
        | OptionKind::Floats
        | OptionKind::Percents
        | OptionKind::FloatsOrPercents
        | OptionKind::Strings
        | OptionKind::Enums => {
            let Some(items) = v.as_array() else {
                if v.is_null() {
                    return Err(null_error(&opt.key));
                }
                return Err(invalid(
                    &opt.key,
                    "expected a JSON array",
                    json!({ "hint": "pass a list of values" }),
                ));
            };
            if items.is_empty() {
                return Err(invalid(
                    &opt.key,
                    "empty list",
                    json!({ "hint": "provide at least one element" }),
                ));
            }
            let parts = items
                .iter()
                .map(|i| scalar(opt, i))
                .collect::<Result<Vec<_>, _>>()?;
            if opt.kind == OptionKind::Strings {
                let quoted: Vec<String> = parts.iter().map(|p| quote_cstyle(p)).collect();
                Ok(quoted.join(";"))
            } else {
                Ok(parts.join(","))
            }
        }
        _ => scalar(opt, v),
    }
}

/// Normalizes one scalar (or one element of a vector kind).
fn scalar(opt: &OptionDef, v: &Value) -> Result<String, ToolError> {
    let key = opt.key.as_str();
    match &opt.kind {
        OptionKind::Bool | OptionKind::Bools => bool_value(key, v),
        OptionKind::Int | OptionKind::Ints => {
            let n = int_value(key, v)?;
            check_range(opt, n as f64)?;
            Ok(n.to_string())
        }
        OptionKind::Float | OptionKind::Floats => {
            let n = float_value(key, v)?;
            check_range(opt, n)?;
            Ok(fmt_f64(n))
        }
        OptionKind::Percent | OptionKind::Percents => {
            let n = percent_value(key, v)?;
            check_range(opt, n)?;
            check_non_negative(opt, n)?;
            Ok(format!("{}%", fmt_f64(n)))
        }
        OptionKind::FloatOrPercent | OptionKind::FloatsOrPercents => {
            if v.as_str().is_some_and(|s| s.trim().ends_with('%')) {
                let n = percent_value(key, v)?;
                check_non_negative(opt, n)?;
                return Ok(format!("{}%", fmt_f64(n)));
            }
            // min/max describe the plain float form only, not the percent form.
            let n = float_value(key, v)?;
            check_range(opt, n)?;
            check_non_negative(opt, n)?;
            Ok(fmt_f64(n))
        }
        OptionKind::String | OptionKind::Strings => match v.as_str() {
            Some(s) => Ok(s.to_string()),
            None if v.is_null() => Err(null_error(key)),
            None => Err(invalid(key, "expected a string", json!({}))),
        },
        OptionKind::Enum | OptionKind::Enums => {
            let Some(s) = v.as_str() else {
                if v.is_null() {
                    return Err(null_error(key));
                }
                return Err(invalid(key, "expected a string", enum_details(opt)));
            };
            enum_values(opt)
                .iter()
                .find(|e| e.eq_ignore_ascii_case(s.trim()))
                .cloned()
                .ok_or_else(|| {
                    invalid(
                        key,
                        format!("`{s}` is not an allowed value"),
                        enum_details(opt),
                    )
                })
        }
        _ => Err(ToolError::new(
            ToolErrorCode::NotWritable,
            format!("`{key}`: type not supported in MVP"),
        )),
    }
}

/// The strings Bambu Studio really serializes for an enum option.
fn enum_values(opt: &OptionDef) -> &[String] {
    if opt.serialized_enum_values.is_empty() {
        &opt.enum_values
    } else {
        &opt.serialized_enum_values
    }
}

fn enum_details(opt: &OptionDef) -> Value {
    json!({ "allowed": enum_values(opt) })
}

/// Bambu `escape_strings_cstyle` for one element.
fn quote_cstyle(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn check_range(opt: &OptionDef, n: f64) -> Result<(), ToolError> {
    if let Some(min) = opt.min
        && n < min
    {
        return Err(invalid(
            &opt.key,
            format!("{} is below the minimum {}", fmt_f64(n), fmt_f64(min)),
            json!({ "min": min, "max": opt.max }),
        ));
    }
    if let Some(max) = opt.max
        && n > max
    {
        return Err(invalid(
            &opt.key,
            format!("{} is above the maximum {}", fmt_f64(n), fmt_f64(max)),
            json!({ "min": opt.min, "max": max }),
        ));
    }
    Ok(())
}

fn check_non_negative(opt: &OptionDef, n: f64) -> Result<(), ToolError> {
    if n < 0.0 && opt.min.is_none_or(|m| m >= 0.0) {
        return Err(invalid(
            &opt.key,
            format!("{} is negative", fmt_f64(n)),
            json!({ "hint": "negative values are not allowed for this option" }),
        ));
    }
    Ok(())
}

fn bool_value(key: &str, v: &Value) -> Result<String, ToolError> {
    let b = match v {
        Value::Bool(b) => Some(*b),
        Value::Number(n) => match n.as_f64() {
            Some(1.0) => Some(true),
            Some(0.0) => Some(false),
            _ => None,
        },
        Value::String(s) => match s.trim().to_ascii_lowercase().as_str() {
            "true" | "1" | "1.0" => Some(true),
            "false" | "0" | "0.0" => Some(false),
            _ => None,
        },
        Value::Null => return Err(null_error(key)),
        _ => None,
    };
    match b {
        Some(true) => Ok("1".into()),
        Some(false) => Ok("0".into()),
        None => Err(invalid(
            key,
            "expected a boolean",
            json!({ "hint": "use true or false" }),
        )),
    }
}

fn int_value(key: &str, v: &Value) -> Result<i64, ToolError> {
    let parsed = match v {
        Value::Number(n) => n.as_i64().or_else(|| {
            n.as_f64()
                .filter(|f| f.fract() == 0.0 && f.abs() < 9e15)
                .map(|f| f as i64)
        }),
        Value::String(s) => {
            let t = s.trim();
            let digits_only = |d: &str| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit());
            match t.split_once('.') {
                None => digits_only(t.strip_prefix('-').unwrap_or(t))
                    .then(|| t.parse().ok())
                    .flatten(),
                // "4.0": an integral decimal is accepted, like JSON 4.0.
                Some((int, frac)) => {
                    let ok = digits_only(int.strip_prefix('-').unwrap_or(int))
                        && !frac.is_empty()
                        && frac.bytes().all(|b| b == b'0');
                    if ok { int.parse().ok() } else { None }
                }
            }
        }
        Value::Null => return Err(null_error(key)),
        _ => None,
    };
    parsed
        .filter(|n| i32::try_from(*n).is_ok())
        .ok_or_else(|| invalid(key, "expected an integer in the 32-bit range", json!({})))
}

fn float_value(key: &str, v: &Value) -> Result<f64, ToolError> {
    match v {
        Value::Number(n) => n
            .as_f64()
            .filter(|f| f.is_finite())
            .ok_or_else(|| invalid(key, "expected a number", json!({}))),
        Value::String(s) => {
            let t = s.trim();
            if t.contains(',') {
                return Err(invalid(
                    key,
                    "comma is not accepted in numbers",
                    json!({ "hint": "use a dot as decimal separator" }),
                ));
            }
            t.parse::<f64>()
                .ok()
                .filter(|f| f.is_finite())
                .ok_or_else(|| invalid(key, "expected a number", json!({})))
        }
        Value::Null => Err(null_error(key)),
        _ => Err(invalid(key, "expected a number", json!({}))),
    }
}

/// Numeric part of a percent value: a number or a string such as `"35%"`
/// (exactly one trailing `%`).
fn percent_value(key: &str, v: &Value) -> Result<f64, ToolError> {
    let (n, from_pct_string) = match v {
        Value::String(s) if s.trim().ends_with('%') => {
            let body = s.trim().strip_suffix('%').unwrap_or_default();
            (float_value(key, &Value::String(body.to_string()))?, true)
        }
        other => (float_value(key, other)?, false),
    };
    if !from_pct_string && n > 0.0 && n < 1.0 {
        return Err(invalid(
            key,
            format!("{} looks like a fraction, not a percentage", fmt_f64(n)),
            json!({ "hint": "use 35 for 35 %" }),
        ));
    }
    Ok(n)
}
